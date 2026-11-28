//! Finds the Bink 2 streams of the movie archive in the game folder named by `LEONIDA_GAME_DIR`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Movies {
    path: PathBuf,
    pub streams: Vec<(u64, usize)>,
}

pub fn game_dir() -> Option<PathBuf> {
    std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from)
}

impl Movies {
    pub fn open() -> Option<Movies> {
        let path = game_dir()?.join("movies.rpf");
        let mut file = File::open(&path).ok()?;
        let total = file.metadata().ok()?.len();
        let mut streams = Vec::new();
        let mut at = 0x7000u64;
        let mut head = [0u8; 8];
        while at + 0x2C <= total {
            file.seek(SeekFrom::Start(at)).ok()?;
            file.read_exact(&mut head).ok()?;
            if &head[..3] != b"KB2" {
                break;
            }
            let len = u64::from(u32::from_le_bytes(head[4..8].try_into().unwrap())) + 8;
            if at + len > total {
                break;
            }
            streams.push((at, len as usize));
            at += len;
            let mut zero = [0u8; 4096];
            loop {
                file.seek(SeekFrom::Start(at)).ok()?;
                let n = file.read(&mut zero).ok()?;
                let skipped = zero[..n].iter().take_while(|&&b| b == 0).count();
                at += skipped as u64;
                if skipped < n || n == 0 {
                    break;
                }
            }
        }
        Some(Movies { path, streams })
    }

    pub fn arc(&self, index: usize) -> Arc<[u8]> {
        let (at, len) = self.streams[index];
        let mut file = File::open(&self.path).unwrap();
        file.seek(SeekFrom::Start(at)).unwrap();
        let mut data = vec![0u8; len];
        file.read_exact(&mut data).unwrap();
        Arc::from(data)
    }
}
