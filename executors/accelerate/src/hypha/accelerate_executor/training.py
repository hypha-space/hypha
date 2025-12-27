import argparse
import json
import logging
import os
import shutil
import sys
import time
import uuid

import numpy as np
import torch
import torch.utils.data
from accelerate import Accelerator, DataLoaderConfiguration
from opentelemetry import metrics
from opentelemetry._logs import set_logger_provider
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
from opentelemetry.instrumentation.system_metrics import SystemMetricsInstrumentor
from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
from opentelemetry.sdk.resources import OTELResourceDetector, get_aggregated_resources
from safetensors.torch import load_file, save_file, save_model

from .api import Session
from .dataset import IterableStreamDataSet
from .model import get_model
from .utils import (
    extract_gradients,
    get_adam,
    get_scheduler,
    merge_models,
    prepare_files,
)

FETCH_PATH = "artifacts"
CURRENT_MODEL_NAME = "global_weights.pt"
MIN_LOOP_TIME_MS = 100

otel_handler = None
if "OTEL" in os.environ:
    resource = get_aggregated_resources([OTELResourceDetector()])

    # Configure OTEL
    logger_provider = LoggerProvider(resource=resource)
    print(logger_provider)
    set_logger_provider(logger_provider)

    exporter = OTLPLogExporter()
    logger_provider.add_log_record_processor(BatchLogRecordProcessor(exporter))
    otel_handler = LoggingHandler(level=logging.NOTSET, logger_provider=logger_provider)

    metric_exporter = OTLPMetricExporter()
    metric_reader = PeriodicExportingMetricReader(metric_exporter)
    meter_provider = MeterProvider(resource=resource, metric_readers=[metric_reader])
    metrics.set_meter_provider(meter_provider)
    SystemMetricsInstrumentor().instrument(meter_provider=meter_provider)

# NOTE: Set the root logger level to NOTSET to ensure all messages are captured
# and attach OTLP + console handlers to root logger
console_handler = logging.StreamHandler(sys.stdout)
console_handler.setLevel(logging.INFO)
console_handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))

logging.getLogger().setLevel(logging.NOTSET)
if otel_handler:
    logging.getLogger().addHandler(otel_handler)
logging.getLogger().addHandler(console_handler)

logger = logging.getLogger(__name__)


def system_time_to_epoch_ms(timeout: object) -> int | None:
    if isinstance(timeout, dict):
        secs = timeout.get("secs_since_epoch")
        nanos = timeout.get("nanos_since_epoch", 0)
        if secs is not None:
            return int(secs * 1000 + int(nanos / 1_000_000))
    if isinstance(timeout, (int, float)):
        # Fallback for numeric nanos representation.
        return int(timeout / 1_000_000)
    return None


def sleep_until_epoch_ms(target_ms: int) -> None:
    now_ms = time.time() * 1000.0
    if target_ms > now_ms:
        time.sleep((target_ms - now_ms) / 1000.0)


if __name__ == "__main__":  # noqa: PLR0915, PLR0912
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    work_dir = args.work_dir

    with Session(args.socket) as session:
        job_spec = json.loads(args.job)

        executor = job_spec["executor"]
        assert executor["class"] == "train"
        config = executor["config"]

        dataloader_config = DataLoaderConfiguration(
            dispatch_batches=False,  # avoid rank-0 bottleneck on IterableDataset
            split_batches=False,  # each process gets full batches (data parallel)
            non_blocking=True,
        )

        accelerator = Accelerator(project_dir=work_dir, dataloader_config=dataloader_config)

        prepare_files(config, session)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        logger.info("Fetched artifacts: %s", os.listdir(local_fetch_path))

        model = get_model(local_fetch_path, config["model"]["task"])
        optimizer = get_adam(config["optimizer"], model.parameters())
        scheduler = get_scheduler(config.get("scheduler"), optimizer)
        data_loader = torch.utils.data.DataLoader(
            IterableStreamDataSet(args.socket, work_dir, local_fetch_path, config["batch_size"], config),
            batch_size=None,
            pin_memory=True,
            num_workers=4,
            persistent_workers=True,
        )

        model, optimizer, scheduler = accelerator.prepare(model, optimizer, scheduler)
        training_data_iter = iter(data_loader)

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, CURRENT_MODEL_NAME)
        # model = accelerator.unwrap_model(model)
        save_model(model, previous_model_path)

        epoch_counter = 1
        job_id = job_spec["job_id"]
        last_gradient: str | None = None
        last_metrics: dict[str, float] = {}
        loss_list = []

        current_status = {
            "executor": "train",
            "details": {"state": "joined"},
        }

        while True:
            loop_start_ms = time.time() * 1000.0
            action_resp = session.send_action({"job_id": job_id, "status": current_status})
            next_action = action_resp.get("next", {})

            if next_action.get("executor") != "train":
                raise RuntimeError(f"Unexpected executor action: {next_action}")

            action = next_action.get("action", {})
            kind = action.get("kind")

            logger.info("Action: %s", kind)

            if kind == "terminate":
                logger.info("Training finished")
                break

            if kind == "idle":
                timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                if timeout_ms is not None:
                    sleep_until_epoch_ms(timeout_ms)
                current_status = {"executor": "train", "details": {"state": "idle"}}
            elif kind == "execute-batch":
                local_batches = action.get("batches")
                logger.info(f"Excecute {local_batches} local batches.")
                for _ in range(local_batches):
                    batch = next(training_data_iter)
                    optimizer.zero_grad(set_to_none=True)
                    outputs = model(**{k: v.to(accelerator.device, non_blocking=True) for k, v in batch.items()})
                    loss = outputs if isinstance(outputs, torch.Tensor) else outputs["loss"]
                    accelerator.backward(loss)
                    optimizer.step()
                    scheduler.step()
                    if accelerator.is_main_process:
                        batch_size = next(iter(batch.values())).shape[0]
                        current_status = {
                            "executor": "train",
                            "details": {"state": "batch-completed", "batch_size": batch_size},
                        }
                        loss_list.append(loss.detach().cpu().numpy())
            elif kind == "send-update":
                target = action.get("target")
                if target is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "SendUpdate missing target reference",
                        },
                    }
                    continue

                # Prepare gradients for SendUpdate
                weight = action.get("weight")
                file_name = f"{epoch_counter}_local_gradients.pt"
                result_path = os.path.join(work_dir, file_name)
                # Copy weights to CPU without moving the live model off-device.
                state_cpu = {k: v.detach().cpu() for k, v in model.state_dict().items()}
                save_file(extract_gradients(state_cpu, previous_model_path, weight), result_path)
                last_gradient = file_name

                last_metrics = {"loss": float(np.mean(loss_list))} if loss_list else {}
                # Reset Losses
                loss_list = []

                if last_gradient is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "SendUpdate requested but no gradients available",
                        },
                    }
                    continue

                try:
                    session.send_resource(target, last_gradient)
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "sent-update",
                            "metrics": last_metrics,
                            "round": epoch_counter,
                        },
                    }
                except Exception as exc:  # noqa: BLE001
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": str(exc),
                        },
                    }
            elif kind == "apply-update":
                source = action.get("source")
                if source is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "ApplyUpdate missing source reference",
                        },
                    }
                    continue

                receive_path = f"incoming-{uuid.uuid4()}"

                try:
                    pointers = session.receive(source, receive_path, timeout=action.get("timeout"))
                    if pointers:
                        latest = pointers[-1] if isinstance(pointers, list) else pointers
                        parameters = latest.get("parameters") if isinstance(latest.get("parameters"), dict) else None
                        rel_path = parameters.get("path") if parameters else latest.get("path")
                        if isinstance(rel_path, str):
                            path = os.path.join(work_dir, rel_path)
                            model.load_state_dict(merge_models(previous_model_path, path))
                            save_model(model, previous_model_path)

                            # Once we updated the model, we no longer need the parameter file.
                            os.remove(path)
                    else:
                        current_status = {
                            "executor": "train",
                            "details": {
                                "state": "error",
                                "type": "connection",
                                "message": "No updates received before timeout.",
                            },
                        }
                        continue
                except Exception as exc:  # noqa: BLE001
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": str(exc),
                        },
                    }
                    continue

                current_status = {
                    "executor": "train",
                    "details": {"state": "applied-update"},
                }
                epoch_counter += 1
            elif kind == "push-to-hub":
                repository = action.get("repository")
                token = action.get("token")
                if repository is None or token is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "PushToHub action missing repository or token",
                        },
                    }
                    continue

                if accelerator.is_main_process:
                    logger.info("Received deprecated push-to-hub action; pushing model")
                    try:
                        accelerator.unwrap_model(model).push_to_hub(repository, token=token)
                        logger.info("Model pushed. Training finished")
                    except Exception as exc:  # noqa: BLE001
                        current_status = {
                            "executor": "train",
                            "details": {"state": "error", "type": "other", "message": str(exc)},
                        }
                        continue
                current_status = {"executor": "train", "details": {"state": "pushed-to-hub"}}
            elif kind == "send-model":
                target = action.get("target")
                if target is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "SendModel missing target reference",
                        },
                    }
                    continue

                try:
                    session.send_resource(target, CURRENT_MODEL_NAME, remove_file=False)
                    current_status = {
                        "executor": "train",
                        "details": {"state": "sent-model"},
                    }
                except Exception as exc:  # noqa: BLE001
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": str(exc),
                        },
                    }

            elif kind == "receive-model":
                source = action.get("source")
                if source is None:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "other",
                            "message": "ReceiveModel missing source reference",
                        },
                    }
                    continue

                try:
                    receive_path = f"incoming-{uuid.uuid4()}"
                    pointers = session.receive(source, receive_path, timeout=action.get("timeout"))
                    if pointers:
                        incomming = pointers[-1] if isinstance(pointers, list) else pointers
                        rel_path = incomming.get("path")
                        if isinstance(rel_path, str):
                            path = os.path.join(work_dir, rel_path)
                            model.load_state_dict(load_file(path))
                            os.remove(previous_model_path)
                            shutil.copy(path, previous_model_path)
                            os.remove(path)
                    else:
                        current_status = {
                            "executor": "train",
                            "details": {
                                "state": "error",
                                "type": "connection",
                                "message": "No model received before timeout.",
                            },
                        }
                        continue
                except Exception as exc:  # noqa: BLE001
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": str(exc),
                        },
                    }
                    continue

                current_status = {
                    "executor": "train",
                    "details": {"state": "applied-update"},
                }
            elif kind == "wait-for-model":
                timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                if timeout_ms is not None:
                    sleep_until_epoch_ms(timeout_ms)
                current_status = {"executor": "train", "details": {"state": "waited-for-model"}}
            else:
                raise RuntimeError(f"Unhandled action kind: {kind}")

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)

        logger.info("Finished training of %s DiLoCo update rounds", epoch_counter - 1)
