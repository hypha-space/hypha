import os
from collections.abc import Iterator
from typing import Any

import torch
from safetensors.torch import load
from snappy import uncompress
from torch.utils.data import IterableDataset

from .api import fetch
from .utils import get_preprocessor


class IterableStreamDataSet(IterableDataset):  # type: ignore[type-arg]
    def __init__(
        self, socket_path: str, work_dir: str, fetch_path: str, batch_size: int, config: dict[str, Any]
    ) -> None:
        super(IterableStreamDataSet).__init__()  # type: ignore[misc]
        self.socket_path = socket_path
        self.work_dir = work_dir
        self.fetch_path = fetch_path
        self.config = config
        self.batch_size = batch_size
        self.model_inputs = (config["model"]["input-names"],)
        self.processor_config = config.get("preprocessor", {})
        self.processor_inputs = self.processor_config.get("input-names", [])

    def __iter__(self):  # type: ignore[no-untyped-def]
        # Don't need sharding each call to data_iter returns a unique instance
        socket_path = self.socket_path
        data_config = self.config["data"]
        work_dir = self.work_dir

        def wrap() -> Iterator[str]:
            while True:
                tensor_data = fetch(socket_path, data_config)
                yield os.path.join(work_dir, tensor_data[0]["path"])

        data_iter = iter(wrap())

        processor = None
        if self.processor_config:
            processor = get_preprocessor(self.processor_config, self.fetch_path)

        # Holds the "remainder" from the previous file
        buffer = None
        for path in data_iter:
            with open(path, "rb") as file:
                raw_bytes = uncompress(file.read())
            data = load(raw_bytes)
            if processor:
                processed = {
                    **{k: v[0] for k, v in processor(**{k: data.pop(k) for k in self.processor_inputs}).items()},
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
