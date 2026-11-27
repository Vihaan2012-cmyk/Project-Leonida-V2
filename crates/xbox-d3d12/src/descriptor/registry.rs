//! The lookup tables translated shaders find their bindless slots in: every image descriptor (T#) and sampler
//! descriptor (S#) the game can hand a shader is registered here under its own words, and gets a slot.
//!
//! The renderer keeps one `VkImageView` and one `VkSampler` for each slot index, built from the decoded
//! descriptor kept here, and uploads the tables (`tables`) to the storage buffers at set 1 bindings 0 and 1
//! (`shader_translate::images`). Slot 0 is the fallback every miss reads.

use std::collections::HashMap;

use shader_translate::images::{self, Number, TableError};

use super::gfx10::{self, Image, ImageWords, Sampler, SamplerWords};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RegistryError {
    #[error("{0:08x?} is not an image descriptor")]
    NotAnImage(ImageWords),
    #[error("lookup table: {0}")]
    Table(#[from] TableError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    pub index: u32,
    /// The descriptor had no slot before; the renderer has a view or sampler to make for it.
    pub new: bool,
}

/// A registered image descriptor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureEntry {
    pub words: ImageWords,
    pub image: Image,
    pub number: Number,
}

/// A registered sampler descriptor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerEntry {
    pub words: SamplerWords,
    pub sampler: Sampler,
}

/// The table words to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tables {
    pub textures: Vec<u32>,
    pub samplers: Vec<u32>,
    pub version: u64,
}

struct Slots<T> {
    by_key: HashMap<[u32; 8], u32>,
    entries: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Default for Slots<T> {
    fn default() -> Slots<T> {
        Slots {
            by_key: HashMap::new(),
            entries: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Slots<T> {
    /// Indices start at 1: 0 is the fallback.
    fn insert(&mut self, key: [u32; 8], entry: T) -> Registration {
        if let Some(index) = self.by_key.get(&key) {
            return Registration {
                index: *index,
                new: false,
            };
        }
        let index = self.free.pop().unwrap_or_else(|| {
            self.entries.push(None);
            self.entries.len() as u32
        });
        self.entries[index as usize - 1] = Some(entry);
        self.by_key.insert(key, index);
        Registration { index, new: true }
    }

    fn remove(&mut self, key: &[u32; 8]) -> Option<u32> {
        let index = self.by_key.remove(key)?;
        self.entries[index as usize - 1] = None;
        self.free.push(index);
        Some(index)
    }

    fn get(&self, index: u32) -> Option<&T> {
        self.entries.get(index.checked_sub(1)? as usize)?.as_ref()
    }

    fn len(&self) -> usize {
        self.by_key.len()
    }
}

#[derive(Default)]
pub struct Registry {
    textures: Slots<TextureEntry>,
    samplers: Slots<SamplerEntry>,
    version: u64,
    built: Option<Tables>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Gives a T# its slot; the same words always get the same one.
    pub fn register_texture(&mut self, words: &ImageWords) -> Result<Registration, RegistryError> {
        let image = gfx10::decode_image(words).ok_or(RegistryError::NotAnImage(*words))?;
        let entry = TextureEntry {
            words: *words,
            image,
            number: images::number(words),
        };
        let registration = self.textures.insert(*words, entry);
        self.version += u64::from(registration.new);
        Ok(registration)
    }

    pub fn register_sampler(&mut self, words: &SamplerWords) -> Registration {
        let entry = SamplerEntry {
            words: *words,
            sampler: gfx10::decode_sampler(words),
        };
        let registration = self.samplers.insert(images::sampler_key(words), entry);
        self.version += u64::from(registration.new);
        registration
    }

    /// Forgets a T#, when its texture is destroyed. Its slot is reused by a later registration.
    pub fn release_texture(&mut self, words: &ImageWords) -> bool {
        let removed = self.textures.remove(words).is_some();
        self.version += u64::from(removed);
        removed
    }

    pub fn release_sampler(&mut self, words: &SamplerWords) -> bool {
        let removed = self.samplers.remove(&images::sampler_key(words)).is_some();
        self.version += u64::from(removed);
        removed
    }

    pub fn texture(&self, index: u32) -> Option<&TextureEntry> {
        self.textures.get(index)
    }

    pub fn sampler(&self, index: u32) -> Option<&SamplerEntry> {
        self.samplers.get(index)
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    pub fn sampler_count(&self) -> usize {
        self.samplers.len()
    }

    /// Registers every image descriptor in a block of heap memory laid out as slots of `stride` dwords (at least
    /// 8). Slots that are not images (buffers, zeros, samplers) are skipped. Returns the registrations made.
    pub fn scan_textures(&mut self, heap: &[u32], stride: usize) -> Vec<(usize, Registration)> {
        let mut found = Vec::new();
        for (slot, chunk) in heap.chunks_exact(stride.max(8)).enumerate() {
            let words: ImageWords = chunk[..8].try_into().expect("eight words");
            if gfx10::is_image(&words)
                && let Ok(registration) = self.register_texture(&words)
            {
                found.push((slot, registration));
            }
        }
        found
    }

    /// Registers every non-zero S# in a block of heap memory laid out as slots of `stride` dwords (at least 4).
    pub fn scan_samplers(&mut self, heap: &[u32], stride: usize) -> Vec<(usize, Registration)> {
        let mut found = Vec::new();
        for (slot, chunk) in heap.chunks_exact(stride.max(4)).enumerate() {
            let words: SamplerWords = chunk[..4].try_into().expect("four words");
            if words != [0; 4] {
                found.push((slot, self.register_sampler(&words)));
            }
        }
        found
    }

    /// The slot word a shader gets for a T#: the bindless index with the number type in the top two bits; 0 when
    /// the descriptor is not registered.
    pub fn texture_slot(&self, words: &ImageWords) -> u32 {
        self.textures
            .by_key
            .get(words)
            .and_then(|index| self.textures.get(*index))
            .map_or(0, |entry| {
                images::slot(self.textures.by_key[words], entry.number)
            })
    }

    pub fn sampler_slot(&self, words: &SamplerWords) -> u32 {
        self.samplers
            .by_key
            .get(&images::sampler_key(words))
            .map_or(0, |index| images::slot(*index, Number::Float))
    }

    /// Changes whenever a registration is added or released.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The tables as the shaders read them, rebuilt only when something changed.
    pub fn tables(&mut self) -> Result<&Tables, RegistryError> {
        if self
            .built
            .as_ref()
            .is_none_or(|t| t.version != self.version)
        {
            let textures: Vec<([u32; 8], u32)> = self
                .textures
                .by_key
                .iter()
                .map(|(key, index)| {
                    let entry = self.textures.get(*index).expect("registered");
                    (*key, images::slot(*index, entry.number))
                })
                .collect();
            let samplers: Vec<([u32; 8], u32)> = self
                .samplers
                .by_key
                .iter()
                .map(|(key, index)| (*key, images::slot(*index, Number::Float)))
                .collect();
            self.built = Some(Tables {
                textures: images::table(&textures)?,
                samplers: images::table(&samplers)?,
                version: self.version,
            });
        }
        Ok(self.built.as_ref().expect("just built"))
    }
}
