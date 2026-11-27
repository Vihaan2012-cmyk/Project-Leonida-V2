//! Finds the Bink 2 streams of the movie archive in the game folder named by `LEONIDA_GAME_DIR`.

use std::path::PathBuf;
use std::sync::Arc;

use memmap2::Mmap;

pub struct Movies {
    map: Mmap,
    pub streams: Vec<(usize, usize)>,
}

pub fn game_dir() -> Option<PathBuf> {
    std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from)
}

fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

impl Movies {
    pub fn open() -> Option<Movies> {
        let path = game_dir()?.join("movies.rpf");
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: the archive is only read, and nothing else maps it for writing.
        let map = unsafe { Mmap::map(&file) }.ok()?;
        let mut streams = Vec::new();
        let mut at = 0x7000;
        while at + 0x2C <= map.len() {
            if &map[at..at + 3] != b"KB2" {
                break;
            }
            let len = word(&map, at + 4) as usize + 8;
            if at + len > map.len() {
                break;
            }
            streams.push((at, len));
            at += len;
            while at < map.len() && map[at] == 0 {
                at += 1;
            }
        }
        Some(Movies { map, streams })
    }

    pub fn bytes(&self, index: usize) -> &[u8] {
        let (at, len) = self.streams[index];
        &self.map[at..at + len]
    }

    pub fn arc(&self, index: usize) -> Arc<[u8]> {
        Arc::from(self.bytes(index))
    }
}
