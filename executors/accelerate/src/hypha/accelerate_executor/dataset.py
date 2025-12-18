from collections.abc import Iterator
from typing import Any

import torch
from safetensors.torch import load
from snappy import uncompress
from torch.utils.data import DataLoader, IterableDataset


class IterableStreamDataSet(IterableDataset):  # type: ignore[type-arg]
    def __init__(
        self,
        data_file_iter: Iterator[str],
        batch_size: int,
        model_inputs: list[str],
        processor_inputs: list[str],
        preprocessor: Any | None,
    ) -> None:
        super(IterableStreamDataSet).__init__()  # type: ignore[misc]
        self.data_iter = data_file_iter
        self.batch_size = batch_size
        self.model_inputs = model_inputs
        self.processor_inputs = processor_inputs
        self.processor = preprocessor

    def __iter__(self):  # type: ignore[no-untyped-def]
        # Don't need sharding each call to data_iter returns a unique instance

        # Holds the "remainder" from the previous file
        buffer = None
        for path in self.data_iter:
            with open(path, "rb") as file:
                raw_bytes = uncompress(file.read())
            data = load(raw_bytes)
            if self.processor:
                processed = {
                    **{k: v[0] for k, v in self.processor(**{k: data.pop(k) for k in self.processor_inputs}).items()},
                    **data,
                }
            else:
                processed = data

            if buffer is None:
                buffer = {k: v for k, v in processed.items() if k in self.model_inputs}
            else:
                for k in buffer:
                    buffer[k] = torch.cat([buffer[k], processed[k]], dim=0)

            primary_key = list(buffer.keys())[0]
            current_len = buffer[primary_key].shape[0]

            while current_len >= self.batch_size:
                batch = {k: buffer[k][: self.batch_size] for k in buffer}

                for k in buffer:
                    buffer[k] = buffer[k][self.batch_size :]

                current_len -= self.batch_size

                yield batch

        # Drop the very last partial batch of the epoch to avoid recompilation
        pass


def dataset_wrapper(dataset: DataLoader) -> Iterator[dict[str, torch.Tensor]]:  # type: ignore[type-arg]
    def wrap() -> Iterator[dict[str, torch.Tensor]]:
        while True:
            yield from dataset

    return iter(wrap())
