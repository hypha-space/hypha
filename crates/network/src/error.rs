use std::{error::Error, fmt::Display};

#[derive(Debug)]
pub enum HyphaError {
    DialError(String),
    SwarmError(String),
}

impl Error for HyphaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

impl Display for HyphaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DialError(msg) => write!(f, "Dial error: {msg}"),
            Self::SwarmError(msg) => write!(f, "Swarm error: {msg}"),
        }
    }
}
