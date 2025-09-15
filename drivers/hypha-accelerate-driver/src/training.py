import argparse
import json
import os
from collections.abc import Iterable

import torch
import torch.utils.data
from accelerate import Accelerator
from safetensors import safe_open
from safetensors.torch import save_model
from tqdm import tqdm
from transformers import GPT2Tokenizer, GPTNeoForCausalLM
from transformers.tokenization_utils import PreTrainedTokenizer

from api import Session


def get_model(model_config: str) -> tuple[torch.nn.Module, PreTrainedTokenizer]:
    # Initializing a model (with random weights) from the EleutherAI/gpt-neo-1.3B style configuration
    model = GPTNeoForCausalLM.from_pretrained(model_config)
    tokenizer = GPT2Tokenizer.from_pretrained(model_config)
    return model, tokenizer


def get_optimizer(
    optimizer_name: str, learning_rate: float, parameters: Iterable[torch.Tensor]
) -> torch.optim.Optimizer:
    if optimizer_name == "AdamW":
        return torch.optim.AdamW(parameters, learning_rate)
    else:
        raise RuntimeError(f"Optimizer {optimizer_name} doesn't exist.")


def get_data_loader(data_set_name: str, batch_size: int) -> torch.utils.data.DataLoader[int]:
    with safe_open(data_set_name, framework="pt", device="cpu") as data:
        ds = data.get_tensor(data.keys()[0])
        return torch.utils.data.DataLoader(ds, batch_size=batch_size)


def merge_models(a: torch.nn.Module, weight_path: str, alpha: float) -> torch.nn.Module:
    # All weights need to be on CPU
    a.to("cpu")
    state_dict = a.state_dict()
    with safe_open(weight_path, framework="pt", device="cpu") as b:  # type: ignore
        for name in b.keys():  # noqa: SIM118
            state_dict[name] += alpha * (b.get_tensor(name) - state_dict[name])
    a.load_state_dict(state_dict)
    return a


def main(socket_path: str, work_dir: str, job_json: str) -> None:  # noqa: PLR0915
    # Background receiver context that fills a queue with update pointers
    with Session(socket_path) as session:
        job_spec = json.loads(job_json)

        # ensure that type is `parameter-server`
        assert job_spec["executor"]["type"] == "diloco-transformer"

        print(json.dumps(job_spec))
        job_spec["executor"]["optimizer"] = "AdamW"
        job_spec["executor"]["learning_rate"] = 1e-4
        job_spec["executor"]["epochs"] = 3
        job_spec["executor"]["batch_size"] = 1
        job_spec["executor"]["checkpointing"] = 2

        # Fetch dataset
        tensor_data = session.fetch(job_spec["executor"]["data"])
        tensor_data_path = os.path.join(work_dir, tensor_data[0]["path"])

        # Training loop
        print(os.listdir(work_dir))
        print("workdir:", work_dir)
        accelerator = Accelerator(project_dir=work_dir)
        device = accelerator.device
        print("Device", device)

        model, tokenizer = get_model(job_spec["executor"]["model"]["value"])
        optimizer = get_optimizer(
            job_spec["executor"]["optimizer"],
            job_spec["executor"]["learning_rate"],
            model.parameters(),
        )

        # Simple warmup + cooldown schedule
        milestone = int(job_spec["executor"]["epochs"] / 3)
        scheduler_warmup = torch.optim.lr_scheduler.LinearLR(
            optimizer, job_spec["executor"]["learning_rate"], 1e-3, milestone
        )
        scheduler_cool_down = torch.optim.lr_scheduler.LinearLR(
            optimizer,
            1e-3,
            job_spec["executor"]["learning_rate"],
            job_spec["executor"]["epochs"] - milestone,
        )
        scheduler = torch.optim.lr_scheduler.SequentialLR(
            optimizer,
            schedulers=[scheduler_warmup, scheduler_cool_down],
            milestones=[milestone],
        )
        data_loader = get_data_loader(tensor_data_path, job_spec["executor"]["batch_size"])

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(model, optimizer, data_loader, scheduler)

        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(job_spec["executor"]["updates"], "incoming") as receiver:
            updates_iter = iter(receiver)
            await_update = False

            for epoch in range(job_spec["executor"]["epochs"]):
                progress_bar = tqdm(
                    range(len(data_loader)),
                    disable=not accelerator.is_main_process,
                )

                if await_update:
                    print("Waiting for model update", flush=True)
                    try:
                        pointers = next(updates_iter)
                        if pointers:
                            try:
                                latest = pointers[-1] if isinstance(pointers, list) else pointers
                                parameters = (
                                    latest.get("parameters") if isinstance(latest.get("parameters"), dict) else None
                                )
                                rel_path = parameters.get("path") if parameters else latest.get("path")
                                if isinstance(rel_path, str):
                                    path = os.path.join(work_dir, rel_path)
                                    base_model = accelerator.unwrap_model(model)
                                    merge_models(base_model, path, 0.9)
                                    base_model.to(device)
                                    print("Weights updated from", rel_path, flush=True)
                            except Exception as e:
                                print(f"pointer handling error: {e}")
                    except StopIteration:
                        print("Receiver stream closed; no updates to merge.")
                    finally:
                        await_update = False

                for batch in training_dataloader:
                    optimizer.zero_grad()

                    # This is specific for training transformer models
                    # TODO: allow for other models
                    inputs = batch
                    outputs = model(input_ids=inputs, labels=inputs)
                    loss = outputs["loss"]
                    accelerator.backward(loss)
                    optimizer.step()
                    scheduler.step()
                    if accelerator.is_main_process:
                        progress_bar.update(1)

                if accelerator.is_main_process and epoch % job_spec["executor"]["checkpointing"] == 0:
                    # For testing purposes set to global!
                    file_name = f"{epoch}_global_weights.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Save unwrapped model to avoid accelerator wrappers interfering
                    save_model(accelerator.unwrap_model(model), result_path)
                    session.send(job_spec["executor"]["results"], file_name)

                    # Mark that before the next training epoch we must wait for an update
                    await_update = True

            print(f"Finished training of {job_spec['executor']['epochs']} epochs", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
