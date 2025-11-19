import os
from collections.abc import Iterable, Iterator
from typing import Any

import torch
import torch.utils.data
from safetensors import safe_open
from torch.nn import Module
from torch.optim import Optimizer

# from torch.optim.lr_scheduler import LRScheduler
from transformers import (
    AutoFeatureExtractor,
    AutoImageProcessor,
    AutoProcessor,
    AutoTokenizer,
    AutoVideoProcessor,
)
from transformers.optimization import (
    get_constant_schedule,
    get_cosine_schedule_with_warmup,
    get_linear_schedule_with_warmup,
    get_wsd_schedule,
)

from .api import Session


def prepare_files(config: dict[str, Any], session: Session) -> None:
    session.fetch(config["model"]["artifact"])

    preprocessor = config.get("preprocessor")
    if preprocessor:
        session.fetch(preprocessor["artifact"])


def get_preprocessor(preprocessor_config: dict[str, Any], local_fetch_path: str) -> Any:
    if preprocessor_config:
        filenames = preprocessor_config["artifact"]["filenames"]
        if filenames:
            type = preprocessor_config["task"]
            file_path = f"{local_fetch_path}/{filenames[0]}"
            if type == "auto":
                return AutoProcessor.from_pretrained(file_path, trust_remote_code=True)  # type: ignore[no-untyped-call]
            if type == "feature":
                return AutoFeatureExtractor.from_pretrained(file_path, trust_remote_code=True)  # type: ignore[no-untyped-call]
            if type == "image":
                return AutoImageProcessor.from_pretrained(file_path, trust_remote_code=True)  # type: ignore[no-untyped-call]
            if type == "tokenizer":
                return AutoTokenizer.from_pretrained(file_path, trust_remote_code=True)  # type: ignore[no-untyped-call]
            if type == "video":
                return AutoVideoProcessor.from_pretrained(file_path, trust_remote_code=True)  # type: ignore[no-untyped-call]
            raise RuntimeError(f"Pre-Processor of type {type} not found")


def get_adam(optimizer: dict[str, Any], parameters: Iterable[torch.Tensor]) -> Optimizer:
    lr = optimizer["learning-rate"]
    if optimizer.get("betas") and optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"], eps=optimizer["epsilon"])
    if optimizer.get("betas"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"])
    if optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, eps=optimizer["epsilon"])
    return torch.optim.AdamW(parameters, lr=lr)


def fetch_data(session: Session, data: str, work_dir: str) -> Iterator[str]:
    def wrap() -> Iterator[str]:
        while True:
            tensor_data = session.fetch(data)
            yield os.path.join(work_dir, tensor_data[0]["path"])

    return iter(wrap())


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


def get_scheduler(scheduler_config: dict[str, Any], optimizer: Optimizer) -> torch.optim.lr_scheduler.LambdaLR:
    if not scheduler_config or not scheduler_config.get("type"):
        return get_constant_schedule(optimizer)  # type: ignore[no-any-return]
    else:
        scheduler_type = scheduler_config["type"]
        warmup_steps = int(scheduler_config["warmup_steps"])
        if scheduler_type == "cosine-with-warmup":
            return get_cosine_schedule_with_warmup(optimizer, warmup_steps, int(scheduler_config["training_steps"]))  # type: ignore[no-any-return]
        if scheduler_type == "linear-with-warmup":
            return get_linear_schedule_with_warmup(optimizer, warmup_steps, int(scheduler_config["training_steps"]))  # type: ignore[no-any-return, no-untyped-call]
        if scheduler_type == "wsd":
            return get_wsd_schedule(optimizer, warmup_steps, int(scheduler_config["decay_step"]))  # type: ignore[no-any-return]
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
