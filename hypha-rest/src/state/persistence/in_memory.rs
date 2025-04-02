use std::collections::HashMap;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::StatePersistence;

/// InMemoryPersistence is a state persistence implementation using an in-memory HashMap.
pub struct InMemoryPersistence {
    data: HashMap<String, Value>,
}

impl InMemoryPersistence {
    #[cfg(test)]
    pub fn new() -> Self {
        InMemoryPersistence {
            data: HashMap::new(),
        }
    }
}

impl StatePersistence for InMemoryPersistence {
    fn put<T>(&mut self, key: &str, value: T) -> Result<()>
    where
        T: Serialize,
    {
        self.data
            .insert(key.to_string(), serde_json::to_value(value)?);
        Ok(())
    }

    fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(serde_json::from_value(
            self.data.get(key).ok_or(anyhow!("unknown key"))?.clone(),
        )?)
    }

    fn delete<T>(&mut self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        if let Some(value) = self.data.remove(key) {
            Ok(Some(serde_json::from_value(value)?))
        } else {
            Ok(None)
        }
    }

    fn list<T>(&self, prefix: &str) -> Result<Vec<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(self
            .data
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(_, v)| serde_json::from_value(v.clone()))
            .collect::<Result<Vec<_>, _>>()?)
    }
}
