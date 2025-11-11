import argparse
import datetime
import json
import os
from collections.abc import Iterable, Iterator
from typing import Any, cast

import numpy as np
import torch
import torch.utils.data
from accelerate import Accelerator
from safetensors import safe_open
from safetensors.torch import save_file, save_model
from torch.nn import Module
from torch.optim import Optimizer

# from torch.optim.lr_scheduler import LRScheduler
from transformers import AutoImageProcessor, AutoModelForCausalLM, AutoModelForImageClassification
from transformers.optimization import (
    get_constant_schedule,
    get_cosine_schedule_with_warmup,
    get_linear_schedule_with_warmup,
    get_wsd_schedule,
)

from api import Session

FETCH_PATH = "artifacts"


def prepare_files(config: dict[str, Any], session: Session, work_dir: str) -> str:
    tensor_data = session.fetch(config["data"])
    tensor_data_path = os.path.join(work_dir, tensor_data[0]["path"])

    session.fetch(config["model"]["artifact"])

    preprocessor = config.get("preprocessor")
    if preprocessor:
        session.fetch(preprocessor)

    return tensor_data_path


def get_model(model_config: str, model_type: str) -> Module:
    # Initializing a model from a Hugging Face configuration
    if model_type == "causal-lm":
        return AutoModelForCausalLM.from_pretrained(model_config)
    if model_type == "vision-classification":
        return cast(Module, AutoModelForImageClassification.from_pretrained(model_config))
    # if model_type == "Torch":
    # ...
    raise RuntimeError(f"Model type {model_type} not supported.")


def get_preprocessor(preprocessor_file: str, model_type: str) -> Any:
    if model_type == "vision-classification":
        return AutoImageProcessor.from_pretrained(preprocessor_file)  # type: ignore[no-untyped-call]
    raise RuntimeError(f"Pre-Processor for model type {model_type} not found")


def get_adam(optimizer: dict[str, Any], parameters: Iterable[torch.Tensor]) -> Optimizer:
    lr = optimizer["learning-rate"]
    if optimizer.get("betas") and optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"], eps=optimizer["epsilon"])
    if optimizer.get("betas"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"])
    if optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, eps=optimizer["epsilon"])
    return torch.optim.AdamW(parameters, lr=lr)


def get_data_loader(
    data_set_path: str,
    work_dir: str,
    model_type: str,
    preprocessor_file: str | None,
    batch_size: int,
) -> torch.utils.data.DataLoader:  # type: ignore[type-arg]
    if model_type == "causal-lm":
        with safe_open(data_set_path, framework="pt", device="cpu") as data:  # type: ignore
            ds = data.get_tensor(data.keys()[0])
            return torch.utils.data.DataLoader(ds, batch_size=batch_size)
    if model_type == "vision-classification":
        if not preprocessor_file:
            raise RuntimeError("Vision classification requires a preprocessor artifact")

        class VisionDs(torch.utils.data.IterableDataset):  # type: ignore[type-arg]
            def __init__(self, data_set_path: str, work_dir: str):
                super(VisionDs).__init__()  # type: ignore[misc]
                self.data_set_path = data_set_path
                self.processor = get_preprocessor(f"{work_dir}/{preprocessor_file}", model_type)

            def __iter__(self):  # type: ignore[no-untyped-def]
                with safe_open(self.data_set_path, framework="pt", device="cpu") as data:  # type: ignore
                    processed = self.processor(data.get_tensor("inputs"))
                    for pixel, label in zip(iter(processed["pixel_values"]), iter(data.get_tensor("targets"))):
                        yield ({"pixel_values": pixel, "labels": label})

        return torch.utils.data.DataLoader(VisionDs(data_set_path, work_dir), batch_size=batch_size)
    raise RuntimeError(f"Dataset for {model_type} not supported")


def get_loss_fn(loss_fn: str) -> Module:
    if loss_fn == "l1":
        return torch.nn.L1Loss()
    if loss_fn == "mse":
        return torch.nn.MSELoss()
    if loss_fn == "cross-entropy":
        return torch.nn.CrossEntropyLoss()
    if loss_fn == "bce-with-logits":
        return torch.nn.BCEWithLogitsLoss()
    if loss_fn == "kl-div":
        return torch.nn.KLDivLoss()
    raise RuntimeError(f"Loss Function {loss_fn} not supported.")


def get_scheduler(type: str, args: dict[str, float], optmizer: Optimizer) -> torch.optim.lr_scheduler.LambdaLR:
    if type == "cosine-with-warmup":
        return get_cosine_schedule_with_warmup(optmizer, int(args["warmup_steps"]), int(args["training_steps"]))  # type: ignore[no-any-return]
    if type == "linear-with-warmup":
        return get_linear_schedule_with_warmup(optmizer, args["warmup_steps"], args["training_steps"])  # type: ignore[no-any-return, no-untyped-call]
    if type == "wsd":
        return get_wsd_schedule(optmizer, int(args["warmup_steps"]), int(args["decay_step"]))  # type: ignore[no-any-return]
    raise RuntimeError(f"Learning rate Scheduler {type} not supported")


def merge_models(old_model: str, weight_path: str) -> dict[str, torch.Tensor]:
    state_dict: dict[str, torch.Tensor] = {}
    with (
        safe_open(weight_path, framework="pt", device="cpu") as g,  # type: ignore[no-untyped-call]
        safe_open(old_model, framework="pt", device="cpu") as m,  # type: ignore[no-untyped-call]
    ):
        for name in m.keys():  # noqa: SIM118
            # state_dict[name] += (alpha * (b.get_tensor(name) - state_dict[name])).to(state_dict[name].dtype)
            # The gradient from 'extract_gradients' is negative. Thus, add instead of subtract.
            state_dict[name] = m.get_tensor(name) + g.get_tensor(name)
    return state_dict


def extract_gradients(state_dict: dict[str, torch.Tensor], previous_model_path: str) -> dict[str, torch.Tensor]:
    with safe_open(previous_model_path, framework="pt", device="cpu") as p:  # type: ignore[no-untyped-call]
        for name in p.keys():  # noqa: SIM118
            # This results in \theta_{t} - \theta_{t-1} = -\nabla
            state_dict[name] -= p.get_tensor(name).to(state_dict[name].dtype)
    return state_dict


def dataset_wrapper(dataset: torch.utils.data.DataLoader) -> Iterator[dict[str, torch.Tensor]]:  # type: ignore[type-arg]
    def wrap() -> Iterator[dict[str, torch.Tensor]]:
        while True:
            yield from dataset

    return iter(wrap())


def main(socket_path: str, work_dir: str, job_json: str) -> None:  # noqa: PLR0915, PLR0912
    # Background receiver context that fills a queue with update pointers
    with Session(socket_path) as session:
        job_spec = json.loads(job_json)

        executor = job_spec["executor"]
        assert executor["class"] == "train"
        config = executor["config"]

        print(json.dumps(executor))

        accelerator = Accelerator(project_dir=work_dir)

        tensor_data_path = prepare_files(config, session, work_dir)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        print(os.listdir(local_fetch_path))
        model_type = config["model"]["task"]
        model = get_model(local_fetch_path, model_type)
        optimizer = get_adam(config["optimizer"], model.parameters())
        epoch_counter = 0

        scheduler_config = config.get("scheduler")
        if not scheduler_config or not scheduler_config.get("type"):
            scheduler = get_constant_schedule(optimizer)
        else:
            scheduler_type = scheduler_config["type"]
            scheduler_args = {k: v for k, v in scheduler_config.items() if k != "type"}
            scheduler = get_scheduler(scheduler_type, scheduler_args, optimizer)

        preprocessor = config.get("preprocessor")
        preprocessor_file: str | None = None
        if preprocessor:
            filenames = preprocessor.get("filenames")
            if filenames:
                preprocessor_file = filenames[0]

        data_loader = get_data_loader(
            tensor_data_path,
            local_fetch_path,
            model_type,
            preprocessor_file,
            config["batch_size"],
        )

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, "0_global_weights.pt")
        save_model(model, previous_model_path)

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(model, optimizer, data_loader, scheduler)
        training_data_iter = dataset_wrapper(training_dataloader)

        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(config["results"], "incoming") as receiver:
            updates_iter = iter(receiver)
            await_update = False

            while True:
                start_time = datetime.datetime.now()

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
                                    # Load new model with outer gradients
                                    model.load_state_dict(merge_models(previous_model_path, path))
                                    # over write previous model
                                    save_model(model, previous_model_path)
                                    model = accelerator.prepare(model)
                                    print("Weights updated from", rel_path, flush=True)
                                    response = session.send_status("update-received")
                                    if response["type"] == "Done":
                                        print("Training finished")
                                        break
                            except Exception as e:
                                print(f"pointer handling error: {e}")
                    except StopIteration:
                        print("Receiver stream closed; no updates to merge.")
                    finally:
                        await_update = False

                losses = []
                counter = -1
                while counter != 0:
                    batch = next(training_data_iter)
                    optimizer.zero_grad()
                    outputs = model(**batch)
                    loss = outputs if isinstance(outputs, torch.Tensor) else outputs["loss"]
                    accelerator.backward(loss)
                    optimizer.step()
                    scheduler.step()
                    losses.append(loss.detach().cpu().numpy())
                    if accelerator.is_main_process:
                        round_time = start_time - datetime.datetime.now()
                        response = session.send_status(
                            {
                                "status": {
                                    "batch_size": next(iter(batch.values())).shape[0],
                                    "round_time": int(round_time.microseconds / 1000),
                                }
                            }
                        )
                        if response["type"] == "ScheduleUpdate":
                            counter = response["counter"]
                        else:
                            counter -= 1
                        start_time = datetime.datetime.now()

                if accelerator.is_main_process:
                    session.send_status("update")
                    # For testing purposes set to global!
                    file_name = f"{epoch_counter}_local_gradients.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Save unwrapped model to avoid accelerator wrappers interfering
                    model = accelerator.unwrap_model(model)
                    # All weights need to be on CPU
                    model.to("cpu")

                    save_file(extract_gradients(model.state_dict(), previous_model_path), result_path)
                    session.send_resource(config["updates"], file_name)

                    # Mark that before the next training epoch we must wait for an update
                    await_update = True

                session.send_status({"metrics": {"round": epoch_counter, "metrics": {"loss": float(np.mean(losses))}}})
                epoch_counter += 1

            print(f"Finished training of {epoch_counter - 1} epochs", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
