use std::fmt;

use serde::{Deserialize, Serialize};
use tracing::level_filters::LevelFilter;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Verbosity(LevelFilter);

impl Verbosity {
    pub fn as_filter(self) -> LevelFilter {
        self.0
    }
}

impl Default for Verbosity {
    fn default() -> Self {
        Self(LevelFilter::DEBUG)
    }
}

impl fmt::Display for Verbosity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Verbosity> for LevelFilter {
    fn from(level: Verbosity) -> Self {
        level.0
    }
}

impl From<Verbosity> for String {
    fn from(level: Verbosity) -> Self {
        level.0.to_string()
    }
}

impl TryFrom<String> for Verbosity {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("telemetry verbosity must not be empty".to_string());
        }

        let normalized = trimmed.to_ascii_lowercase();
        let level = match normalized.as_str() {
            "off" => LevelFilter::OFF,
            "error" => LevelFilter::ERROR,
            "warn" => LevelFilter::WARN,
            "info" => LevelFilter::INFO,
            "debug" => LevelFilter::DEBUG,
            "trace" => LevelFilter::TRACE,
            _ => {
                return Err(format!(
                    "invalid telemetry verbosity `{trimmed}`: expected one of \"off\", \"error\", \"warn\", \"info\", \"debug\", \"trace\""
                ));
            }
        };

        Ok(Verbosity(level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_debug() {
        assert_eq!(LevelFilter::from(Verbosity::default()), LevelFilter::DEBUG);
    }

    #[test]
    fn parses_case_insensitive_levels() {
        let level = Verbosity::try_from("INFO".to_string()).unwrap();
        assert_eq!(LevelFilter::from(level), LevelFilter::INFO);
    }

    #[test]
    fn rejects_invalid_levels() {
        let err = Verbosity::try_from("verbose".to_string()).unwrap_err();
        assert!(err.contains("invalid telemetry verbosity"));
    }
}
