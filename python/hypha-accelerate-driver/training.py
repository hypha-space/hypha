import argparse
import os

import torch
import torch.utils.data
from accelerate import Accelerator
from datasets import load_dataset
from transformers import GPT2Tokenizer, GPTNeoForCausalLM
from safetensors import safe_open
from safetensors.torch import save_model
from tqdm import tqdm

from api import Session


def get_model(model_config: str):
    # Initializing a model (with random weights) from the EleutherAI/gpt-neo-1.3B style configuration
    model = GPTNeoForCausalLM.from_pretrained(model_config)
    tokenizer = GPT2Tokenizer.from_pretrained(model_config)
    return model, tokenizer


def get_optimizer(optimizer_name: str, learning_rate: float, parameters):
    if optimizer_name == "AdamW":
        return torch.optim.AdamW(parameters, learning_rate)
    else:
        raise RuntimeError(f"Optimizer {optimizer_name} doesn't exist.")


def get_data_loader(data_set_name: str, batch_size: int, tokenizer):
    # TODO: data needs to be provided by the task
    ds = load_dataset(data_set_name)["train"]
    train_ds, test_ds = ds.train_test_split(0.2, 0.8, True, seed=40).values()
    train_ds = CustomC4(train_ds["text"][:40], tokenizer, context_length=2048)
    return torch.utils.data.DataLoader(train_ds, batch_size=batch_size)


def merge_models(a, weight_path: str, alpha):
    # All weights need to be on CPU
    a.to("cpu")
    state_dict = a.state_dict()
    with safe_open(weight_path, framework="pt", device="cpu") as b:
        for name in b.keys():
            state_dict[name] += alpha * (b.get_tensor(name) - state_dict[name])
    a.load_state_dict(state_dict)
    return a


# TODO: This needs to be generic and support user-provided datasets.
class CustomC4(torch.utils.data.Dataset):
    def __init__(self, data, tokenizer, context_length):
        tokenizer.pad_token = tokenizer.eos_token
        self.tokenizer = tokenizer
        self.tokenized_data = self._tokenize(data, context_length)

    def _tokenize(self, data, context_length):
        tokenized_inputs = self.tokenizer(
            data,
            return_tensors="pt",
            max_length=context_length,
            truncation=True,
            padding="max_length",
        )
        return tokenized_inputs["input_ids"]

    def __len__(self):
        return len(self.tokenized_data)

    def __getitem__(self, idx):
        return self.tokenized_data[idx]


def main(socket_path: str, work_dir: str):
    with Session(socket_path) as client:
        while True:
            train_config = client.wait_for_task()
            print(f"Training with configuration: {train_config}", flush=True)

            client.set_task_status(train_config["task_id"], "Running")

            accelerator = Accelerator(project_dir=work_dir)
            device = accelerator.device

            model, tokenizer = get_model(train_config["model"])
            optimizer = get_optimizer(
                train_config["optimizer"],
                train_config["learning_rate"],
                model.parameters(),
            )

            # TODO: remove this
            milestone = int(train_config["epochs"] / 3)
            scheduler_warmup = torch.optim.lr_scheduler.LinearLR(
                optimizer, train_config["learning_rate"], 1e-3, milestone
            )
            scheduler_cool_down = torch.optim.lr_scheduler.LinearLR(
                optimizer,
                1e-3,
                train_config["learning_rate"],
                train_config["epochs"] - milestone,
            )
            scheduler = torch.optim.lr_scheduler.SequentialLR(
                optimizer,
                schedulers=[scheduler_warmup, scheduler_cool_down],
                milestones=[milestone],
            )
            data_loader = get_data_loader(
                train_config["dataset"], train_config["batch_size"], tokenizer
            )

            model, optimizer, training_dataloader, scheduler = accelerator.prepare(
                model, optimizer, data_loader, scheduler
            )

            latest_model = 0
            update_in_epoch = False
            for epoch in range(train_config["epochs"]):
                progress_bar = tqdm(
                    range(len(data_loader)),
                    disable=not accelerator.is_main_process,
                )
                for batch in training_dataloader:
                    optimizer.zero_grad()
                    # check for model updates
                    if not update_in_epoch:
                        latest_version, weights_path = client.get_parameters(
                            train_config["task_id"]
                        )

                        if latest_version > latest_model:
                            model = merge_models(model, weights_path, 0.9)
                            model.to(device)
                            latest_model = latest_version
                            update_in_epoch = True
                            print("Weights updated", flush=True)

                    # This is specific for training transformer models
                    # TODO: allow for other models
                    inputs = batch
                    outputs = model(input_ids=inputs, labels=inputs)
                    loss = outputs["loss"]
                    # loss = loss_function(outputs, targets)
                    accelerator.backward(loss)
                    optimizer.step()
                    scheduler.step()
                    if accelerator.is_main_process:
                        progress_bar.update(1)
                if accelerator.is_main_process:
                    if epoch % train_config["checkpointing"] == 0:
                        # For testing purposes set to global!
                        result_path = os.path.join(
                            work_dir,
                            f"{train_config['task_id']}_{epoch}_global_weights.pt",
                        )
                        save_model(model, result_path)

                        # Once we notify the driver about the result file, ownership of that file
                        # is transferred to the driver.
                        client.set_task_status(
                            train_config["task_id"],
                            "Finished",
                            {
                                "epoch": epoch,
                                "path": result_path,
                            },
                        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", type=str, required=True)
    parser.add_argument("--work-dir", type=str, required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir)
