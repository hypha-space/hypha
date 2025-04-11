import torch
import torch.utils.data
from accelerate import Accelerator
from datasets import load_dataset
from tqdm import tqdm
from transformers import GPT2Tokenizer, GPTNeoForCausalLM
import argparse
import json
from safetensors.torch import save_model, safe_open
import os


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
            print(name, "updated")
            state_dict[name] += alpha * ( b.get_tensor(name) - state_dict[name])
    a.load_state_dict(state_dict)
    return a

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


def main(work_dir: str):
    # Load config file
    with open(work_dir + "/training_config.json") as file:
        train_config = json.load(file)

    accelerator = Accelerator(work_dir)
    device = accelerator.device

    model, tokenizer = get_model(train_config["model"])
    optimizer = get_optimizer(
        train_config["optimizer"], train_config["learning_rate"], model.parameters()
    )

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
    
    latest_model = -1
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
                files = os.listdir(work_dir)
                latest_version = max([int(f.split("_")[0]) for f in files if "global" in f] or [-1])
                if  latest_version > latest_model:
                    weights_path = f"{work_dir}/{latest_version}_global_weights.pt"
                    model = merge_models(model, weights_path, .9)
                    model.to(device)
                    latest_model = latest_version
                    update_in_epoch = True
                    print("Weights updated")

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
                save_model(model, f"{work_dir}/{epoch}_global_weights.pt")
            
if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--working_dir", help="the workling dir that the training will use", type=str
    )
    args = parser.parse_args()
    if not args.working_dir:
        raise RuntimeError("A 'working_dir' was expected but not provided.")
    main(args.working_dir)
