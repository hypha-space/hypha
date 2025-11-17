import argparse
import datetime
import json
import os

import numpy as np
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
        # Start receiver immediately, but do not consume until we've sent once
        with session.receive(config["results"], "incoming") as receiver:
            updates_iter = iter(receiver)
            await_update = False

            while True:
                start_time = datetime.datetime.now()

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
                                    response = session.send_status("update-received")
                                    if response["type"] == "Done":
                                        print("Training finished")
                                        break
                            except Exception as e:
                                print(f"pointer handling error: {e}")
                    except StopIteration:
                        print("Receiver stream closed; no updates to merge.")
                    finally:
                        await_update = False

                losses = []
                counter = -1
                while counter != 0:
                    batch = next(training_data_iter)
                    optimizer.zero_grad()
                    outputs = model(**batch)
                    loss = outputs if isinstance(outputs, torch.Tensor) else outputs["loss"]
                    accelerator.backward(loss)
                    optimizer.step()
                    scheduler.step()
                    losses.append(loss.detach().cpu().numpy())
                    if accelerator.is_main_process:
                        round_time = start_time - datetime.datetime.now()
                        response = session.send_status(
                            {
                                "status": {
                                    "batch_size": next(iter(batch.values())).shape[0],
                                    "round_time": int(round_time.microseconds / 1000),
                                }
                            }
                        )
                        if response["type"] == "ScheduleUpdate":
                            counter = response["counter"]
                        else:
                            counter -= 1
                        start_time = datetime.datetime.now()

                if accelerator.is_main_process:
                    session.send_status("update")
                    # For testing purposes set to global!
                    file_name = f"{epoch_counter}_local_gradients.pt"
                    result_path = os.path.join(work_dir, file_name)
                    # Save unwrapped model to avoid accelerator wrappers interfering
                    model = accelerator.unwrap_model(model)
                    # All weights need to be on CPU
                    model.to("cpu")

                    save_file(extract_gradients(model.state_dict(), previous_model_path), result_path)
                    session.send_resource(config["updates"], file_name)

                    # Mark that before the next training epoch we must wait for an update
                    await_update = True

                    session.send_status(
                        {"metrics": {"round": epoch_counter, "metrics": {"loss": float(np.mean(losses))}}}
                    )
                    epoch_counter += 1

            print(f"Finished training of {epoch_counter - 1} DiLoCo update rounds", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
