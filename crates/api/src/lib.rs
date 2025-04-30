use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Work(),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    WorkDone(),
}
