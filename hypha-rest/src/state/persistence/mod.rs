use anyhow::Result;

#[cfg(test)]
pub use in_memory::InMemoryPersistence;
pub use rocksdb::RocksDBPersistence;
use serde::{Deserialize, Serialize};

mod in_memory;
mod rocksdb;

/// Persistence trait for storing and retrieving resources.
/// We assume that implementations of this trait will be "fast",
/// i.e. using blocking operations is not a concern here.
/// TODO: Create an 'Error' struct instead of using 'anyhow::Error'.
pub trait StatePersistence {
    fn put<T>(&mut self, key: &str, value: T) -> Result<()>
    where
        T: Serialize;
    fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>;
    fn delete<T>(&mut self, key: &str) -> Result<Option<T>>
    where
        T: for<'de> Deserialize<'de>;
    fn list<T>(&self, prefix: &str) -> Result<Vec<T>>
    where
        T: for<'de> Deserialize<'de>;
}
