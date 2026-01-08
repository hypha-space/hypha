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
import gymnasium as gym
import numpy as np
import torch
import torch.nn as nn
import torch.optim as optim
from torch.distributions.normal import Normal
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

from .api import Session
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


def make_env(gym_id, normalize_reward=True):
    def thunk():
        env = gym.make(gym_id)
        env = gym.wrappers.RecordEpisodeStatistics(env)
        env = gym.wrappers.ClipAction(env)
        env = gym.wrappers.NormalizeObservation(env)
        env = gym.wrappers.TransformObservation(env, lambda obs: np.clip(obs, -10, 10), env.observation_space)
        if normalize_reward:
            env = gym.wrappers.NormalizeReward(env)
            env = gym.wrappers.TransformReward(env, lambda reward: np.clip(reward, -10, 10))
        return env

    return thunk


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
        device = "cuda" if torch.cuda.is_available() else "cpu"
        job_spec = json.loads(args.job)

        executor = job_spec["executor"]
        assert executor["class"] == "train"
        config = executor["config"]

        prepare_files(config, session)
        local_fetch_path = f"{work_dir}/{FETCH_PATH}"
        logger.info("Fetched artifacts: %s", os.listdir(local_fetch_path))

        num_envs = 1
        gym_id = "HalfCheetah-v5"
        envs = gym.vector.SyncVectorEnv([make_env(gym_id) for i in range(num_envs)])
        eval_env = make_env(gym_id, normalize_reward=False)()

        num_steps = 1024 #2048
        gamma = 0.99
        gae_lambda = 0.95
        lr = 3e-4
        # total_timesteps = 2000000
        num_minibatches = 16
        update_epochs = 10
        clip_coef = 0.2
        ent_coef = 0.0
        vf_coef = 0.5
        max_grad_norm = 0.5
        target_kl = None
        batch_size = int(num_envs * num_steps)  # config["batch_size"]
        minibatch_size = int(batch_size // num_minibatches)
        # num_updates = total_timesteps // batch_size

        global_step = 0
        first_obs, _ = envs.reset()
        next_obs = torch.Tensor(first_obs).to(device)
        next_done = torch.zeros(num_envs).to(device)

        model = get_model(local_fetch_path, config["model"]["task"])
        optimizer = get_adam(config["optimizer"], model.parameters())
        # batch_size = config["batch_size"]

        # Serialize the model to disk
        previous_model_path = os.path.join(work_dir, CURRENT_MODEL_NAME)
        save_model(model, previous_model_path)

        epoch_counter = 1
        job_id = job_spec["job_id"]
        last_gradient: str | None = None
        last_metrics: dict[str, float] = {}
        v_losses = []
        p_losses = []
        losses = []
        eval_returns = []

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

                #
                # Generate
                #

                obs = torch.zeros((num_steps, num_envs) + envs.single_observation_space.shape).to(device)
                actions = torch.zeros((num_steps, num_envs) + envs.single_action_space.shape).to(device)
                logprobs = torch.zeros((num_steps, num_envs)).to(device)
                rewards = torch.zeros((num_steps, num_envs)).to(device)
                dones = torch.zeros((num_steps, num_envs)).to(device)
                values = torch.zeros((num_steps, num_envs)).to(device)

                for step in range(0, num_steps):
                    global_step += 1 * num_envs
                    obs[step] = next_obs
                    dones[step] = next_done

                    # ALGO LOGIC: action logic
                    with torch.no_grad():
                        act, logprob, _, value = model.get_action_and_value(next_obs)
                        values[step] = value.flatten()
                    actions[step] = act
                    logprobs[step] = logprob

                    # TRY NOT TO MODIFY: execute the game and log data.
                    next_obs, reward, terminated, truncated, _ = envs.step(act.cpu().numpy())
                    rewards[step] = torch.tensor(reward).to(device).view(-1)
                    next_obs, next_done = (
                        torch.Tensor(next_obs).to(device),
                        torch.Tensor(np.logical_or(terminated, truncated)).to(device),
                    )

                # bootstrap value if not done
                with torch.no_grad():
                    next_value = model.get_value(next_obs).reshape(1, -1)

                    advantages = torch.zeros_like(rewards).to(device)
                    lastgaelam = 0
                    for t in reversed(range(num_steps)):
                        if t == num_steps - 1:
                            nextnonterminal = 1.0 - next_done
                            nextvalues = next_value
                        else:
                            nextnonterminal = 1.0 - dones[t + 1]
                            nextvalues = values[t + 1]
                        delta = rewards[t] + gamma * nextvalues * nextnonterminal - values[t]
                        advantages[t] = lastgaelam = delta + gamma * gae_lambda * nextnonterminal * lastgaelam
                    returns = advantages + values

                #
                # Train
                #

                # flatten the batch
                b_obs = obs.reshape((-1,) + envs.single_observation_space.shape).to(device)
                b_actions = actions.reshape((-1,) + envs.single_action_space.shape).to(device)
                b_logprobs = logprobs.reshape(-1).to(device)
                b_advantages = advantages.reshape(-1).to(device)
                b_returns = returns.reshape(-1).to(device)
                b_values = values.reshape(-1).to(device)

                ## TODO
                # Annealing the rate if instructed to do so.
                # frac = 1.0 - (update - 1.0) / num_updates
                logger.info(f"{action}")
                optimizer.param_groups[0]["lr"] = config["optimizer"]["learning-rate"] * (1. - action.get("lr_multiplier"))

                # Optimizing the policy and value network
                b_inds = np.arange(batch_size)
                clipfracs = []
                for epoch in range(update_epochs):
                    np.random.shuffle(b_inds)
                    for start in range(0, batch_size, minibatch_size):
                        end = start + minibatch_size
                        mb_inds = b_inds[start:end]

                        _, newlogprob, entropy, newvalue = model.get_action_and_value(
                            b_obs[mb_inds], b_actions[mb_inds]
                        )
                        logratio = newlogprob - b_logprobs[mb_inds]
                        ratio = logratio.exp()

                        with torch.no_grad():
                            # calculate approx_kl http://joschu.net/blog/kl-approx.html
                            old_approx_kl = (-logratio).mean()
                            approx_kl = ((ratio - 1) - logratio).mean()
                            clipfracs += [((ratio - 1.0).abs() > clip_coef).float().mean().item()]

                        mb_advantages = b_advantages[mb_inds]
                        mb_advantages = (mb_advantages - mb_advantages.mean()) / (mb_advantages.std() + 1e-8)

                        # Policy loss
                        pg_loss1 = -mb_advantages * ratio
                        pg_loss2 = -mb_advantages * torch.clamp(ratio, 1 - clip_coef, 1 + clip_coef)
                        pg_loss = torch.max(pg_loss1, pg_loss2).mean()

                        # Value loss
                        newvalue = newvalue.view(-1)
                        v_loss_unclipped = (newvalue - b_returns[mb_inds]) ** 2
                        v_clipped = b_values[mb_inds] + torch.clamp(
                            newvalue - b_values[mb_inds],
                            -clip_coef,
                            clip_coef,
                        )
                        v_loss_clipped = (v_clipped - b_returns[mb_inds]) ** 2
                        v_loss_max = torch.max(v_loss_unclipped, v_loss_clipped)
                        v_loss = 0.5 * v_loss_max.mean()

                        entropy_loss = entropy.mean()
                        loss = pg_loss - ent_coef * entropy_loss + v_loss * vf_coef
                        p_losses.append(pg_loss.detach().cpu().numpy())
                        v_losses.append(v_loss.detach().cpu().numpy())
                        losses.append(loss.detach().cpu().numpy())

                        optimizer.zero_grad()
                        loss.backward()
                        nn.utils.clip_grad_norm_(model.parameters(), max_grad_norm)
                        optimizer.step()

                y_pred, y_true = b_values.cpu().numpy(), b_returns.cpu().numpy()
                var_y = np.var(y_true)
                explained_var = np.nan if var_y == 0 else 1 - np.var(y_true - y_pred) / var_y

                # Do eval
                with torch.no_grad():
                    training_stats = envs.envs[0].get_wrapper_attr("obs_rms")
                    eval_stats = eval_env.get_wrapper_attr("obs_rms")
                    eval_stats.mean = training_stats.mean

                    eval_obs, _ = eval_env.reset()
                    eval_return = 0
                    eval_terminated = False
                    eval_truncated = False
                    while not (eval_terminated or eval_truncated):
                        act = model.actor_mean(torch.Tensor(eval_obs).reshape((1, -1)))
                        eval_obs, eval_reward, eval_terminated, eval_truncated, _ = eval_env.step(
                            act.cpu().numpy().squeeze()
                        )
                        eval_return += eval_reward - eval_truncated * -100

                    eval_returns.append(eval_return)

                current_status = {
                    "executor": "train",
                    "details": {"state": "batch-completed", "batch_size": 1, "batches": 1},
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

                last_metrics = {
                    "loss": float(np.mean(losses)) if losses else 0,
                    "v_loss": float(np.mean(v_losses)) if v_losses else 0,
                    "p_loss": float(np.mean(p_losses)) if p_losses else 0,
                    "eval_return": float(np.mean(eval_returns)) if eval_returns else 0,
                }

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
                losses = []
                v_losses = []
                p_losses = []
                eval_returns = []
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

                logger.info("Received deprecated push-to-hub action; pushing model")
                try:
                    model.push_to_hub(repository, token=token)
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
