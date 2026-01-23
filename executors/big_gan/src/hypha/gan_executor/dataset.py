import logging
import os
from typing import Any

import httpx
import torch
from safetensors.torch import load
from snappy import uncompress
from torch.utils.data import IterableDataset

from .utils import get_preprocessor

logger = logging.getLogger(__name__)


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

        self.model_inputs = set(config["model"]["input-names"])
        self.processor_config = config.get("preprocessor", {})
        self.processor_inputs = self.processor_config.get("input-names", [])

    def __iter__(self):  # type: ignore[no-untyped-def]
        transport = httpx.HTTPTransport(uds=self.socket_path)

        with httpx.Client(transport=transport, timeout=None) as client:
            processor = None
            if self.processor_config:
                processor = get_preprocessor(self.processor_config, self.fetch_path)

            # State: Holds "leftovers" that are smaller than one batch
            remainder = None

            while True:
                try:
                    resp = client.post("http://hypha/resources/fetch", json=self.config["data"], timeout=None)
                    resp.raise_for_status()
                    tensor_data = resp.json()

                    file_path = os.path.join(self.work_dir, tensor_data[0]["path"])
                    with open(file_path, "rb") as file:
                        raw_bytes = uncompress(file.read())
                    data = load(raw_bytes)
                except Exception as e:
                    logger.error(f"Error loading {file_path}: {e}")
                    continue

                if processor:
                    proc_inputs = {k: data[k] for k in self.processor_inputs if k in data}
                    proc_out = processor(**proc_inputs)
                    # Merge processed output (taking index 0 for batch dim) and unwrap list if exits.
                    data.update({k: v[0] if isinstance(v, list) else v for k, v in proc_out.items()})

                processed = {k: v for k, v in data.items() if k in self.model_inputs}

                if not processed:
                    continue

                # Get the number of samples in the new file
                first_key = next(iter(processed))
                num_new = processed[first_key].shape[0]

                cursor = 0
                if remainder is not None:
                    rem_len = remainder[first_key].shape[0]
                    needed = self.batch_size - rem_len

                    if num_new >= needed:
                        # Take 'needed' slice from new data
                        slice_needed = {k: v[:needed] for k, v in processed.items()}

                        batch = {}
                        for k in remainder:
                            # Cat the remainder + slice
                            cat_tensor = torch.cat([remainder[k], slice_needed[k]], dim=0)
                            # CRITICAL: contiguous() fixes the Segfault by re-packing memory
                            batch[k] = cat_tensor.contiguous()

                        yield batch

                        cursor = needed
                        remainder = None  # Remainder is consumed
                    else:
                        # New file is too small to even fill the gap. Append all of it.
                        for k in remainder:
                            remainder[k] = torch.cat([remainder[k], processed[k]], dim=0)
                        cursor = num_new  # We consumed the whole file

                while cursor + self.batch_size <= num_new:
                    end = cursor + self.batch_size

                    # Yield a clean, INDEPENDENT copy using .clone()
                    # This prevents sending a "View" of the underlying raw_bytes buffer
                    # which might be gc'ed when the loop iterates.
                    batch = {k: v[cursor:end].clone() for k, v in processed.items()}
                    yield batch

                    cursor = end

                if cursor < num_new:
                    # .clone() is safer here to let the original 'data' be garbage collected
                    remainder = {k: v[cursor:].clone() for k, v in processed.items()}
