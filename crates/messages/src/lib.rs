use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Work(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    WorkDone(),
    Acknowledged(),
    Error(String),
}
