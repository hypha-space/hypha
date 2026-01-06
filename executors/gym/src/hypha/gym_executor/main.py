import argparse
import json
import logging
import time
from collections import deque
from pathlib import Path

import gymnasium as gym
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

                case "generate":
                    for _ in range(num_local_rounds):
                        # actions = [agent.get_action(observation) for observation in observations]
                        actions = envs.action_space.sample()
                        next_observations, rewards, terminations, truncations, infos = envs.step(actions)

                        for i in range(envs.num_envs):
                            if not episode_start[i]:
                                replay_buffer.append(
                                    (
                                        observations[i],
                                        actions[i],
                                        rewards[i],
                                        terminations[i],
                                        next_observations[i],
                                    )
                                )

                        observations = next_observations
                        episode_start = np.logical_or(terminations, truncations)

                        all_observations.append(observations)

                    current_status = {"executor": "gymnasium", "details": {"state": "generated-data"}}

                case "send":
                    target = action.get("target")
                    if target is None:
                        current_status = {
                            "executor": "gymnasium",
                            "details": {"state": "error", "type": "other", "message": "Send missing target reference"},
                        }
                        continue

                    observations_path = Path(work_dir) / "observations.safetensors"
                    save_file({"observations": np.concatenate(all_observations)}, observations_path)

                    try:
                        session.send_resource(target, str(observations_path))
                        current_status = {"executor": "gymnasium", "details": {"state": "sent-data"}}
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
                    current_status = {"executor": "gymnasium", "details": {"state": "received-agent-state"}}

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)

        envs.close()
        logger.info("Environment closed")


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    job_spec = json.loads(job_json)

    executor = job_spec["executor"]
    assert executor["class"] == "gymnasium"

    gymnasium(socket_path, work_dir, job_spec["job_id"], executor["config"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
