import argparse
import json
import os
from collections.abc import Iterable, Iterator
from contextlib import AbstractContextManager, contextmanager
from types import TracebackType
from typing import Any
import tarfile

import httpx
import torch
import torch.utils.data
from accelerate import Accelerator
from datasets.data_files import Callable
from safetensors import safe_open
from safetensors.torch import load_file, save_model, load
from tqdm import tqdm
from transformers import AutoModelForCausalLM, AutoModelForImageClassification, AutoImageProcessor
from transformers.optimization import (
    get_constant_schedule,
    get_cosine_schedule_with_warmup,
    get_linear_schedule_with_warmup,
    get_wsd_schedule,
)

from api import Session


def prepare_files(config: dict[str, Any], session):
    if "type" in config["model"] and config["model"]["type"] == "huggingface":
        session.fetch(config["model"])
    if "type" in config["data"] and config["data"]["type"] == "huggingface":
        session.fetch(config["data"])
    if "type" in config["config"]["preprocessor"] and config["config"]["preprocessor"]["type"] == "huggingface":
        session.fetch(config["config"]["preprocessor"])


def get_model(model_config: str, model_type: str) -> torch.nn.Module:
    # Initializing a model from a Hugging Face configuration
    if model_type == "CausalLm":
        return AutoModelForCausalLM.from_pretrained(model_config)
    if model_type == "VisionClassification":
        return AutoModelForImageClassification.from_pretrained(model_config)
    # if model_type == "Torch":
    # ...
    raise RuntimeError(f"Model type {model_type} not supported.")
    
    
def get_preprocessor(preprocessor_file: str, model_type: str):
    if model_type == "VisionClassification":
        return AutoImageProcessor.from_pretrained(preprocessor_file)


def get_training_loop(
    config: dict[str, Any], work_dir, data_iter,
) -> Callable[
    [
        torch.optim.Optimizer,
        torch.nn.Module,
        Accelerator,
        torch.optim.lr_scheduler.LRScheduler,
        tqdm,
    ],
    tuple[torch.optim.Optimizer, torch.nn.Module, Accelerator, torch.optim.lr_scheduler.LRScheduler],
]:
    if config["type"] == "CausalLm":
        def causal_lm_loop(
            optimizer, model, accelerator, scheduler, progress_bar
        ) -> tuple[torch.optim.Optimizer, torch.nn.Module, Accelerator, torch.optim.lr_scheduler.LRScheduler]:
            for batch in data_iter:
                optimizer.zero_grad()
                inputs = batch[0]
                outputs = model(input_ids=inputs, labels=inputs)
                loss = outputs["loss"]
                accelerator.backward(loss)
                optimizer.step()
                scheduler.step()
                if accelerator.is_main_process and progress_bar:
                    progress_bar.update(1)
            return optimizer, model, accelerator, scheduler

        return causal_lm_loop
        
    if config["type"] == "VisionClassification":
        def vision_classification_loop(
            optimizer, model, accelerator, scheduler, progress_bar
        ) -> tuple[torch.optim.Optimizer, torch.nn.Module, Accelerator, torch.optim.lr_scheduler.LRScheduler]:
            for _ in range(config["batches_per_local_epoch"]):
                batch = next(data_iter)
                print(batch["pixel_values"].shape)
                optimizer.zero_grad()
                outputs = model(**batch)
                loss = outputs["loss"]
                accelerator.backward(loss)
                optimizer.step()
                scheduler.step()
                if accelerator.is_main_process and progress_bar:
                    progress_bar.update(1)
            return optimizer, model, accelerator, scheduler

        return vision_classification_loop

    if config["type"] == "Torch":
        loss_fn = get_loss_fn(config["loss_fn"])

        def torch_loop(
            optimizer, model, accelerator, scheduler, progress_bar
        ) -> tuple[torch.optim.Optimizer, torch.nn.Module, Accelerator, torch.optim.lr_scheduler.LRScheduler]:
            for inputs, targets in data_iter:
                optimizer.zero_grad()
                outputs = model(inputs, targets)
                loss = loss_fn(outputs, targets)
                accelerator.backward(loss)
                optimizer.step()
                scheduler.step()
                if accelerator.is_main_process and progress_bar:
                        progress_bar.update(1)
            return optimizer, model, accelerator, scheduler

        return torch_loop

    raise RuntimeError(f"Model type {config} not supported for training.")


def get_optimizer(optimizer: dict[str, Any], parameters: Iterable[torch.Tensor]) -> torch.optim.Optimizer:
    if optimizer["type"] == "Adam":
        lr = optimizer.get("learning_rate")
        if optimizer.get("betas") and optimizer.get("epsilon"):
            return torch.optim.AdamW(parameters, lr=lr, betas=optimizer.get("betas"), eps=optimizer.get("epsilon"))
        if optimizer.get("betas"):
            return torch.optim.AdamW(parameters, lr=lr, betas=optimizer.get("betas"))
        if optimizer.get("epsilon"):
            return torch.optim.AdamW(parameters, lr=lr, eps=optimizer.get("epsilon"))
        return torch.optim.AdamW(parameters, lr=lr)
    else:
        raise RuntimeError(f"Optimizer {optimizer['type']} doesn't exist.")


def get_data_loader(work_dir: str, data_config: dict[str, str], model_type: str, preprocessor_file, batch_size) -> torch.utils.data.DataLoader:
    if model_type == "CausalLm":
        ds = load_file(f"{work_dir}/{data_config['filenames'][0]}")
        data_loader = torch.utils.data.DataLoader(
            torch.utils.data.TensorDataset(ds["inputs"][:6]), batch_size=batch_size
        )
        return data_loader
    if model_type == "VisionClassification":
        
        class VisionDs(torch.utils.data.IterableDataset):
            def __init__(self, work_dir, files):
                super(VisionDs).__init__()
                self.files = [f"{work_dir}/{file}" for file in files]
                self.processor = get_preprocessor(f"{work_dir}/{preprocessor_file}", model_type)
            
            def __iter__(self):
                for file in self.files:
                    with tarfile.open(file, "r:gz") as tar:
                        for f in tar.getnames():
                            tensors = load(tar.extractfile(f).read())
                            processed = self.processor(tensors["inputs"])
                            for p,l in zip(iter(processed["pixel_values"]), iter(tensors["targets"])):
                                yield({"pixel_values":p, "labels":l})
            
            # TODO: This shouldn't be needed. IterableDataSet don't have a length..
            # def __len__(self):
            #     cnt = 0
            #     for v in self.__iter__():
            #         cnt += 1
            #     return cnt
        
        return torch.utils.data.DataLoader(VisionDs(work_dir, data_config['filenames']), batch_size=batch_size)

    raise RuntimeError(f"Dataset for {model_type} not supported")


def get_loss_fn(loss_fn: str) -> torch.nn.Module:
    if loss_fn == "L1":
        return torch.nn.L1()
    if loss_fn == "MSE":
        return torch.nn.MSE()
    if loss_fn == "CrossEntropyLoss":
        return torch.nn.CrossEntropyLoss()
    if loss_fn == "BCEWithLogits":
        return torch.nn.BCEWithLogits()
    if loss_fn == "KLDivLoss":
        return torch.nn.KLDivLoss()
    raise RuntimeError(f"Loss Function {loss_fn} not supported.")


def get_scheduler(scheduler: dict[str, Any], optmizer: torch.optim.Optimizer) -> torch.optim.lr_scheduler.LRScheduler:
    if not scheduler or not scheduler["type"]:
        return get_constant_schedule(optmizer)
    if scheduler["type"] == "CosineWithWarmup":
        return get_cosine_schedule_with_warmup(
            optmizer, int(scheduler.get("warmup_steps")), int(scheduler.get("training_steps"))
        )
    if scheduler["type"] == "LinearWithWarmup":
        return get_linear_schedule_with_warmup(optmizer, scheduler.get("warmup_steps"), scheduler.get("training_steps"))
    if scheduler["type"] == "WSD":
        return get_wsd_schedule(optmizer, int(scheduler.get("warmup_steps")), int(scheduler.get("decay_steps")))
    raise RuntimeError(f"Learning rate Scheduler {scheduler['type']} not supported")


def merge_models(a: torch.nn.Module, weight_path: str, alpha: float) -> torch.nn.Module:
    # All weights need to be on CPU
    a.to("cpu")
    state_dict = a.state_dict()
    with safe_open(weight_path, framework="pt", device="cpu") as b:  # type: ignore
        for name in b.keys():  # noqa: SIM118
            state_dict[name] += (alpha * (b.get_tensor(name) - state_dict[name])).to(state_dict[name].dtype)          
    a.load_state_dict(state_dict)
    return a
    
    
def dataset_wrapper(dataset):
    def wrap():
        while True:
            for batch in dataset:
                yield(batch)
    
    return iter(wrap())


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

    def fetch(self, req) -> httpx.Response:
        with self._client.stream(
            "POST",
            "http://hypha/resources/fetch",
            json=req,
            timeout=None,  # block indefinitely for SSE updates
        ) as resp:
            return resp


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


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    # Background receiver context that fills a queue with update pointers
    with Session(socket_path) as session:
        job_spec = json.loads(job_json)

        # ensure that type is `parameter-server`
        assert job_spec["executor"]["type"] == "diloco-transformer"

        # simplify config retrival
        job_spec = job_spec["executor"]
        print(json.dumps(job_spec))

        # Pre-create subscription for background receiver
        subscribe_req = {"receive": job_spec["updates"], "out_dir": "incoming"}

        print("workdir:", work_dir)
        accelerator = Accelerator(project_dir=work_dir)

        prepare_files(job_spec, session)
        print(os.listdir(f"{work_dir}/artifacts"))
        model = get_model(f"{work_dir}/artifacts", job_spec["config"]["type"])
        optimizer = get_optimizer(
            job_spec["config"]["optimizer"],
            model.parameters(),
        )

        scheduler = get_scheduler(job_spec["config"]["scheduler"], optimizer)

        # print(os.listdir(f"{work_dir}/artifacts"))
        data_loader = get_data_loader(
            f"{work_dir}/artifacts",
            job_spec["data"],
            job_spec["config"]["type"],
            job_spec["config"]["preprocessor"]["filenames"][0],
            job_spec["config"]["batch_size"]
        )

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(model, optimizer, data_loader, scheduler)
        training_data_iter = dataset_wrapper(training_dataloader)
        run_epoch = get_training_loop(job_spec["config"], f"{work_dir}/artifacts", training_data_iter)
        
        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(job_spec["executor"]["updates"], "incoming") as receiver:
            updates_iter = iter(receiver)
            await_update = False

            for epoch in range(job_spec["config"]["epochs"]):
                progress_bar = tqdm(
                    range(job_spec["config"]["batches_per_local_epoch"]),
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
                                    base_model = merge_models(base_model, path, 0.9)
                                    model = accelerator.prepare(base_model)
                                    print("Weights updated from", rel_path, flush=True)
                            except Exception as e:
                                print(f"pointer handling error: {e}")
                    except StopIteration:
                        print("Receiver stream closed; no updates to merge.")
                    finally:
                        await_update = False

                optimizer, model, accelerator, scheduler = run_epoch(
                    optimizer, model, accelerator, scheduler, progress_bar
                )

                if accelerator.is_main_process and epoch % job_spec["config"]["checkpointing"] == 0:
                    # For testing purposes set to global!
                    file_name = f"{epoch}_global_weights.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Save unwrapped model to avoid accelerator wrappers interfering
                    save_model(accelerator.unwrap_model(model), result_path)
                    try:
                        transport = httpx.HTTPTransport(uds=socket_path)

                        with httpx.Client(transport=transport) as client:
                            req = {"send": job_spec["results"], "path": file_name}
                            client.post("http://hypha/resources/send", json=req).raise_for_status()

                    except Exception as e:
                        print(f"send error on {result_path}: {e}")

                    # Mark that before the next training epoch we must wait for an update
                    await_update = True

            print(f"Finished training of {job_spec['config']['epochs']} epochs", flush=True)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
