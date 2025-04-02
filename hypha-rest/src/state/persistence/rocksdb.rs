use std::{path::Path, str};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::StatePersistence;

/// RocksDBPersistence is a state persistence implementation using RocksDB.
pub struct RocksDBPersistence {
    db: rocksdb::DB,
}

impl RocksDBPersistence {
    pub fn try_new<P>(path: P) -> Result<Self, rocksdb::Error>
    where
        P: AsRef<Path>,
    {
        let db = rocksdb::DB::open_default(path)?;
        Ok(Self { db })
    }
}

impl StatePersistence for RocksDBPersistence {
    fn put<T>(&mut self, key: &str, value: T) -> Result<()>
    where
        T: Serialize,
    {
        let task_bytes = serde_json::to_vec(&value)?;
        self.db.put(key.as_bytes(), task_bytes)?;
        Ok(())
    }

    fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = self.db.get_pinned(key.as_bytes())?;
        match value {
            Some(value) => {
                let v: T = serde_json::from_slice(&value)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn delete<T>(&mut self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = self.db.get_pinned(key.as_bytes())?;
        match value {
            Some(value) => {
                self.db.delete(key.as_bytes())?;
                let v: T = serde_json::from_slice(&value)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn list<T>(&self, prefix: &str) -> Result<Vec<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut values = Vec::new();
        for iter in self.db.prefix_iterator(prefix.as_bytes()) {
            let (_, value) = iter?;
            let v: T = serde_json::from_slice(&value)?;
            values.push(v);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_rocksdb_persistence() {
        let path = tempdir().unwrap();
        let mut persistence = RocksDBPersistence::try_new(path).unwrap();

        let key = "test_key";
        let value = "test_value".to_string();

        persistence.put(key, value.clone()).unwrap();
        let retrieved_value: String = persistence.get(key).unwrap().unwrap();
        assert_eq!(retrieved_value, value);

        let _ = persistence.delete::<String>(key).unwrap();
        let deleted_value: Option<String> = persistence.get(key).unwrap();
        assert!(deleted_value.is_none());

        let prefix = "test";
        persistence
            .put("test_key1", "test_value1".to_string())
            .unwrap();
        persistence
            .put("test_key2", "test_value2".to_string())
            .unwrap();
        let list: Vec<String> = persistence.list(prefix).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], "test_value1");
        assert_eq!(list[1], "test_value2");
    }
}
