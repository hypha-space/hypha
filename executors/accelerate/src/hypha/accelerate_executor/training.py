import argparse
import json
import os
import time
import uuid
from typing import Optional

import torch
import torch.utils.data
from accelerate import Accelerator
from safetensors.torch import save_file, save_model

from .api import Session
from .dataset import IterableStreamDataSet, dataset_wrapper
from .model import get_model
from .utils import (
    extract_gradients,
    fetch_data,
    get_adam,
    get_preprocessor,
    get_scheduler,
    merge_models,
    prepare_files,
)

FETCH_PATH = "artifacts"


def system_time_to_epoch_ms(timeout: object) -> Optional[int]:
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
    now_ms = int(time.time() * 1000.0)
    if target_ms > now_ms:
        time.sleep((target_ms - now_ms) / 1000.0)


def main(socket_path: str, work_dir: str, job_json: str) -> None:  # noqa: PLR0915, PLR0912
    # Background receiver context that fills a queue with update pointers
    with Session(socket_path) as session:
        job_spec = json.loads(job_json)

        executor = job_spec["executor"]
        assert executor["class"] == "train"
        config = executor["config"]

        print(json.dumps(executor))

        accelerator = Accelerator(project_dir=work_dir)

        prepare_files(config, session)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        print(os.listdir(local_fetch_path))

        model = get_model(local_fetch_path, config["model"]["task"])
        optimizer = get_adam(config["optimizer"], model.parameters())
        scheduler = get_scheduler(config.get("scheduler"), optimizer)
        preprocessor_config = config.get("preprocessor")
        data_loader = torch.utils.data.DataLoader(
            IterableStreamDataSet(
                fetch_data(session, config["data"], work_dir),
                config["model"]["input-names"],
                preprocessor_config["input-names"] if preprocessor_config else [],
                get_preprocessor(preprocessor_config, local_fetch_path),
            ),
            batch_size=config["batch_size"],
        )

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, "0_global_weights.pt")
        save_model(model, previous_model_path)

        model, optimizer, training_dataloader, scheduler = accelerator.prepare(model, optimizer, data_loader, scheduler)
        training_data_iter = dataset_wrapper(training_dataloader)

        epoch_counter = 1
        job_id = job_spec["job_id"]
        MIN_LOOP_TIME_MS = 100
        last_gradient: Optional[str] = None
        last_metrics: dict[str, float] = {}

        current_status = {
            "executor": "train",
            "details": {"state": "idle"},
        }

        while True:
            loop_start_ms = time.time() * 1000.0
            action_resp = session.send_action({"job_id": job_id, "status": current_status})
            next_action = action_resp.get("next", {})

            if next_action.get("executor") != "train":
                raise RuntimeError(f"Unexpected executor action: {next_action}")

            action = next_action.get("action", {})
            kind = action.get("kind")

            print(f"Action: {kind}", flush=True)

            if kind == "terminate":
                print("Training finished", flush=True)
                break

            if kind == "idle":
                timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                if timeout_ms is not None:
                    sleep_until_epoch_ms(timeout_ms)
                current_status = {"executor": "train", "details": {"state": "idle"}}
            elif kind == "execute-batch":
                batch = next(training_data_iter)
                optimizer.zero_grad()
                outputs = model(**batch)
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
                    # Prepare gradients for potential SendUpdate
                    file_name = f"{epoch_counter}_local_gradients.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Copy weights to CPU without moving the live model off-device.
                    model_state = accelerator.unwrap_model(model).state_dict()
                    state_cpu = {k: v.detach().cpu() for k, v in model_state.items()}
                    save_file(extract_gradients(state_cpu, previous_model_path), result_path)
                    last_gradient = file_name
                    last_metrics = {"loss": float(loss.detach().cpu().numpy())}
                else:
                    current_status = {
                        "executor": "train",
                        "details": {"state": "batch-completed", "batch_size": 0},
                    }
            elif kind == "send-update":
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

                timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                timeout_sec = (timeout_ms - int(time.time() * 1000.0)) / 1000.0 if timeout_ms else None
                if timeout_sec is not None and timeout_sec < 1.0:
                    timeout_sec = 1.0

                try:
                    session.send_resource(target, last_gradient, timeout=timeout_sec)
                    current_status = {"executor": "train", "details": {"state": "sent-update"}}
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

                timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                read_timeout = (timeout_ms - int(time.time() * 1000.0)) / 1000.0 if timeout_ms else None
                if read_timeout is not None and read_timeout <= 0:
                    # Scheduler will tell us what to do next.
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": "ApplyUpdate timeout reached before receive",
                        },
                    }
                    continue

                receive_path = f"incoming-{uuid.uuid4()}"

                try:
                    with session.receive(source, receive_path, timeout=read_timeout) as receiver:
                        updates_iter = iter(receiver)
                        pointers = next(updates_iter)
                        if pointers:
                            latest = pointers[-1] if isinstance(pointers, list) else pointers
                            parameters = (
                                latest.get("parameters") if isinstance(latest.get("parameters"), dict) else None
                            )
                            rel_path = parameters.get("path") if parameters else latest.get("path")
                            if isinstance(rel_path, str):
                                path = os.path.join(work_dir, rel_path)
                                model.load_state_dict(merge_models(previous_model_path, path))
                                save_model(model, previous_model_path)
                                model = accelerator.prepare(model)
                except StopIteration:
                    current_status = {
                        "executor": "train",
                        "details": {
                            "state": "error",
                            "type": "connection",
                            "message": "Receiver stream closed; no updates to merge.",
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
                    "details": {"state": "applied-update", "round": epoch_counter, "metrics": last_metrics},
                }
                epoch_counter += 1
            else:
                raise RuntimeError(f"Unhandled action kind: {kind}")

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)

        print(f"Finished training of {epoch_counter - 1} DiLoCo update rounds", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
