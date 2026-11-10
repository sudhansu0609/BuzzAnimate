// Copyright 2022 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use guillotiere::{AllocId, AtlasAllocator, size2};
use peniko::ImageData;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

const DEFAULT_ATLAS_SIZE: i32 = 1024;
const MAX_ATLAS_SIZE: i32 = 8192;
const EVICT_AFTER_GENERATIONS: u64 = 2;

#[derive(Default)]
pub struct Images<'a> {
    /// Width of the square image atlas texture.
    pub width: u32,
    /// Height of the square image atlas texture.
    pub height: u32,
    /// Number of resident images evicted during the current resolve pass.
    ///
    /// This is only used for renderer-side debug logging.
    pub evicted: usize,
    /// Images that must be uploaded in the current resolve pass, with atlas locations.
    pub images: &'a [(ImageData, u32, u32)],
}

#[derive(Clone)]
struct ResidentImage {
    image: ImageData,
    alloc_id: AllocId,
    x: u32,
    y: u32,
    dirty: bool,
    last_used_generation: u64,
}

pub(crate) struct ImageCache {
    atlas: AtlasAllocator,
    /// Maximum side length for the square image atlas texture.
    max_size: i32,
    /// Monotonic counter for resolve passes, used to track when resident images were last used.
    generation: u64,
    /// Number of resident images evicted during the current resolve pass.
    ///
    /// This is exposed through [`Images::evicted`] for renderer-side debug logging, and also
    /// prevents repeated stale-eviction scans during the same resolve pass.
    evicted_in_resolve: usize,
    /// Map from image blob id to atlas residency.
    map: HashMap<u64, ResidentImage>,
    /// Images that must be uploaded in the current resolve pass, with atlas locations.
    images: Vec<(ImageData, u32, u32)>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub(crate) fn new() -> Self {
        Self::new_with_sizes(DEFAULT_ATLAS_SIZE, MAX_ATLAS_SIZE)
    }

    fn new_with_sizes(initial_size: i32, max_size: i32) -> Self {
        Self {
            atlas: AtlasAllocator::new(size2(initial_size, initial_size)),
            max_size,
            generation: 0,
            evicted_in_resolve: 0,
            map: HashMap::default(),
            images: Vec::default(),
        }
    }

    pub(crate) fn begin_resolve(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.evicted_in_resolve = 0;
        self.images.clear();
    }

    pub(crate) fn restart_resolve_pass(&mut self) {
        self.images.clear();
        let previous_generation = self.generation.wrapping_sub(1);
        for resident in self.map.values_mut() {
            if resident.last_used_generation == self.generation {
                resident.last_used_generation = previous_generation;
            }
        }
    }

    pub(crate) fn images(&self) -> Images<'_> {
        Images {
            width: self.atlas.size().width as u32,
            height: self.atlas.size().height as u32,
            evicted: self.evicted_in_resolve,
            images: &self.images,
        }
    }

    pub(crate) fn bump_size(&mut self) -> bool {
        let mut new_size = self.atlas.size().width * 2;
        while new_size <= self.max_size {
            if self.repack_to_size(new_size) {
                self.images.clear();
                return true;
            }
            new_size *= 2;
        }
        false
    }

    pub(crate) fn get_or_insert(&mut self, image: &ImageData) -> Option<(u32, u32)> {
        match self.map.entry(image.data.id()) {
            Entry::Occupied(mut occupied) => {
                let resident = occupied.get_mut();
                let xy = (resident.x, resident.y);
                if resident.last_used_generation != self.generation {
                    resident.last_used_generation = self.generation;
                    if resident.dirty {
                        self.images
                            .push((resident.image.clone(), resident.x, resident.y));
                    }
                }
                Some(xy)
            }
            Entry::Vacant(vacant) => {
                let alloc = self
                    .atlas
                    .allocate(size2(image.width as _, image.height as _))?;
                let x = alloc.rectangle.min.x as u32;
                let y = alloc.rectangle.min.y as u32;
                let resident = ResidentImage {
                    image: image.clone(),
                    alloc_id: alloc.id,
                    x,
                    y,
                    dirty: true,
                    last_used_generation: self.generation,
                };
                self.images.push((image.clone(), x, y));
                vacant.insert(resident);
                Some((x, y))
            }
        }
    }

    pub(crate) fn finish_resolve(&mut self) {
        for resident in self.map.values_mut() {
            if resident.last_used_generation == self.generation {
                resident.dirty = false;
            }
        }
    }

    pub(crate) fn mark_dirty(&mut self, image: &ImageData) {
        if let Some(resident) = self.map.get_mut(&image.data.id()) {
            resident.dirty = true;
        }
    }

    pub(crate) fn can_fit_image(&self, image: &ImageData) -> bool {
        image.width <= self.atlas.size().width as u32
            && image.height <= self.atlas.size().height as u32
    }

    pub(crate) fn evict_stale_entries(&mut self) -> bool {
        if self.evicted_in_resolve != 0 {
            return false;
        }
        let Some(stale_before) = self.generation.checked_sub(EVICT_AFTER_GENERATIONS) else {
            return false;
        };
        for (_id, resident) in self
            .map
            .extract_if(|_, resident| resident.last_used_generation < stale_before)
        {
            self.atlas.deallocate(resident.alloc_id);
            self.evicted_in_resolve += 1;
        }
        self.evicted_in_resolve != 0
    }

    /// Evict the least recently used resident image that the current resolve pass has not
    /// asked for, so a new image can take its place *now*.
    ///
    /// [`Self::evict_stale_entries`] only frees images unused for
    /// [`EVICT_AFTER_GENERATIONS`] passes. With the atlas already at its maximum size that
    /// leaves a newly needed image unallocated — and so not drawn at all — for that many
    /// frames whenever the previous frame's images fill the atlas: a cut to a new full-frame
    /// background drew the stage's background colour for two frames, a white flash. This is
    /// the fallback for exactly that case. An image already used by this pass is never
    /// evicted, so nothing the frame has placed can lose its slot; returns `false` when every
    /// resident image is in use.
    pub(crate) fn evict_least_recent(&mut self) -> bool {
        let generation = self.generation;
        let Some(id) = self
            .map
            .iter()
            .filter(|(_, resident)| resident.last_used_generation != generation)
            .min_by_key(|(_, resident)| resident.last_used_generation)
            .map(|(id, _)| *id)
        else {
            return false;
        };
        if let Some(resident) = self.map.remove(&id) {
            self.atlas.deallocate(resident.alloc_id);
            self.evicted_in_resolve += 1;
        }
        true
    }

    fn repack_to_size(&mut self, size: i32) -> bool {
        let mut atlas = AtlasAllocator::new(size2(size, size));
        let mut entries: Vec<_> = self.map.iter().collect();
        entries.sort_by_key(|(id, _)| *id);
        let mut map = HashMap::with_capacity(self.map.len());
        for (id, resident) in entries {
            let Some(alloc) =
                atlas.allocate(size2(resident.image.width as _, resident.image.height as _))
            else {
                return false;
            };
            let mut resident = resident.clone();
            resident.alloc_id = alloc.id;
            resident.x = alloc.rectangle.min.x as u32;
            resident.y = alloc.rectangle.min.y as u32;
            resident.dirty = true;
            map.insert(*id, resident);
        }
        self.atlas = atlas;
        self.map = map;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::{Blob, ImageAlphaType, ImageFormat};
    use std::sync::Arc;

    fn image(id_byte: u8, width: u32, height: u32) -> ImageData {
        let len = (width * height * 4) as usize;
        ImageData {
            data: Blob::new(Arc::new(vec![id_byte; len])),
            format: ImageFormat::Rgba8,
            width,
            height,
            alpha_type: ImageAlphaType::Alpha,
        }
    }

    #[test]
    fn atlas_size_persists_after_growth() {
        let mut cache = ImageCache::new_with_sizes(16, 64);
        assert_eq!(cache.atlas.size().width, 16);
        assert!(cache.bump_size());
        assert_eq!(cache.atlas.size().width, 32);
        cache.begin_resolve();
        assert_eq!(cache.atlas.size().width, 32);
    }

    #[test]
    fn resident_entries_are_reused_across_resolves() {
        let mut cache = ImageCache::new_with_sizes(32, 64);
        let image = image(7, 8, 8);

        cache.begin_resolve();
        let first = cache.get_or_insert(&image).unwrap();
        assert_eq!(cache.images.len(), 1);
        cache.finish_resolve();

        cache.begin_resolve();
        let second = cache.get_or_insert(&image).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.images.len(), 0);
        assert_eq!(cache.map.len(), 1);
    }

    #[test]
    fn marked_dirty_resident_entries_are_uploaded_again() {
        let mut cache = ImageCache::new_with_sizes(32, 64);
        let image = image(7, 8, 8);

        cache.begin_resolve();
        let first = cache.get_or_insert(&image).unwrap();
        cache.finish_resolve();

        cache.mark_dirty(&image);
        cache.begin_resolve();
        let second = cache.get_or_insert(&image).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.images.len(), 1);
        assert_eq!(cache.images[0].0.data.id(), image.data.id());
        assert_eq!((cache.images[0].1, cache.images[0].2), first);
        cache.finish_resolve();

        cache.begin_resolve();
        assert_eq!(cache.get_or_insert(&image), Some(first));
        assert_eq!(cache.images.len(), 0);
    }

    #[test]
    fn marked_dirty_unused_entries_stay_dirty() {
        let mut cache = ImageCache::new_with_sizes(32, 64);
        let image = image(7, 8, 8);

        cache.begin_resolve();
        let xy = cache.get_or_insert(&image).unwrap();
        cache.finish_resolve();

        cache.mark_dirty(&image);
        cache.begin_resolve();
        cache.finish_resolve();

        cache.begin_resolve();
        assert_eq!(cache.get_or_insert(&image), Some(xy));
        assert_eq!(cache.images.len(), 1);
    }

    #[test]
    fn stale_entries_can_be_evicted_under_pressure() {
        let mut cache = ImageCache::new_with_sizes(16, 16);
        let image_a = image(1, 10, 10);
        let image_b = image(2, 10, 10);

        cache.begin_resolve();
        assert!(cache.get_or_insert(&image_a).is_some());

        cache.begin_resolve();

        cache.begin_resolve();
        cache.begin_resolve();
        assert!(cache.get_or_insert(&image_b).is_none());
        assert!(cache.evict_stale_entries());
        assert!(cache.get_or_insert(&image_b).is_some());
        assert!(!cache.map.contains_key(&image_a.data.id()));
    }

    #[test]
    fn stale_entries_are_evicted_at_most_once_per_resolve() {
        let mut cache = ImageCache::new_with_sizes(32, 32);
        let image_a = image(1, 8, 8);
        let image_b = image(2, 8, 8);

        cache.begin_resolve();
        assert!(cache.get_or_insert(&image_a).is_some());
        cache.finish_resolve();

        cache.begin_resolve();
        assert!(cache.get_or_insert(&image_b).is_some());
        cache.finish_resolve();

        cache.begin_resolve();
        cache.begin_resolve();
        cache.begin_resolve();

        assert!(cache.evict_stale_entries());
        assert!(!cache.evict_stale_entries());
    }

    /// The white flash: the atlas is at its maximum size and full of the previous frame's
    /// images, and the next frame needs a new one. It must fit on *this* frame, by evicting
    /// an image this frame does not use — not two frames later when that image is stale.
    #[test]
    fn a_full_atlas_makes_room_for_a_new_image_at_once() {
        let mut cache = ImageCache::new_with_sizes(16, 16);
        // Two 16x8 images fill a 16x16 atlas, as two 5120x2880 backgrounds fill 8192x8192.
        let a = image(1, 16, 8);
        let b = image(2, 16, 8);
        let c = image(3, 16, 8);

        cache.begin_resolve();
        assert!(cache.get_or_insert(&a).is_some());
        assert!(cache.get_or_insert(&b).is_some());
        cache.finish_resolve();

        // The next frame keeps B and cuts to C.
        cache.begin_resolve();
        assert!(cache.get_or_insert(&b).is_some());
        assert!(cache.get_or_insert(&c).is_none(), "the atlas is full");
        assert!(!cache.evict_stale_entries(), "A was used one frame ago: not stale yet");
        assert!(!cache.bump_size(), "the atlas is at its maximum size");
        assert!(cache.evict_least_recent());
        assert!(cache.get_or_insert(&c).is_some(), "C is drawn on this frame");
        assert!(!cache.map.contains_key(&a.data.id()), "A, unused this frame, made way");
        assert!(cache.map.contains_key(&b.data.id()), "B, used this frame, kept its slot");
        assert_eq!(cache.images.last().map(|i| i.0.data.id()), Some(c.data.id()), "C is uploaded");
    }

    #[test]
    fn an_image_used_by_this_frame_is_never_evicted() {
        let mut cache = ImageCache::new_with_sizes(16, 16);
        let (a, b, c) = (image(1, 16, 8), image(2, 16, 8), image(3, 16, 8));
        cache.begin_resolve();
        assert!(cache.get_or_insert(&a).is_some());
        assert!(cache.get_or_insert(&b).is_some());
        // The same frame asks for a third: there is nothing it may take.
        assert!(cache.get_or_insert(&c).is_none());
        assert!(!cache.evict_least_recent());
        assert_eq!(cache.map.len(), 2);
    }

    #[test]
    fn failed_repack_leaves_existing_residency_unchanged() {
        let mut cache = ImageCache::new_with_sizes(16, 16);
        let image = image(1, 12, 12);

        cache.begin_resolve();
        let xy = cache.get_or_insert(&image).unwrap();

        assert!(!cache.repack_to_size(8));
        assert_eq!(cache.atlas.size().width, 16);
        assert_eq!(cache.get_or_insert(&image), Some(xy));
        assert_eq!(cache.map.len(), 1);
    }
}
