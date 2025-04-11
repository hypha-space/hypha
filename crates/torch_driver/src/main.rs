use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tempdir::TempDir;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct TrainingConfig {
    model: String,
    dataset: String,
    epochs: u32,
    batch_size: u32,
    learning_rate: f32,
    learning_rate_scheduler: String,
    optimizer: String,
    checkpointing: i32,
}

fn main() {
    let config_file = "test.yaml";
    let py_script = "python/training.py";
    let working_dir = TempDir::new(Uuid::new_v4().to_string().as_str()).unwrap();
    let training_config = TrainingConfig {
        model: format!("EleutherAI/gpt-neo-125m"),
        dataset: format!("datablations/c4-filter-small"),
        epochs: 2,
        batch_size: 4,
        learning_rate: 1e-5,
        learning_rate_scheduler: format!(""),
        optimizer: format!("AdamW"),
        checkpointing: 1,
    };
    let buffer = File::create(format!(
        "{}/{}",
        working_dir.path().to_str().unwrap(),
        "training_config.json"
    ))
    .unwrap();
    serde_json::to_writer(&buffer, &training_config).unwrap();

    let mut accelerate_process = Command::new("uv")
        .arg("run")
        .arg("accelerate")
        .arg("launch")
        .arg("--config-file")
        .arg(config_file)
        .arg(py_script)
        .arg("--working_dir")
        .arg(working_dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = accelerate_process.stdout.take().unwrap();

    // Stream output.
    let lines = BufReader::new(stdout).lines();
    for line in lines {
        println!("{}", line.unwrap());
    }
}
