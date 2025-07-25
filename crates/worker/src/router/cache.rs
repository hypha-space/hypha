use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::fs;
use tracing::{debug, warn};

/// Simple cache management for pulled content
pub struct CacheManager {
    base_dir: PathBuf,
    max_age: Duration,
}

impl CacheManager {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            max_age: Duration::from_secs(86400), // 24 hours default
        }
    }
    
    /// Check if a cached item exists and is still valid
    pub async fn is_cached(&self, key: &str) -> bool {
        let cache_path = self.cache_path(key);
        
        if !cache_path.exists() {
            return false;
        }
        
        // Check age
        match fs::metadata(&cache_path).await {
            Ok(metadata) => {
                if let Ok(modified) = metadata.modified() {
                    let age = SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or(Duration::MAX);
                    
                    if age < self.max_age {
                        debug!(key = %key, age_secs = age.as_secs(), "Cache hit");
                        true
                    } else {
                        debug!(key = %key, age_secs = age.as_secs(), "Cache expired");
                        false
                    }
                } else {
                    // Can't determine age, assume valid
                    true
                }
            }
            Err(e) => {
                warn!(key = %key, error = ?e, "Failed to check cache metadata");
                false
            }
        }
    }
    
    /// Get the cache path for a given key
    pub fn cache_path(&self, key: &str) -> PathBuf {
        // Simple hash-based path to avoid filesystem issues with special characters
        let hash = calculate_hash(key);
        let prefix = &hash[..2]; // Use first 2 chars as directory prefix
        
        self.base_dir.join(prefix).join(hash)
    }
    
    /// Clean up expired cache entries
    pub async fn cleanup(&self) -> Result<usize, std::io::Error> {
        let mut removed = 0;
        
        let mut entries = fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    if let Ok(modified) = metadata.modified() {
                        let age = SystemTime::now()
                            .duration_since(modified)
                            .unwrap_or(Duration::ZERO);
                        
                        if age > self.max_age {
                            if let Err(e) = fs::remove_file(entry.path()).await {
                                warn!(path = ?entry.path(), error = ?e, "Failed to remove expired cache entry");
                            } else {
                                removed += 1;
                            }
                        }
                    }
                }
            }
        }
        
        debug!(removed = removed, "Cleaned up expired cache entries");
        Ok(removed)
    }
}

fn calculate_hash(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_cache_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path());
        
        let path1 = cache.cache_path("test_key_1");
        let path2 = cache.cache_path("test_key_2");
        let path3 = cache.cache_path("test_key_1"); // Same as path1
        
        assert_ne!(path1, path2);
        assert_eq!(path1, path3);
    }
    
    #[tokio::test]
    async fn test_cache_expiry() {
        let temp_dir = TempDir::new().unwrap();
        let mut cache = CacheManager::new(temp_dir.path());
        cache.max_age = Duration::from_millis(100); // Short expiry for testing
        
        let cache_path = cache.cache_path("test_key");
        
        // Create parent directory
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).await.unwrap();
        }
        
        // Create cache file
        fs::write(&cache_path, "cached content").await.unwrap();
        
        // Should be cached initially
        assert!(cache.is_cached("test_key").await);
        
        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(150)).await;
        
        // Should be expired now
        assert!(!cache.is_cached("test_key").await);
    }
}