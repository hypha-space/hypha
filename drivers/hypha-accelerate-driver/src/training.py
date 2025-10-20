import argparse
import json
import os
from collections.abc import Callable, Iterable, Iterator
from typing import Any, cast

import numpy as np
import torch
import torch.utils.data
from accelerate import Accelerator
from safetensors import safe_open
from safetensors.torch import save_file, save_model
from torch.nn import Module
from torch.optim import Optimizer
from torch.optim.lr_scheduler import LRScheduler
from tqdm import tqdm
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
    # Fetch dataset
    tensor_data = session.fetch(config["data"])
    tensor_data_path = os.path.join(work_dir, tensor_data[0]["path"])

    if "type" in config["model"] and config["model"]["type"] == "huggingface":
        session.fetch(config["model"])
    if "type" in config["config"]["preprocessor"] and config["config"]["preprocessor"]["type"] == "huggingface":
        session.fetch(config["config"]["preprocessor"])

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


def get_training_loop(
    config: dict[str, Any],
    data_iter: Iterator[dict[str, torch.Tensor]],
) -> Callable[  # type: ignore[type-arg]
    [
        Optimizer,
        Module,
        Accelerator,
        LRScheduler,
        tqdm,
    ],
    tuple[Optimizer, Module, Accelerator, LRScheduler, float],
]:
    if config["type"] == "causal-lm" or config["type"] == "vision-classification":

        def transformer_loop(
            optimizer: Optimizer,
            model: Module,
            accelerator: Accelerator,
            scheduler: LRScheduler,
            progress_bar: tqdm,  # type: ignore[type-arg]
        ) -> tuple[Optimizer, Module, Accelerator, LRScheduler, float]:
            losses = np.zeros(config["batches_per_local_epoch"])
            for i in range(config["batches_per_local_epoch"]):
                batch = next(data_iter)
                optimizer.zero_grad()
                outputs = model(**batch)
                loss = outputs["loss"]
                accelerator.backward(loss)
                optimizer.step()
                scheduler.step()
                losses[i] = loss.detach().cpu().numpy()
                if accelerator.is_main_process and progress_bar:
                    progress_bar.update(1)
            return optimizer, model, accelerator, scheduler, float(np.mean(losses))

        return transformer_loop

    if config["type"] == "torch":
        loss_fn = get_loss_fn(config["loss_fn"])

        def torch_loop(  # type: ignore[no-untyped-def]
            optimizer: Optimizer, model: Module, accelerator: Accelerator, scheduler: LRScheduler, progress_bar
        ) -> tuple[Optimizer, Module, Accelerator, LRScheduler, float]:
            losses = np.zeros(config["batches_per_local_epoch"])
            for i in range(config["batches_per_local_epoch"]):
                inputs, targets = next(data_iter)
                optimizer.zero_grad()
                outputs = model(inputs, targets)
                loss = loss_fn(outputs, targets)
                accelerator.backward(loss)
                optimizer.step()
                scheduler.step()
                losses[i] = loss.detach().cpu().numpy()
                if accelerator.is_main_process and progress_bar:
                    progress_bar.update(1)
            return optimizer, model, accelerator, scheduler, float(np.mean(losses))

        return torch_loop

    raise RuntimeError(f"Model type {config} not supported for training.")


def get_optimizer(optimizer: dict[str, Any], parameters: Iterable[torch.Tensor]) -> Optimizer:
    optimizer_type = optimizer["type"]
    if optimizer_type == "adam":
        lr = optimizer["learning_rate"]
        if optimizer.get("betas") and optimizer.get("epsilon"):
            return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"], eps=optimizer["epsilon"])
        if optimizer.get("betas"):
            return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"])
        if optimizer.get("epsilon"):
            return torch.optim.AdamW(parameters, lr=lr, eps=optimizer["epsilon"])
        return torch.optim.AdamW(parameters, lr=lr)
    else:
        raise RuntimeError(f"Optimizer {optimizer_type} doesn\bt exist.")


def get_data_loader(
    data_set_path: str,
    work_dir: str,
    model_type: str,
    preprocessor_file: str,
    batch_size: int,
) -> torch.utils.data.DataLoader:  # type: ignore[type-arg]
    if model_type == "causal-lm":
        with safe_open(data_set_path, framework="pt", device="cpu") as data:  # type: ignore
            ds = data.get_tensor(data.keys()[0])
            return torch.utils.data.DataLoader(ds, batch_size=batch_size)
    if model_type == "vision-classification":

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
            state_dict[name] = m.get_tensor(name) - g.get_tensor(name)
    return state_dict


def extract_gradients(state_dict: dict[str, torch.Tensor], previous_model_path: str) -> dict[str, torch.Tensor]:
    with safe_open(previous_model_path, framework="pt", device="cpu") as p:  # type: ignore[no-untyped-call]
        for name in p.keys():  # noqa: SIM118
            state_dict[name] -= p.get_tensor(name).to(state_dict[name].dtype)
    return state_dict


def dataset_wrapper(dataset: torch.utils.data.DataLoader) -> Iterator[dict[str, torch.Tensor]]:  # type: ignore[type-arg]
    def wrap() -> Iterator[dict[str, torch.Tensor]]:
        while True:
            yield from dataset

    return iter(wrap())


def main(socket_path: str, work_dir: str, job_json: str) -> None:  # noqa: PLR0915
    # Background receiver context that fills a queue with update pointers
    with Session(socket_path) as session:
        job_spec = json.loads(job_json)

        # simplify config retrival
        job_spec = job_spec["executor"]

        # ensure that type is `parameter-server`
        assert job_spec["type"] == "diloco-transformer"

        print(json.dumps(job_spec))

        # Training loop
        accelerator = Accelerator(project_dir=work_dir)

        tensor_data_path = prepare_files(job_spec, session, work_dir)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        print(os.listdir(local_fetch_path))
        model = get_model(local_fetch_path, job_spec["config"]["type"])
        optimizer = get_optimizer(
            job_spec["config"]["optimizer"],
            model.parameters(),
        )

        if not job_spec["config"]["scheduler"] or not job_spec["config"]["scheduler"]["type"]:
            scheduler = get_constant_schedule(optimizer)
        else:
            scheduler_args = job_spec["config"]["scheduler"]
            scheduler_type = scheduler_args["type"]
            del scheduler_args["type"]
            scheduler = get_scheduler(scheduler_type, scheduler_args, optimizer)

        data_loader = get_data_loader(
            tensor_data_path,
            local_fetch_path,
            job_spec["config"]["type"],
            job_spec["config"]["preprocessor"]["filenames"][0],
            job_spec["config"]["batch_size"],
        )

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, "0_global_weights.pt")
        save_model(model, previous_model_path)

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(model, optimizer, data_loader, scheduler)
        training_data_iter = dataset_wrapper(training_dataloader)
        run_epoch = get_training_loop(job_spec["config"], training_data_iter)

        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(job_spec["updates"], "incoming") as receiver:
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
                                    # Load new model with outer gradients
                                    model.load_state_dict(merge_models(previous_model_path, path))
                                    # over write previous model
                                    save_model(model, previous_model_path)
                                    model = accelerator.prepare(model)
                                    print("Weights updated from", rel_path, flush=True)
                            except Exception as e:
                                print(f"pointer handling error: {e}")
                    except StopIteration:
                        print("Receiver stream closed; no updates to merge.")
                    finally:
                        await_update = False

                optimizer, model, accelerator, scheduler, loss = run_epoch(
                    optimizer, model, accelerator, scheduler, progress_bar
                )

                if accelerator.is_main_process and epoch % job_spec["config"]["checkpointing"] == 0:
                    # For testing purposes set to global!
                    file_name = f"{epoch}_local_gradients.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Save unwrapped model to avoid accelerator wrappers interfering
                    #
                    model = accelerator.unwrap_model(model)
                    # All weights need to be on CPU
                    model.to("cpu")

                    save_file(extract_gradients(model.state_dict(), previous_model_path), result_path)
                    session.send(job_spec["results"], file_name)

                    # Mark that before the next training epoch we must wait for an update
                    await_update = True

                # session.send_status(job_spec["status"], {"local_epoch": epoch, "loss": loss})

            print(f"Finished training of {job_spec['config']['epochs']} epochs", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
