use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::models::FolderMetadata;

#[derive(Debug)]
struct CacheEntry {
    value: FolderMetadata,
    prev: Option<String>,
    next: Option<String>,
}

pub struct LruFolderCache {
    capacity: usize,
    cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
    head: Arc<Mutex<Option<String>>>,
    tail: Arc<Mutex<Option<String>>>,
    size: Arc<Mutex<usize>>,
}

impl LruFolderCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cache: Arc::new(Mutex::new(HashMap::new())),
            head: Arc::new(Mutex::new(None)),
            tail: Arc::new(Mutex::new(None)),
            size: Arc::new(Mutex::new(0)),
        }
    }

    pub fn get(&self, key: &str) -> Option<FolderMetadata> {
        let mut cache = self.cache.lock().unwrap();

        if let Some(entry) = cache.get(key) {
            let value = entry.value.clone();
            drop(cache);

            // Move to front
            self.move_to_front(key);
            Some(value)
        } else {
            None
        }
    }

    pub fn insert(&self, key: String, value: FolderMetadata) {
        let mut cache = self.cache.lock().unwrap();
        let mut size = self.size.lock().unwrap();

        if cache.contains_key(&key) {
            // Update existing entry
            if let Some(entry) = cache.get_mut(&key) {
                entry.value = value;
            }
            drop(cache);
            drop(size);
            self.move_to_front(&key);
            return;
        }

        // Check if we need to evict
        if *size >= self.capacity {
            drop(cache);
            drop(size);
            self.evict_lru();
            cache = self.cache.lock().unwrap();
            size = self.size.lock().unwrap();
        }

        // Insert new entry
        let entry = CacheEntry {
            value,
            prev: None,
            next: None,
        };

        cache.insert(key.clone(), entry);
        *size += 1;
        drop(cache);
        drop(size);

        self.add_to_front(&key);
    }

    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        let mut head = self.head.lock().unwrap();
        let mut tail = self.tail.lock().unwrap();
        let mut size = self.size.lock().unwrap();

        cache.clear();
        *head = None;
        *tail = None;
        *size = 0;

        debug!("Folder cache cleared");
    }

    pub fn len(&self) -> usize {
        *self.size.lock().unwrap()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn move_to_front(&self, key: &str) {
        self.remove_from_list(key);
        self.add_to_front(key);
    }

    fn add_to_front(&self, key: &str) {
        let mut cache = self.cache.lock().unwrap();
        let mut head = self.head.lock().unwrap();

        if let Some(current_head) = head.as_ref() {
            if let Some(head_entry) = cache.get_mut(current_head) {
                head_entry.prev = Some(key.to_string());
            }
        } else {
            // First entry, also set as tail
            let mut tail = self.tail.lock().unwrap();
            *tail = Some(key.to_string());
        }

        if let Some(entry) = cache.get_mut(key) {
            entry.next = head.clone();
            entry.prev = None;
        }

        *head = Some(key.to_string());
    }

    fn remove_from_list(&self, key: &str) {
        let mut cache = self.cache.lock().unwrap();
        let mut head = self.head.lock().unwrap();
        let mut tail = self.tail.lock().unwrap();

        if let Some(entry) = cache.get(key) {
            let prev = entry.prev.clone();
            let next = entry.next.clone();

            // Update previous node
            if let Some(prev_key) = &prev {
                if let Some(prev_entry) = cache.get_mut(prev_key) {
                    prev_entry.next = next.clone();
                }
            } else {
                // This was the head
                *head = next.clone();
            }

            // Update next node
            if let Some(next_key) = &next {
                if let Some(next_entry) = cache.get_mut(next_key) {
                    next_entry.prev = prev.clone();
                }
            } else {
                // This was the tail
                *tail = prev.clone();
            }
        }
    }

    fn evict_lru(&self) {
        let tail = self.tail.lock().unwrap();

        if let Some(tail_key) = tail.as_ref() {
            let key_to_remove = tail_key.clone();
            drop(tail);

            self.remove_from_list(&key_to_remove);

            let mut cache = self.cache.lock().unwrap();
            let mut size = self.size.lock().unwrap();

            cache.remove(&key_to_remove);
            *size -= 1;

            debug!("Evicted folder from cache: {}", key_to_remove);
        }
    }
}

impl Clone for LruFolderCache {
    fn clone(&self) -> Self {
        Self {
            capacity: self.capacity,
            cache: Arc::clone(&self.cache),
            head: Arc::clone(&self.head),
            tail: Arc::clone(&self.tail),
            size: Arc::clone(&self.size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FolderMetadata;

    #[test]
    fn test_lru_cache_basic_operations() {
        let cache = LruFolderCache::new(2);

        let folder1 = FolderMetadata {
            id: "1".to_string(),
            name: "Folder1".to_string(),
            parents: None,
        };

        let folder2 = FolderMetadata {
            id: "2".to_string(),
            name: "Folder2".to_string(),
            parents: None,
        };

        // Insert items
        cache.insert("1".to_string(), folder1.clone());
        cache.insert("2".to_string(), folder2.clone());

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("1").unwrap().name, "Folder1");
        assert_eq!(cache.get("2").unwrap().name, "Folder2");
    }

    #[test]
    fn test_lru_eviction() {
        let cache = LruFolderCache::new(2);

        let folder1 = FolderMetadata {
            id: "1".to_string(),
            name: "Folder1".to_string(),
            parents: None,
        };

        let folder2 = FolderMetadata {
            id: "2".to_string(),
            name: "Folder2".to_string(),
            parents: None,
        };

        let folder3 = FolderMetadata {
            id: "3".to_string(),
            name: "Folder3".to_string(),
            parents: None,
        };

        // Fill cache
        cache.insert("1".to_string(), folder1.clone());
        cache.insert("2".to_string(), folder2.clone());

        // Access folder1 to make it recently used
        let _ = cache.get("1");

        // Insert folder3, should evict folder2
        cache.insert("3".to_string(), folder3.clone());

        assert_eq!(cache.len(), 2);
        assert!(cache.get("1").is_some());
        assert!(cache.get("2").is_none()); // Should be evicted
        assert!(cache.get("3").is_some());
    }

    #[test]
    fn test_cache_clear() {
        let cache = LruFolderCache::new(10);

        let folder = FolderMetadata {
            id: "1".to_string(),
            name: "Folder1".to_string(),
            parents: None,
        };

        cache.insert("1".to_string(), folder);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }
}
