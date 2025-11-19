from collections.abc import Iterator
from typing import Any

import torch
from safetensors.torch import load_file
from torch.utils.data import DataLoader, IterableDataset


class IterableStreamDataSet(IterableDataset):  # type: ignore[type-arg]
    def __init__(
        self,
        data_file_iter: Iterator[str],
        model_inputs: list[str],
        processor_inputs: list[str],
        preprocessor: Any | None,
    ) -> None:
        super(IterableStreamDataSet).__init__()  # type: ignore[misc]
        self.data_iter = data_file_iter
        self.model_inputs = model_inputs
        self.processor_inputs = processor_inputs
        self.processor = preprocessor

    def __iter__(self):  # type: ignore[no-untyped-def]
        for path in self.data_iter:
            data = load_file(path, device="cpu")
            processed = (
                {**self.processor(**{k: data.pop(k) for k in self.processor_inputs}), **data}
                if self.processor
                else data
            )

            for values in zip(*(processed[k] for k in self.model_inputs)):
                yield ({k: v for k, v in zip(self.model_inputs, values)})


def dataset_wrapper(dataset: DataLoader) -> Iterator[dict[str, torch.Tensor]]:  # type: ignore[type-arg]
    def wrap() -> Iterator[dict[str, torch.Tensor]]:
        while True:
            yield from dataset

    return iter(wrap())
