import argparse
import json
import logging
import os
import shutil
import time
import uuid
from pathlib import Path

import gymnasium as gym
import numpy as np
import torch
from safetensors.torch import load_file, load_model, save_file, save_model
from torch import nn
from transformers import AutoModel

from .api import Session
from .utils import extract_gradients, get_adam, merge_models, prepare_files

FETCH_PATH = "artifacts"
CURRENT_MODEL_NAME = "global_weights.pt"
MIN_LOOP_TIME_MS = 100

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


def ppo_trainer(socket_path: str, work_dir: str, job_id: str, config) -> None:  # noqa: PLR0912, PLR0915
    with Session(socket_path) as session:
        prepare_files(config, session)
        local_fetch_path = str(Path(work_dir) / FETCH_PATH)
        logger.info("Fetched artifacts: %s", os.listdir(local_fetch_path))

        ######### Parameters #########
        gym_id = "HalfCheetah-v5"
        lr = 3e-4
        seed = 1
        total_timesteps = 2000000
        capture_video = False
        num_envs = 1
        num_steps = 2048
        gamma = 0.99
        gae_lambda = 0.95
        num_minibatches = 32
        update_epochs = 10
        clip_coef = 0.2
        ent_coef = 0.0
        vf_coef = 0.5
        max_grad_norm = 0.5
        target_kl = None
        batch_size = int(num_envs * num_steps)
        minibatch_size = int(batch_size // num_minibatches)
        cuda = False
        num_updates = total_timesteps // batch_size
        ######### Parameters #########
        update = 0

        agent = AutoModel.from_pretrained(local_fetch_path, trust_remote_code=True).to("cpu")

        # model = get_model(local_fetch_path, config["model"]["task"])
        optimizer = get_adam(config["optimizer"], agent.parameters())
        # scheduler = get_scheduler(config.get("scheduler"), optimizer)
        batch_size = config["batch_size"]

        previous_model_path = str(Path(work_dir) / CURRENT_MODEL_NAME)
        save_model(agent, previous_model_path)

        epoch_counter = 1
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

            match kind:
                case "terminate":
                    logger.info("Training finished")
                    break

                case "idle":
                    timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                    if timeout_ms is not None:
                        sleep_until_epoch_ms(timeout_ms)

                    current_status = {"executor": "train", "details": {"state": "idle"}}

                case "execute-batch":
                    # ALGO Logic: Storage setup
                    # Expected Data
                    # obs = torch.zeros((num_steps, num_envs) + envs.single_observation_space.shape).to(device)
                    # actions = torch.zeros((num_steps, num_envs) + envs.single_action_space.shape).to(device)
                    # logprobs = torch.zeros((num_steps, num_envs)).to(device)
                    # rewards = torch.zeros((num_steps, num_envs)).to(device)
                    # dones = torch.zeros((num_steps, num_envs)).to(device)
                    # values = torch.zeros((num_steps, num_envs)).to(device)

                    receive_path = str(Path(work_dir) / str(uuid.uuid4()))
                    training_data_json = session.receive(config["source"], receive_path)

                    file_path = Path(work_dir) / training_data_json[0]["path"]
                    training_data = load_file(file_path)

                    obs = training_data["obs"]
                    actions = training_data["actions"]
                    logprobs = training_data["logprobs"]
                    rewards = training_data["rewards"]
                    dones = training_data["dones"]
                    values = training_data["values"]

                    # first_obs, _ = envs.reset()
                    # next_obs = torch.Tensor(first_obs)
                    next_done = torch.zeros(num_envs)
                    num_updates = total_timesteps // batch_size

                    # Annealing the rate if instructed to do so.
                    frac = 1.0 - (update - 1.0) / num_updates
                    lrnow = frac * lr
                    optimizer.param_groups[0]["lr"] = lrnow

                    # bootstrap value if not done
                    with torch.no_grad():
                        # next_value = agent.get_value(next_obs).reshape(1, -1)
                        next_value = obs.reshape(1, -1)

                        advantages = torch.zeros_like(rewards)
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

                    # flatten the batch
                    b_obs = obs.reshape((-1,) + agent.config.observation_space)
                    b_logprobs = logprobs.reshape(-1)
                    b_actions = actions.reshape((-1,) + agent.config.action_space)
                    b_advantages = advantages.reshape(-1)
                    b_returns = returns.reshape(-1)
                    b_values = values.reshape(-1)

                    # Optimizing the policy and value network
                    b_inds = np.arange(batch_size)
                    clipfracs = []
                    for epoch in range(update_epochs):
                        np.random.shuffle(b_inds)
                        for start in range(0, batch_size, minibatch_size):
                            end = start + minibatch_size
                            mb_inds = b_inds[start:end]

                            _, newlogprob, entropy, newvalue = agent.get_action_and_value(
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

                            optimizer.zero_grad()
                            loss.backward()
                            nn.utils.clip_grad_norm_(agent.parameters(), max_grad_norm)
                            optimizer.step()

                    y_pred, y_true = b_values.cpu().numpy(), b_returns.cpu().numpy()
                    var_y = np.var(y_true)
                    explained_var = np.nan if var_y == 0 else 1 - np.var(y_true - y_pred) / var_y

                    update += 1

                    current_status = {
                        "executor": "train",
                        "details": {"state": "batch-completed", "batch_size": "", "batches": ""},
                    }

                case "send-update":
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
                    state_cpu = {k: v.detach().cpu() for k, v in agent.state_dict().items()}
                    save_file(extract_gradients(state_cpu, previous_model_path, weight), result_path)
                    last_gradient = file_name

                    last_metrics = {"loss": float(np.mean(loss_list))} if loss_list else {}

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
                    loss_list = []
                case "apply-update":
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
                            parameters = (
                                latest.get("parameters") if isinstance(latest.get("parameters"), dict) else None
                            )
                            rel_path = parameters.get("path") if parameters else latest.get("path")
                            if isinstance(rel_path, str):
                                path = str(
                                    Path(work_dir) / rel_path
                                )  # As https://github.com/huggingface/safetensors/blob/806426784adb43631e9a1102d4621126bb589347/bindings/python/py_src/safetensors/torch.py#L228C33-L228C48
                                # it should be fine to use `strict=False` here.
                                agent.load_state_dict(merge_models(previous_model_path, path), strict=False)
                                save_model(agent, previous_model_path)

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
                    except Exception as exc:
                        current_status = {
                            "executor": "train",
                            "details": {"state": "error", "type": "connection", "message": str(exc)},
                        }
                        continue

                case "push-to-hub":
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

                case "send-model":
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
                case "receive-model":
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
                                load_model(agent, path)
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

                case "wait-for-model":
                    timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                    if timeout_ms is not None:
                        sleep_until_epoch_ms(timeout_ms)
                    current_status = {"executor": "train", "details": {"state": "waited-for-model"}}

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    job_spec = json.loads(job_json)

    executor = job_spec["executor"]
    assert executor["class"] == "rl-trainer"

    ppo_trainer(socket_path, work_dir, job_spec["job_id"], executor["config"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
