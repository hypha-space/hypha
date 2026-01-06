from collections.abc import Iterable
from typing import Any

import torch
import torch.utils.data
from safetensors import safe_open
from torch.nn import Module
from torch.optim import Optimizer

from .api import Session


def prepare_files(config: dict[str, Any], session: Session) -> None:
    session.fetch(config["model"]["artifact"])


def get_adam(optimizer: dict[str, Any], parameters: Iterable[torch.Tensor]) -> Optimizer:
    lr = optimizer["learning-rate"]
    if optimizer.get("betas") and optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"], eps=optimizer["epsilon"])
    if optimizer.get("betas"):
        return torch.optim.AdamW(parameters, lr=lr, betas=optimizer["betas"])
    if optimizer.get("epsilon"):
        return torch.optim.AdamW(parameters, lr=lr, eps=optimizer["epsilon"])
    return torch.optim.AdamW(parameters, lr=lr)


def merge_models(old_model: str, weight_path: str) -> dict[str, torch.Tensor]:
    state_dict: dict[str, torch.Tensor] = {}
    with (
        safe_open(weight_path, framework="pt", device="cpu") as g,  # type: ignore[no-untyped-call]
        safe_open(old_model, framework="pt", device="cpu") as m,  # type: ignore[no-untyped-call]
    ):
        for name in m.keys():  # noqa: SIM118
            # state_dict[name] += (alpha * (b.get_tensor(name) - state_dict[name])).to(state_dict[name].dtype)
            # The gradient from 'extract_gradients' is negative. Thus, add instead of subtract.
            model_weight = m.get_tensor(name)
            state_dict[name] = model_weight + g.get_tensor(name).to(model_weight.dtype)
    return state_dict


def extract_gradients(
    state_dict: dict[str, torch.Tensor], previous_model_path: str, weight: float = 1
) -> dict[str, torch.Tensor]:
    with safe_open(previous_model_path, framework="pt", device="cpu") as p:  # type: ignore[no-untyped-call]
        for name in p.keys():  # noqa: SIM118
            # This results in \theta_{t} - \theta_{t-1} = -\nabla
            state_dict[name] -= p.get_tensor(name).to(state_dict[name].dtype)
            # We need to filter out the `num_batches_tracked` from BatchNormalization.
            # Weighting will fail here and its ok to just sum them up.
            if weight != 1 and "num_batches_tracked" not in name:
                state_dict[name] *= weight
            # Downcast to bfloat16 for compressed gradients.
            if torch.is_floating_point(state_dict[name]):
                state_dict[name] = state_dict[name].bfloat16()
    return state_dict
