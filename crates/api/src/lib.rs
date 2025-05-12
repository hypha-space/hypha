use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Work(),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    WorkDone(),
}
