//! A [`BytecodeStore`] in a folder: one file per shader, named by the hash of its contents.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::seen::{BytecodeStore, PsoKey};

pub struct DirBytecode {
    dir: PathBuf,
}

impl DirBytecode {
    /// The folder is made when the first shader is kept.
    pub fn new(dir: impl Into<PathBuf>) -> DirBytecode {
        DirBytecode { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, hash: &PsoKey) -> PathBuf {
        let name: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(name)
    }
}

impl BytecodeStore for DirBytecode {
    fn put(&self, hash: &PsoKey, bytes: &[u8]) {
        let path = self.file(hash);
        if path.exists() {
            return;
        }
        let _ = std::fs::create_dir_all(&self.dir);
        let temporary = path.with_extension("tmp");
        if std::fs::write(&temporary, bytes).is_ok() && std::fs::rename(&temporary, &path).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    /// The bytes, unless the file is missing or its contents no longer hash to the name.
    fn get(&self, hash: &PsoKey) -> Option<Vec<u8>> {
        let bytes = std::fs::read(self.file(hash)).ok()?;
        let found: PsoKey = Sha256::digest(&bytes).into();
        (found == *hash).then_some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_of(bytes: &[u8]) -> PsoKey {
        Sha256::digest(bytes).into()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp/dir-bytecode")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn shaders_come_back_by_their_hash_and_a_damaged_file_reads_as_missing() {
        let dir = scratch("round-trip");
        let store = DirBytecode::new(&dir);
        let bytes = vec![7u8; 300];
        let hash = hash_of(&bytes);
        assert!(store.get(&hash).is_none());
        store.put(&hash, &bytes);
        store.put(&hash, &bytes);
        assert_eq!(store.get(&hash).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(DirBytecode::new(&dir).get(&hash).unwrap(), bytes);
        let name: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(dir.join(name), b"damaged").unwrap();
        assert!(store.get(&hash).is_none());
    }
}
