import argparse
import json
import os
import threading
import time
from collections.abc import Iterator
from contextlib import AbstractContextManager, contextmanager
from types import TracebackType
from queue import Empty, Queue
from typing import Any, Optional, Iterable, cast
import httpx
import torch
import torch.utils.data
from accelerate import Accelerator
from datasets import DatasetDict, load_dataset
from safetensors import safe_open
from safetensors.torch import save_model
from tqdm import tqdm
from transformers import GPT2Tokenizer, GPTNeoForCausalLM
from transformers.tokenization_utils import PreTrainedTokenizer


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


def get_data_loader(
    data_set_name: str, batch_size: int, tokenizer: PreTrainedTokenizer
) -> torch.utils.data.DataLoader[int]:
    # TODO: data needs to be provided by the task
    ds = cast(DatasetDict, load_dataset(data_set_name))["train"]
    train_ds, test_ds = ds.train_test_split(0.2, 0.8, True, seed=40).values()
    train_ds = CustomC4(train_ds["text"][:6], tokenizer, context_length=2048)
    return torch.utils.data.DataLoader(train_ds, batch_size=batch_size)


def merge_models(a: torch.nn.Module, weight_path: str, alpha: float) -> torch.nn.Module:
    # All weights need to be on CPU
    a.to("cpu")
    state_dict = a.state_dict()
    with safe_open(weight_path, framework="pt", device="cpu") as b:  # type: ignore
        for name in b.keys():  # noqa: SIM118
            state_dict[name] += alpha * (b.get_tensor(name) - state_dict[name])
    a.load_state_dict(state_dict)
    return a


class Session(AbstractContextManager["Session", None]):
    def __init__(self, socket_path: str) -> None:
        transport = httpx.HTTPTransport(uds=socket_path)
        self._client = httpx.Client(transport=transport)

    def __enter__(self) -> "Session":
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None
    ) -> None:
        self._client.close()

    @contextmanager
    def receive(self, req) -> Iterator["EventSource"]:
        with self._client.stream(
            "POST",
            "http://hypha/resources/receive",
            json=req,
            headers={"Accept": "text/event-stream"},
            timeout=None,  # block indefinitely for SSE updates
        ) as resp:
            yield EventSource(resp)


class EventSource:
    def __init__(self, response: httpx.Response) -> None:
        self._response = response

    @property
    def response(self) -> httpx.Response:
        return self._response

    def __iter__(self) -> Iterator[Any]:
        for line in self._response.iter_lines():
            fieldname, _, value = line.rstrip("\n").partition(":")

            if fieldname == "data":
                result = json.loads(value)

                yield result
            # Ignore other SSE fields (e.g., event:, id:, retry:)

# TODO: This needs to be generic and support user-provided datasets.
class CustomC4(torch.utils.data.Dataset[int]):
    def __init__(self, data: Any, tokenizer: PreTrainedTokenizer, context_length: int) -> None:
        tokenizer.pad_token = tokenizer.eos_token
        self.tokenizer = tokenizer
        self.tokenized_data = self._tokenize(data, context_length)

    def _tokenize(self, data: Any, context_length: int) -> list[int]:
        tokenized_inputs = self.tokenizer(
            data,
            return_tensors="pt",
            max_length=context_length,
            truncation=True,
            padding="max_length",
        )
        return cast(list[int], tokenized_inputs["input_ids"])

    def __len__(self) -> int:
        return len(self.tokenized_data)

    def __getitem__(self, idx: int) -> int:
        return self.tokenized_data[idx]


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
        job_spec["executor"]["dataset"] = "datablations/c4-filter-small"
        job_spec["executor"]["checkpointing"] = 2

        # Pre-create subscription for background receiver
        subscribe_req = {"receive": job_spec["executor"]["updates"], "out_dir": "incoming"}

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
        data_loader = get_data_loader(job_spec["executor"]["data"]["value"], job_spec["executor"]["batch_size"], tokenizer)

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(
            model, optimizer, data_loader, scheduler
        )

        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(subscribe_req) as receiver:
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
                                parameters = latest.get("parameters") if isinstance(latest.get("parameters"), dict) else None
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
                    try:
                        transport = httpx.HTTPTransport(uds=socket_path)
                        with httpx.Client(transport=transport, timeout=10) as client:
                            req = {"send": job_spec["executor"]["results"], "path": file_name}
                            resp = client.post("http://hypha/resources/send", json=req, timeout=30.0)
                            print("send:", resp.status_code, resp.text)
                    except Exception as e:
                        print(f"send error on {result_path}: {e}")

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
