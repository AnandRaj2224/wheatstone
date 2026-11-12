// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub generation: u64,
    pub page_num: usize,
    pub scale_key: u32,
}

pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024; // 128 MiB default byte budget

fn texture_byte_size(texture: &gdk::MemoryTexture) -> usize {
    texture.width() as usize * texture.height() as usize * 4
}

/// LRU cache for texture objects to limit memory usage
pub struct TextureCache {
    cache: RefCell<HashMap<TextureKey, gdk::MemoryTexture>>,
    access_order: RefCell<VecDeque<TextureKey>>,
    max_size: usize,
    max_bytes: usize,
    current_bytes: Cell<usize>,
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl TextureCache {
    pub fn new(max_size: usize) -> Self {
        Self::with_limits(max_size, DEFAULT_MAX_BYTES)
    }

    pub fn with_limits(max_size: usize, max_bytes: usize) -> Self {
        #[cfg(debug_assertions)]
        println!(
            "TextureCache: Created with max size {} pages, max bytes {} ({} MiB)",
            max_size,
            max_bytes,
            max_bytes / (1024 * 1024)
        );

        Self {
            cache: RefCell::new(HashMap::new()),
            access_order: RefCell::new(VecDeque::new()),
            max_size,
            max_bytes,
            current_bytes: Cell::new(0),
            hits: Cell::new(0),
            misses: Cell::new(0),
        }
    }

    /// Get a texture from cache, marking it as recently accessed
    pub fn get(&self, generation: u64, page_num: usize, scale_key: u32) -> Option<gdk::MemoryTexture> {
        let key = TextureKey {
            generation,
            page_num,
            scale_key,
        };
        let texture = self.cache.borrow().get(&key).cloned();
        if let Some(texture) = texture {
            self.hits.set(self.hits.get() + 1);
            self.mark_accessed(key);

            #[cfg(debug_assertions)]
            println!(
                "TextureCache: Cache hit for page {} (gen {}, scale {})",
                page_num, generation, scale_key
            );

            Some(texture)
        } else {
            self.misses.set(self.misses.get() + 1);
            #[cfg(debug_assertions)]
            println!(
                "TextureCache: Cache miss for page {} (gen {}, scale {})",
                page_num, generation, scale_key
            );

            None
        }
    }

    /// Insert a texture into cache, evicting LRU if necessary
    pub fn insert(&self, generation: u64, page_num: usize, scale_key: u32, texture: gdk::MemoryTexture) {
        let key = TextureKey {
            generation,
            page_num,
            scale_key,
        };
        let tex_bytes = texture_byte_size(&texture);
        let mut cache = self.cache.borrow_mut();

        // If this page already exists at this scale, remove it first (to update access order and byte size)
        if let Some(old_tex) = cache.remove(&key) {
            let old_bytes = texture_byte_size(&old_tex);
            self.current_bytes.set(self.current_bytes.get().saturating_sub(old_bytes));
            drop(old_tex);
            let mut order = self.access_order.borrow_mut();
            order.retain(|&k| k != key);
        }

        // Evict if we're at capacity either by page count or byte budget
        while !cache.is_empty()
            && (cache.len() >= self.max_size
                || self.current_bytes.get() + tex_bytes > self.max_bytes)
        {
            self.evict_lru(&mut cache);
        }

        self.current_bytes.set(self.current_bytes.get() + tex_bytes);
        cache.insert(key, texture);
        drop(cache); // Release borrow before mark_accessed

        self.mark_accessed(key);

        #[cfg(debug_assertions)]
        println!(
            "TextureCache: Inserted page {} gen {} scale {} (size: {}, bytes: {}/{})",
            page_num,
            generation,
            scale_key,
            self.cache.borrow().len(),
            self.current_bytes.get(),
            self.max_bytes
        );
    }

    /// Clear all cached textures
    pub fn clear(&self) {
        #[cfg(debug_assertions)]
        {
            let size = self.cache.borrow().len();
            if size > 0 {
                println!("TextureCache: Clearing {} textures ({} bytes)", size, self.current_bytes.get());
            }
        }

        // Take ownership and explicitly drop each texture
        let textures: Vec<_> = self.cache.borrow_mut().drain().collect();
        self.current_bytes.set(0);

        // Track drops
        #[cfg(debug_assertions)]
        crate::memory_tracking::MEMORY_COUNTERS
            .textures_dropped
            .fetch_add(textures.len(), std::sync::atomic::Ordering::Relaxed);

        // Drop each one explicitly
        for (_key, texture) in textures {
            drop(texture);
        }

        // Clear the access order
        self.access_order.borrow_mut().clear();
    }

    /// Remove a specific page from cache across all scales
    pub fn remove(&self, generation: u64, page_num: usize) {
        let mut cache = self.cache.borrow_mut();
        let keys_to_remove: Vec<TextureKey> = cache
            .keys()
            .filter(|k| k.generation == generation && k.page_num == page_num)
            .copied()
            .collect();

        let mut dropped = 0;
        for key in &keys_to_remove {
            if let Some(texture) = cache.remove(key) {
                let bytes = texture_byte_size(&texture);
                self.current_bytes.set(self.current_bytes.get().saturating_sub(bytes));
                drop(texture);
                dropped += 1;
            }
        }

        if dropped > 0 {
            let mut order = self.access_order.borrow_mut();
            order.retain(|k| !(k.generation == generation && k.page_num == page_num));

            #[cfg(debug_assertions)]
            crate::memory_tracking::MEMORY_COUNTERS
                .textures_dropped
                .fetch_add(dropped, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Remove all textures belonging to an older generation
    pub fn remove_generation(&self, generation: u64) {
        let mut cache = self.cache.borrow_mut();
        let keys_to_remove: Vec<TextureKey> = cache
            .keys()
            .filter(|k| k.generation == generation)
            .copied()
            .collect();

        let mut dropped = 0;
        for key in keys_to_remove {
            if let Some(tex) = cache.remove(&key) {
                let bytes = texture_byte_size(&tex);
                self.current_bytes.set(self.current_bytes.get().saturating_sub(bytes));
                drop(tex);
                dropped += 1;
            }
        }
        let mut order = self.access_order.borrow_mut();
        order.retain(|k| k.generation != generation);

        if dropped > 0 {
            #[cfg(debug_assertions)]
            crate::memory_tracking::MEMORY_COUNTERS
                .textures_dropped
                .fetch_add(dropped, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Get current cache size (for debugging)
    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.cache.borrow().len()
    }

    pub fn current_bytes(&self) -> usize {
        self.current_bytes.get()
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn hits(&self) -> u64 {
        self.hits.get()
    }

    pub fn misses(&self) -> u64 {
        self.misses.get()
    }

    /// Mark a page as recently accessed (moves to end of queue)
    fn mark_accessed(&self, key: TextureKey) {
        let mut order = self.access_order.borrow_mut();
        // Remove if already present
        order.retain(|&k| k != key);
        // Add to back (most recent)
        order.push_back(key);
    }

    /// Evict the least recently used page
    fn evict_lru(&self, cache: &mut HashMap<TextureKey, gdk::MemoryTexture>) {
        let mut order = self.access_order.borrow_mut();

        while let Some(key) = order.pop_front() {
            if let Some(texture) = cache.remove(&key) {
                let bytes = texture_byte_size(&texture);
                self.current_bytes.set(self.current_bytes.get().saturating_sub(bytes));

                #[cfg(debug_assertions)]
                println!(
                    "TextureCache: Evicted and dropping LRU page {} (gen {}, scale {}, {} bytes)",
                    key.page_num, key.generation, key.scale_key, bytes
                );

                #[cfg(debug_assertions)]
                crate::memory_tracking::MEMORY_COUNTERS
                    .textures_dropped
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                drop(texture);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::glib::Bytes;

    fn make_test_texture() -> gdk::MemoryTexture {
        let pixels = vec![255u8; 16]; // 2x2 RGBA
        let bytes = Bytes::from(&pixels);
        gdk::MemoryTexture::new(2, 2, gdk::MemoryFormat::R8g8b8a8, &bytes, 8)
    }

    #[test]
    fn test_texture_cache_generation_isolation() {
        let cache = TextureCache::new(5);
        let tex = make_test_texture();

        cache.insert(1, 0, 100, tex.clone());
        assert_eq!(cache.size(), 1);

        // Access with wrong generation -> miss!
        assert!(cache.get(2, 0, 100).is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // Access with correct generation -> hit!
        assert!(cache.get(1, 0, 100).is_some());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn test_texture_cache_zoom_invalidation_without_doc_switch() {
        let cache = TextureCache::new(5);
        let tex_100 = make_test_texture();
        let tex_150 = make_test_texture();

        // Document generation is 1. Page 0 rendered at zoom scale 100 (e.g. 1.0x)
        cache.insert(1, 0, 100, tex_100.clone());
        assert_eq!(cache.size(), 1);

        // Access with matching scale 100 -> hit!
        assert!(cache.get(1, 0, 100).is_some());
        assert_eq!(cache.hits(), 1);

        // Zoom changed to scale 150 (1.5x) without document switch (generation stays 1).
        // Access with new scale 150 -> cache miss! Stale resolution is never served.
        assert!(cache.get(1, 0, 150).is_none());
        assert_eq!(cache.misses(), 1);

        // Insert newly rendered texture at scale 150
        cache.insert(1, 0, 150, tex_150.clone());
        assert_eq!(cache.size(), 2);
        assert!(cache.get(1, 0, 150).is_some());

        // When settle_render_scale runs upon zoom change, cache.clear() reliably wipes all scales
        cache.clear();
        assert_eq!(cache.size(), 0);
        assert!(cache.get(1, 0, 100).is_none());
        assert!(cache.get(1, 0, 150).is_none());
    }

    #[test]
    fn test_texture_cache_lru_eviction() {
        let cache = TextureCache::new(2);
        let tex = make_test_texture();

        cache.insert(1, 0, 100, tex.clone());
        cache.insert(1, 1, 100, tex.clone());
        assert_eq!(cache.size(), 2);

        // Access page 0 so page 1 becomes LRU
        assert!(cache.get(1, 0, 100).is_some());

        // Insert page 2 -> should evict page 1
        cache.insert(1, 2, 100, tex.clone());
        assert_eq!(cache.size(), 2);

        assert!(cache.get(1, 0, 100).is_some(), "Page 0 should remain");
        assert!(cache.get(1, 2, 100).is_some(), "Page 2 should remain");
        assert!(cache.get(1, 1, 100).is_none(), "Page 1 should have been evicted");
    }

    #[test]
    fn test_texture_cache_remove_generation() {
        let cache = TextureCache::new(5);
        let tex = make_test_texture();

        cache.insert(1, 0, 100, tex.clone());
        cache.insert(1, 1, 100, tex.clone());
        cache.insert(2, 0, 100, tex.clone());
        assert_eq!(cache.size(), 3);

        cache.remove_generation(1);
        assert_eq!(cache.size(), 1);

        assert!(cache.get(1, 0, 100).is_none());
        assert!(cache.get(1, 1, 100).is_none());
        assert!(cache.get(2, 0, 100).is_some());
    }

    #[test]
    fn test_texture_cache_byte_budget_eviction() {
        // Each test texture is 2x2 RGBA = 16 bytes.
        // Limit max_bytes to 32 bytes (holds at most 2 textures even if max_size is 10).
        let cache = TextureCache::with_limits(10, 32);
        let tex = make_test_texture();

        cache.insert(1, 0, 100, tex.clone());
        cache.insert(1, 1, 100, tex.clone());
        assert_eq!(cache.size(), 2);
        assert_eq!(cache.current_bytes(), 32);

        // Access page 0 to make page 1 LRU
        assert!(cache.get(1, 0, 100).is_some());

        // Insert page 2 (16 bytes) -> exceeds 32 byte budget -> evicts page 1
        cache.insert(1, 2, 100, tex.clone());
        assert_eq!(cache.size(), 2);
        assert_eq!(cache.current_bytes(), 32);
        assert!(cache.get(1, 0, 100).is_some(), "Page 0 should remain");
        assert!(cache.get(1, 2, 100).is_some(), "Page 2 should remain");
        assert!(cache.get(1, 1, 100).is_none(), "Page 1 should have been evicted by byte budget");
    }
}
