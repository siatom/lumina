//! Process-wide LRU cache for generated thumbnails.
//!
//! Thumbnail generation is the expensive part of opening a folder (a full
//! decode per tile), and the same tiles are requested again and again while the
//! user scrolls or revisits a folder. Caching the encoded PNG bytes makes the
//! second and later requests effectively free.
//!
//! The cache is keyed by `(path, revision, max)` — `revision` is the frontend's
//! `<modified_ms>-<file_size>` token, so an edited file is never served from a
//! stale entry. The mutex is only held for the map lookup / insert; decoding
//! happens outside it, so concurrent requests are not serialised.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use lru::LruCache;

use crate::error::AppError;
use crate::imaging;

/// Identifies one cached thumbnail: the file, its revision and the target size.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    pub path: String,
    pub revision: String,
    pub max: u32,
}

/// Bounded, thread-safe LRU of encoded PNG thumbnails.
pub struct ThumbCache {
    inner: Mutex<LruCache<ThumbKey, Arc<Vec<u8>>>>,
}

impl ThumbCache {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("capacity is clamped to >= 1");
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    fn get(&self, key: &ThumbKey) -> Option<Arc<Vec<u8>>> {
        self.inner.lock().ok()?.get(key).cloned()
    }

    fn put(&self, key: ThumbKey, bytes: Arc<Vec<u8>>) {
        if let Ok(mut cache) = self.inner.lock() {
            cache.put(key, bytes);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().map(|c| c.len()).unwrap_or(0)
    }
}

/// The process-wide thumbnail cache (512 entries covers a few hundred tiles).
pub fn thumbnail_cache() -> &'static ThumbCache {
    static CACHE: OnceLock<ThumbCache> = OnceLock::new();
    CACHE.get_or_init(|| ThumbCache::new(512))
}

/// Return the PNG thumbnail for `path` at `max` px, decoding and caching it on a
/// miss. `revision` is the cache-busting token (`<modified_ms>-<file_size>`).
pub fn thumbnail_png(path: &str, max: u32, revision: &str) -> Result<Arc<Vec<u8>>, AppError> {
    let key = ThumbKey {
        path: path.to_string(),
        revision: revision.to_string(),
        max,
    };
    let cache = thumbnail_cache();
    if let Some(bytes) = cache.get(&key) {
        return Ok(bytes);
    }
    // Decode without holding the lock so concurrent tiles stay parallel.
    let bytes = Arc::new(imaging::make_thumbnail(Path::new(path), max)?);
    cache.put(key, Arc::clone(&bytes));
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_and_evicts() {
        let cache = ThumbCache::new(2);
        let key = |p: &str| ThumbKey {
            path: p.to_string(),
            revision: "1-1".to_string(),
            max: 64,
        };

        cache.put(key("a"), Arc::new(vec![1]));
        cache.put(key("b"), Arc::new(vec![2]));
        assert_eq!(cache.get(&key("a")).unwrap().as_slice(), &[1]);

        // Inserting a third entry evicts the least-recently used ("b", since
        // "a" was just touched by `get`).
        cache.put(key("c"), Arc::new(vec![3]));
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("b")).is_none());
        assert!(cache.get(&key("a")).is_some());
        assert!(cache.get(&key("c")).is_some());
    }
}
