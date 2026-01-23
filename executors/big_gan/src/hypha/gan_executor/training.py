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
from safetensors.torch import load_model, save_file, save_model
from torchmetrics.image.inception import InceptionScore
from torchmetrics.image.fid import FrechetInceptionDistance

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

from safetensors.torch import load_model

FETCH_PATH = "artifacts"
CURRENT_MODEL_NAME = "global_weights.pt"
MIN_LOOP_TIME_MS = 100

# NOTE: Set the root logger level to NOTSET to ensure all messages are captured
# and attach console and OTEL (if configured) handlers to root logger
logging.getLogger().setLevel(logging.NOTSET)

# NOTE: Set level for httpx and httpcore to WARNING to reduce noise
logging.getLogger("httpx").setLevel(logging.WARNING)
logging.getLogger("httpcore").setLevel(logging.WARNING)

console_handler = logging.StreamHandler(sys.stdout)
console_handler.setLevel(logging.INFO)
console_handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))
logging.getLogger().addHandler(console_handler)


# NOTE: Only configure OTEL exporters if endpoint is defined.
# If no endpoint is configured, skip exporters.
otel_endpoint = os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
if otel_endpoint:
    resource = get_aggregated_resources([OTELResourceDetector()])

    exporter = OTLPLogExporter()
    logger_provider = LoggerProvider(resource=resource)
    logger_provider.add_log_record_processor(BatchLogRecordProcessor(exporter))
    set_logger_provider(logger_provider)

    otel_handler = LoggingHandler(level=logging.NOTSET, logger_provider=logger_provider)
    logging.getLogger().addHandler(otel_handler)

    metric_exporter = OTLPMetricExporter()
    metric_reader = PeriodicExportingMetricReader(metric_exporter)
    meter_provider = MeterProvider(resource=resource, metric_readers=[metric_reader])
    metrics.set_meter_provider(meter_provider)

    SystemMetricsInstrumentor().instrument(meter_provider=meter_provider)


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

    logger.info("Started")

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

        gradient_accumulation_steps = 32
        accelerator = Accelerator(
            project_dir=work_dir,
            dataloader_config=dataloader_config,
            gradient_accumulation_steps=gradient_accumulation_steps
        )

        # print(accelerator.state.mixed_precision)

        prepare_files(config, session)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        logger.info("Fetched artifacts: %s", os.listdir(local_fetch_path))

        model = get_model(local_fetch_path, config["model"]["task"])
        load_model(model, local_fetch_path + "/model.safetensors")
        model.train()
        # optimizer = get_adam(config["optimizer"], model.parameters())
        optimizer_D = torch.optim.AdamW(model.discriminator.parameters(), lr=2e-4, betas=(0., 0.999))
        optimizer_G = torch.optim.AdamW(model.generator.parameters(), lr=5e-5, betas=(0., 0.999))
        scheduler = get_scheduler(config.get("scheduler"), optimizer_D)
        batch_size = 64 #config["batch_size"]
        data_loader = torch.utils.data.DataLoader(
            IterableStreamDataSet(args.socket, work_dir, local_fetch_path, batch_size, config),
            batch_size=None,
            pin_memory=True,
            num_workers=4,
            persistent_workers=True,
        )

        model, optimizer_D, optimizer_G, scheduler = accelerator.prepare(model, optimizer_D, optimizer_G, scheduler)
        training_data_iter = iter(data_loader)

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, CURRENT_MODEL_NAME)
        # model = accelerator.unwrap_model(model)
        save_model(model, previous_model_path)

        epoch_counter = 1
        job_id = job_spec["job_id"]
        last_gradient: str | None = None
        last_metrics: dict[str, float] = {}
        g_loss_list = []
        d_loss_list = []
        zero_batch_counter = 0

        inception = InceptionScore(normalize=True)
        fid = FrechetInceptionDistance(feature=192, normalize=True)

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
                if local_batches == 0 and zero_batch_counter > 100 + np.random.randint(0, 15):
                    local_batches = 1
                for _ in range(local_batches):
                    for i in range(gradient_accumulation_steps):
                        with accelerator.accumulate(model):
                            batch = next(training_data_iter)
                            z = torch.randn(batch_size, model.config.latent_dim).to(device=accelerator.device, dtype=torch.bfloat16)
                            fake_labels = torch.nn.functional.one_hot(
                                    torch.randint(
                                        0,
                                        model.config.num_classes,
                                        (batch_size,)
                                    ),
                                    model.config.num_classes
                            ).to(device=accelerator.device, dtype=torch.bfloat16)
                            real_imgs = batch["images"].to(device=accelerator.device, dtype=torch.bfloat16)
                            real_labels = batch["labels"].to(device=accelerator.device, dtype=torch.bfloat16)

                            # --- Train Discriminator ---
                            optimizer_D.zero_grad(set_to_none=True)

                            gen_imgs = model.generator(z, fake_labels)

                            real_loss = torch.mean(torch.nn.functional.relu(1. - model.discriminator(real_imgs, real_labels)))
                            fake_loss = torch.mean(torch.nn.functional.relu(1. + model.discriminator(gen_imgs.detach(), fake_labels)))
                            d_loss = (real_loss + fake_loss) / 2
                            accelerator.backward(d_loss)
                            if accelerator.sync_gradients:
                                accelerator.clip_grad_norm_(model.discriminator.parameters(), 1.)
                            optimizer_D.step()

                            # --- Train Generator ---
                            optimizer_G.zero_grad(set_to_none=True)
                            z = torch.randn(batch_size, model.config.latent_dim).to(device=accelerator.device, dtype=torch.bfloat16)
                            fake_labels = torch.nn.functional.one_hot(
                                    torch.randint(
                                        0,
                                        model.config.num_classes,
                                        (batch_size,)
                                    ),
                                    model.config.num_classes
                            ).to(device=accelerator.device, dtype=torch.bfloat16)
                            gen_imgs = model.generator(z, fake_labels)

                            d_out = model.discriminator(gen_imgs, fake_labels)
                            g_loss = -torch.mean(model.discriminator(gen_imgs, fake_labels))
                            accelerator.backward(g_loss)
                            if accelerator.sync_gradients:
                                accelerator.clip_grad_norm_(model.generator.parameters(), 1.)
                            optimizer_G.step()

                        scheduler.step()
                        if accelerator.is_main_process:
                            g_loss_list.append(g_loss.detach().cpu().float().numpy())
                            d_loss_list.append(d_loss.detach().cpu().float().numpy())

                    if accelerator.is_main_process and batch:
                        with torch.no_grad():
                            test_size=10
                            real_im = batch["images"][:test_size]
                            z_ = torch.randn(test_size, model.config.latent_dim).to(device=accelerator.device, dtype=torch.bfloat16)
                            _fake_labels = torch.nn.functional.one_hot(
                                    torch.randint(
                                        0,
                                        model.config.num_classes,
                                        (test_size,)
                                    ),
                                    model.config.num_classes
                            ).to(device=accelerator.device, dtype=torch.bfloat16)
                            gen_im = model.generator(z_, _fake_labels).cpu().float()
                            inception.update(gen_im)
                            fid.update(gen_im, real=False)
                            fid.update(real_im, real=True)

                if local_batches == 0:
                    zero_batch_counter += 1
                else:
                    zero_batch_counter = 0
                if accelerator.is_main_process:
                    current_status = {
                        "executor": "train",
                        "details": {"state": "batch-completed", "batch_size": 1, "batches": local_batches},
                    }
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

                last_metrics = {}
                if g_loss_list:
                    last_metrics["g_loss"] = float(np.mean(g_loss_list))
                if d_loss_list:
                    last_metrics["d_loss"] = float(np.mean(d_loss_list))
                inception_mean, inception_std = inception.compute()
                last_metrics["inception_mean"] = float(inception_mean)
                last_metrics["inception_std"] = float(inception_std)
                last_metrics["fid"] = float(fid.compute())


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
                    continue

                # Reset Losses only if we succesfully send updates.
                g_loss_list = []
                d_loss_list = []
                inception.reset()
                fid.reset()
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
                            # As https://github.com/huggingface/safetensors/blob/806426784adb43631e9a1102d4621126bb589347/bindings/python/py_src/safetensors/torch.py#L228C33-L228C48
                            # it should be fine to use `strict=False` here.
                            model.load_state_dict(merge_models(previous_model_path, path), strict=False)
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
                zero_batch_counter = 0
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
                            load_model(model, path)
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
