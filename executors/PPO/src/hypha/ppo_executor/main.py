import argparse
import json
import logging
import time
from collections import deque
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.optim as optim
from torch.distributions.normal import Normal
import numpy as np
from safetensors.numpy import save_file

from .api import Session

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


def gymnasium(socket_path: str, work_dir: str, job_id: str, config) -> None:
    with Session(socket_path) as session:
        envs = gym.make_vec(config["environment"], num_envs=4)

        replay_buffer = deque(maxlen=100)
        episode_start = np.zeros(envs.num_envs, dtype=bool)

        num_local_rounds = 1
        observations, infos = envs.reset()

        all_observations = []

        current_status = {
            "executor": "gymnasium",
            "details": {"state": "joined"},
        }
        
        ######### Parameters #########
        lr = 3e-4
        total_timesteps = 2000000
        capture_video = False
        num_envs = 1
        num_steps = 2048
        gamma = .99
        gae_lambda = .95
        num_minibatches = 32
        update_epochs = 10
        clip_coef = .2
        ent_coef = .0
        vf_coef = 0.5
        max_grad_norm = .5
        target_kl = None
        batch_size = int(num_envs * num_steps)
        minibatch_size = int(batch_size // num_minibatches)
        cuda = False
        ######### Parameters #########

        while True:
            loop_start_ms = time.time() * 1000.0
            action_resp = session.send_action({"job_id": job_id, "status": current_status})
            next_action = action_resp.get("next", {})

            if next_action.get("executor") != "gymnasium":
                raise RuntimeError(f"Unexpected executor action: {next_action}")

            action = next_action.get("action", {})
            kind = action.get("kind")

            logger.info("Action: %s", kind)

            match kind:
                case "terminate":
                    logger.info("Data generation finished")
                    break

                case "idle":
                    timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                    if timeout_ms is not None:
                        sleep_until_epoch_ms(timeout_ms)

                    current_status = {"executor": "gymnasium", "details": {"state": "idle"}}

                case "train":
                    
                    # ALGO Logic: Storage setup
                    # Expected Data
                    # obs = torch.zeros((num_steps, num_envs) + envs.single_observation_space.shape).to(device)
                    # actions = torch.zeros((num_steps, num_envs) + envs.single_action_space.shape).to(device)
                    # logprobs = torch.zeros((num_steps, num_envs)).to(device)
                    # rewards = torch.zeros((num_steps, num_envs)).to(device)
                    # dones = torch.zeros((num_steps, num_envs)).to(device)
                    # values = torch.zeros((num_steps, num_envs)).to(device)
                    
                    # Annealing the rate if instructed to do so.
                    frac = 1.0 - (update - 1.0) / num_updates
                    lrnow = frac * lr
                    optimizer.param_groups[0]["lr"] = lrnow
                    
                    # bootstrap value if not done
                    with torch.no_grad():
                        next_value = agent.get_value(next_obs).reshape(1, -1)
            
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
            
                    # flatten the batch
                    b_obs = obs.reshape((-1,) + envs.single_observation_space.shape)
                    b_logprobs = logprobs.reshape(-1)
                    b_actions = actions.reshape((-1,) + envs.single_action_space.shape)
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
            
                            _, newlogprob, entropy, newvalue = agent.get_action_and_value(b_obs[mb_inds], b_actions[mb_inds])
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
                    
                    current_status = {"executor": "gymnasium", "details": {"state": "train", ""}}

                case "send":
                    target = action.get("target")
                    if target is None:
                        current_status = {
                            "executor": "gymnasium",
                            "details": {"state": "error", "type": "other", "message": "Send missing target reference"},
                        }
                        continue

                    observations_path = Path(work_dir) / "observations.safetensors"
                    save_file({"observations": all_observations}, observations_path)

                    try:
                        session.send_resource(target, str(observations_path))
                        current_status = {"executor": "gymnasium", "details": {"state": "send"}}
                    except Exception as exc:
                        current_status = {
                            "executor": "gymnasium",
                            "details": {"state": "error", "type": "connection", "message": str(exc)},
                        }
                        continue

                case "update":
                    # TODO: load current model state and setup agent
                    # model_state = session.fetch(config["model"]["artifact"])
                    # model_state = os.path.join(work_dir, model_state[0]["path"])
                    # model = load_from_safetensor(model_state)

                    # agent = PPOAgent(env, model)
                    # TODO: set agent state
                    current_status = {"executor": "gymnasium", "details": {"state": "update"}}

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)

        envs.close()
        logger.info("Environment closed")


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    job_spec = json.loads(job_json)

    executor = job_spec["executor"]
    assert executor["class"] == "gymnasium"

    gymnasium(socket_path, work_dir, executor["job_id"], executor["config"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
