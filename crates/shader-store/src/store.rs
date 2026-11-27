//! The `redb` database of translated programs.

use std::path::{Path, PathBuf};

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::error::StoreError;
use crate::program::{Key, Source, key_hex, translate_program};
use crate::record::{Record, translator_version};
use crate::translate::Built;

const SHADERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("shaders");

const SPIRV_MAGIC: u32 = 0x0723_0203;

/// A program ready for a pipeline: its record and the SPIR-V to build, with any override applied.
#[derive(Debug)]
pub struct Program {
    pub key: Key,
    pub record: Record,
    pub built: Result<Built, String>,
    /// The override file whose SPIR-V replaced the stored one.
    pub overridden_by: Option<PathBuf>,
    /// An override file that was found but could not be used.
    pub override_error: Option<String>,
}

pub struct Store {
    db: Database,
    overrides: Vec<PathBuf>,
}

fn key_bytes(key: &Key) -> &[u8] {
    key
}

impl Store {
    /// Opens the database at `path`, creating it and its folder when missing.
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)?;
        }
        let db = Database::create(path)?;
        let write = db.begin_write()?;
        write.open_table(SHADERS)?;
        write.commit()?;
        Ok(Store {
            db,
            overrides: Vec::new(),
        })
    }

    /// Folders searched, in order, for `<hash>.spv` files that replace stored SPIR-V.
    pub fn with_overrides(mut self, folders: Vec<PathBuf>) -> Store {
        self.overrides = folders;
        self
    }

    pub fn overrides(&self) -> &[PathBuf] {
        &self.overrides
    }

    /// The stored record, whatever its translator version; an older record layout is an error.
    pub fn get(&self, key: &Key) -> Result<Option<Record>, StoreError> {
        let read = self.db.begin_read()?;
        let table = read.open_table(SHADERS)?;
        let Some(value) = table.get(key_bytes(key))? else {
            return Ok(None);
        };
        Record::decode(value.value()).map(Some)
    }

    /// The stored record when it was made by this translator, in this layout, for this source's context.
    pub fn current(&self, source: &Source) -> Result<Option<Record>, StoreError> {
        Ok(match self.get(&source.key()) {
            Ok(Some(record))
                if record.translator == translator_version()
                    && record.context == source.context() =>
            {
                Some(record)
            }
            Ok(_) | Err(StoreError::Schema { .. }) => None,
            Err(error) => return Err(error),
        })
    }

    /// Writes records in one transaction: all of them land or none do.
    pub fn put_batch(&self, records: &[(Key, Record)]) -> Result<(), StoreError> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(SHADERS)?;
            for (key, record) in records {
                table.insert(key_bytes(key), record.encode().as_slice())?;
            }
        }
        write.commit()?;
        Ok(())
    }

    pub fn len(&self) -> Result<u64, StoreError> {
        let read = self.db.begin_read()?;
        Ok(read.open_table(SHADERS)?.len()?)
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Every key and record, for reports.
    pub fn for_each(
        &self,
        mut each: impl FnMut(Key, Result<Record, StoreError>),
    ) -> Result<(), StoreError> {
        let read = self.db.begin_read()?;
        let table = read.open_table(SHADERS)?;
        for entry in table.iter()? {
            let (key, value) = entry?;
            let Ok(key) = Key::try_from(key.value()) else {
                continue;
            };
            each(key, Record::decode(value.value()));
        }
        Ok(())
    }

    /// The first override file for `key` in the override folders.
    pub fn override_path(&self, key: &Key) -> Option<PathBuf> {
        let name = format!("{}.spv", key_hex(key));
        self.overrides
            .iter()
            .map(|folder| folder.join(&name))
            .find(|path| path.is_file())
    }

    /// Applies the override for `key` to a record's stored program.
    pub fn resolve(&self, key: Key, record: Record) -> Program {
        let built = record.built();
        self.resolve_with(key, record, built)
    }

    /// As `resolve`, for a program already in hand, such as one just translated.
    pub fn resolve_with(
        &self,
        key: Key,
        record: Record,
        mut built: Result<Built, String>,
    ) -> Program {
        let mut overridden_by = None;
        let mut override_error = None;
        if let (Some(path), Ok(built)) = (self.override_path(&key), built.as_mut()) {
            match read_spirv(&path) {
                Ok(words) => {
                    *built.spirv_mut() = words;
                    overridden_by = Some(path);
                }
                Err(error) => override_error = Some(format!("{}: {error}", path.display())),
            }
        }
        Program {
            key,
            record,
            built,
            overridden_by,
            override_error,
        }
    }

    /// The program for `source`: the stored one when current, else freshly translated and stored.
    pub fn program(&self, source: &Source) -> Result<Program, StoreError> {
        let key = source.key();
        if let Some(record) = self.current(source)? {
            return Ok(self.resolve(key, record));
        }
        let (record, built) = translate_program(source);
        self.put_batch(&[(key, record.clone())])?;
        Ok(self.resolve_with(key, record, built))
    }
}

fn read_spirv(path: &Path) -> Result<Vec<u32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err("not a whole number of SPIR-V words".into());
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
        .collect();
    if words[0] != SPIRV_MAGIC {
        return Err("does not start with the SPIR-V magic number".into());
    }
    Ok(words)
}

#[doc(hidden)]
impl Store {
    /// Writes raw bytes under `key`, for tests of damaged and older records.
    pub fn put_raw(&self, key: &Key, bytes: &[u8]) -> Result<(), StoreError> {
        let write = self.db.begin_write()?;
        write.open_table(SHADERS)?.insert(key_bytes(key), bytes)?;
        write.commit()?;
        Ok(())
    }
}
