import argparse
import json
import logging
import os
import shutil
import sys
import time
import uuid
from pathlib import Path

import gymnasium as gym
import numpy as np
import torch
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
from transformers import AutoModel

from .api import Session

FETCH_PATH = "artifacts"
CURRENT_MODEL_NAME = "global_weights.pt"
MIN_LOOP_TIME_MS = 100
FETCH_PATH = "artifacts"

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


def gymnasium(socket_path: str, work_dir: str, job_id: str, config) -> None:  # noqa: PLR0912, PLR0915
    with Session(socket_path) as session:
        device = "cpu"
        num_envs = 4
        # env setup
        envs = gym.vector.SyncVectorEnv([make_env(config["environment"]) for i in range(num_envs)])

        session.fetch(config["model"]["artifact"])
        local_fetch_path = str(Path(work_dir) / FETCH_PATH)
        logger.info("Fetched artifacts: %s", os.listdir(local_fetch_path))
        agent = AutoModel.from_pretrained(local_fetch_path, trust_remote_code=True).to(device)

        # Serialize the model to disk
        previous_model_path = str(Path(work_dir) / CURRENT_MODEL_NAME)
        # model = accelerator.unwrap_model(model)
        save_model(agent, previous_model_path)

        # Do Config
        num_steps = 2048
        gamma = 0.99
        gae_lambda = 0.95

        global_step = 0
        first_obs, _ = envs.reset()
        next_obs = torch.Tensor(first_obs).to(device)
        next_done = torch.zeros(num_envs).to(device)

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
                            action, logprob, _, value = agent.get_action_and_value(next_obs)
                            values[step] = value.flatten()
                        actions[step] = action
                        logprobs[step] = logprob

                        # TRY NOT TO MODIFY: execute the game and log data.
                        next_obs, reward, terminated, truncated, _ = envs.step(action.cpu().numpy())
                        rewards[step] = torch.tensor(reward).to(device).view(-1)
                        next_obs, next_done = (
                            torch.Tensor(next_obs).to(device),
                            torch.Tensor(np.logical_or(terminated, truncated)).to(device),
                        )

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

                    current_status = {"executor": "gymnasium", "details": {"state": "generated-data"}}

                case "wait-for-receiver":
                    timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                    if timeout_ms is not None:
                        sleep_until_epoch_ms(timeout_ms)

                    current_status = {"executor": "gymnasium", "details": {"state": "waited-for-receiver"}}

                case "send":
                    target = action.get("target")

                    observations_file = "observations.safetensors"
                    observations_path = Path(work_dir) / observations_file

                    tensor_dict = {
                        "obs": obs.reshape((-1,) + envs.single_observation_space.shape),
                        "actions": actions.reshape((-1,) + envs.single_action_space.shape),
                        "logprobs": logprobs.reshape(-1),
                        "advantages": advantages.reshape(-1),
                        "returns": returns.reshape(-1),
                        "values": values.reshape(-1),
                    }

                    save_file(tensor_dict, observations_path)

                    try:
                        session.send_resource(target, observations_file, remove_file=True)
                        current_status = {"executor": "gymnasium", "details": {"state": "sent-data"}}
                    except Exception as exc:
                        current_status = {
                            "executor": "gymnasium",
                            "details": {"state": "error", "type": "connection", "message": str(exc)},
                        }
                        continue

                case "wait-for-model":
                    timeout_ms = system_time_to_epoch_ms(action.get("timeout"))
                    if timeout_ms is not None:
                        sleep_until_epoch_ms(timeout_ms)
                    current_status = {"executor": "gymnasium", "details": {"state": "waited-for-model"}}

                case "receive-model":
                    # TODO: load current model state and setup agent
                    # model_state = session.fetch(config["model"]["artifact"])
                    # model_state = os.path.join(work_dir, model_state[0]["path"])
                    # model = load_from_safetensor(model_state)
                    source = action.get("source")
                    if source is None:
                        current_status = {
                            "executor": "gymnasium",
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
                                "executor": "gymnasium",
                                "details": {
                                    "state": "error",
                                    "type": "connection",
                                    "message": "No model received before timeout.",
                                },
                            }
                            continue
                    except Exception as exc:
                        current_status = {
                            "executor": "gymnasium",
                            "details": {
                                "state": "error",
                                "type": "connection",
                                "message": str(exc),
                            },
                        }
                        continue

                    current_status = {"executor": "gymnasium", "details": {"state": "received-model"}}

            elapsed = time.time() * 1000.0 - loop_start_ms
            if elapsed < MIN_LOOP_TIME_MS:
                time.sleep((MIN_LOOP_TIME_MS - elapsed) / 1000.0)


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
