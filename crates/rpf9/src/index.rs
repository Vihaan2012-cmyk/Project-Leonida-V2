//! The asset index: one row per resource, kept in a `redb` file, with lookups by kind, name hash, recovered name
//! and content hash.

use std::collections::BTreeMap;
use std::path::Path;

use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTable, TableDefinition, WriteTransaction,
};
use serde::{Deserialize, Serialize};

use crate::archive::ScanReport;
use crate::error::Rpf9Error;
use crate::model::{Kind, Resource};
use crate::names;

/// `(archive, offset)` to the resource row.
const ASSETS: TableDefinition<(u32, u64), &[u8]> = TableDefinition::new("assets");
const ARCHIVES: TableDefinition<u32, &[u8]> = TableDefinition::new("archives");
const BY_KIND: TableDefinition<(u8, u32, u64), ()> = TableDefinition::new("by_kind");
const BY_NAME_HASH: TableDefinition<(u32, u32, u64), ()> = TableDefinition::new("by_name_hash");
const BY_NAME: TableDefinition<(&str, u32, u64), ()> = TableDefinition::new("by_name");
const BY_CONTENT: TableDefinition<(u128, u32, u64), ()> = TableDefinition::new("by_content");

/// An archive that has been indexed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub id: u32,
    /// Path inside the package, `/`-separated.
    pub name: String,
    pub size: u64,
    pub resources: u64,
    pub streams: u64,
    pub stored_bytes: u64,
    pub inflated_bytes: u64,
    pub zero_padding: u64,
    pub unread_bytes: u64,
    pub bink2: u64,
    pub awc: u64,
    pub rejected: u64,
    /// Whether the scan ran to the end (a failed run leaves its archive marked as not done).
    pub done: bool,
}

/// What `Index::list` selects.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub kind: Option<Kind>,
    /// Text found in a recovered name or an archive name; eight hex digits also match a name hash and 64 a
    /// content hash.
    pub find: Option<String>,
    pub limit: Option<usize>,
}

pub struct Index {
    db: Database,
}

fn key_of(resource: &Resource) -> (u32, u64) {
    (resource.archive, resource.offset)
}

fn content_key(hash: &[u8; 32]) -> u128 {
    u128::from_be_bytes(hash[..16].try_into().unwrap())
}

fn name_keys(resource: &Resource) -> Vec<String> {
    let mut keys = Vec::new();
    for name in &resource.names {
        for key in [name.to_lowercase(), names::stem(name).to_lowercase()] {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

fn decode(bytes: &[u8]) -> Result<Resource, Rpf9Error> {
    Ok(postcard::from_bytes(bytes)?)
}

impl Index {
    /// Open the index at `path`, creating it when it does not exist.
    pub fn open(path: &Path) -> Result<Index, Rpf9Error> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| Rpf9Error::io(parent, error))?;
        }
        let db = Database::create(path)?;
        let write = db.begin_write()?;
        {
            write.open_table(ASSETS)?;
            write.open_table(ARCHIVES)?;
            write.open_table(BY_KIND)?;
            write.open_table(BY_NAME_HASH)?;
            write.open_table(BY_NAME)?;
            write.open_table(BY_CONTENT)?;
        }
        write.commit()?;
        Ok(Index { db })
    }

    pub fn archives(&self) -> Result<Vec<ArchiveInfo>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(ARCHIVES)?;
        table
            .iter()?
            .map(|row| {
                let (_, value) = row?;
                Ok(postcard::from_bytes(value.value())?)
            })
            .collect()
    }

    pub fn archive(&self, name: &str) -> Result<Option<ArchiveInfo>, Rpf9Error> {
        Ok(self.archives()?.into_iter().find(|info| info.name == name))
    }

    /// Start (or restart) indexing the archive `name`: its old rows are removed and its id is returned.
    pub fn begin_archive(&self, name: &str, size: u64) -> Result<u32, Rpf9Error> {
        let existing = self.archive(name)?;
        let id = match &existing {
            Some(info) => {
                self.remove_rows(info.id)?;
                info.id
            }
            None => {
                self.archives()?
                    .iter()
                    .map(|info| info.id)
                    .max()
                    .unwrap_or(0)
                    + 1
            }
        };
        self.store_archive(&ArchiveInfo {
            id,
            name: name.to_string(),
            size,
            resources: 0,
            streams: 0,
            stored_bytes: 0,
            inflated_bytes: 0,
            zero_padding: 0,
            unread_bytes: 0,
            bink2: 0,
            awc: 0,
            rejected: 0,
            done: false,
        })?;
        Ok(id)
    }

    /// Record how the scan of an archive ended and mark it done.
    pub fn finish_archive(
        &self,
        id: u32,
        name: &str,
        size: u64,
        report: &ScanReport,
    ) -> Result<ArchiveInfo, Rpf9Error> {
        let info = ArchiveInfo {
            id,
            name: name.to_string(),
            size,
            resources: report.resources,
            streams: report.walk.streams,
            stored_bytes: report.walk.stored_bytes,
            inflated_bytes: report.walk.inflated_bytes,
            zero_padding: report.walk.zero_padding,
            unread_bytes: report.walk.unread_bytes,
            bink2: report.walk.bink2,
            awc: report.walk.awc,
            rejected: report.rejected,
            done: true,
        };
        self.store_archive(&info)?;
        Ok(info)
    }

    fn store_archive(&self, info: &ArchiveInfo) -> Result<(), Rpf9Error> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(ARCHIVES)?;
            let bytes = postcard::to_stdvec(info)?;
            table.insert(info.id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    fn remove_rows(&self, archive: u32) -> Result<(), Rpf9Error> {
        let write = self.db.begin_write()?;
        {
            let mut assets = write.open_table(ASSETS)?;
            let mut by_kind = write.open_table(BY_KIND)?;
            let mut by_hash = write.open_table(BY_NAME_HASH)?;
            let mut by_name = write.open_table(BY_NAME)?;
            let mut by_content = write.open_table(BY_CONTENT)?;
            let rows: Vec<Resource> = assets
                .range((archive, 0)..=(archive, u64::MAX))?
                .map(|row| decode(row?.1.value()))
                .collect::<Result<_, _>>()?;
            for resource in rows {
                remove_keys(
                    &resource,
                    &mut by_kind,
                    &mut by_hash,
                    &mut by_name,
                    &mut by_content,
                )?;
                assets.remove(key_of(&resource))?;
            }
        }
        write.commit()?;
        Ok(())
    }

    /// Add resources, each with its `archive` set to the id from `begin_archive`, in one transaction.
    pub fn add(&self, resources: &[Resource]) -> Result<(), Rpf9Error> {
        let write = self.db.begin_write()?;
        insert_rows(&write, resources)?;
        write.commit()?;
        Ok(())
    }

    pub fn get(&self, archive: u32, offset: u64) -> Result<Option<Resource>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(ASSETS)?;
        table
            .get((archive, offset))?
            .map(|row| decode(row.value()))
            .transpose()
    }

    /// The resources of a kind, at most `limit`, in archive and offset order of each archive.
    pub fn by_kind(&self, kind: Kind, limit: usize) -> Result<Vec<Resource>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(BY_KIND)?;
        let code = kind.code();
        let keys = table
            .range((code, 0, 0)..=(code, u32::MAX, u64::MAX))?
            .take(limit)
            .map(|row| {
                let row = row?;
                let key = row.0.value();
                Ok((key.1, key.2))
            })
            .collect::<Result<Vec<_>, Rpf9Error>>()?;
        load(&read, keys)
    }

    pub fn by_name_hash(&self, hash: u32) -> Result<Vec<Resource>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(BY_NAME_HASH)?;
        let keys = table
            .range((hash, 0, 0)..=(hash, u32::MAX, u64::MAX))?
            .map(|row| {
                let row = row?;
                let key = row.0.value();
                Ok((key.1, key.2))
            })
            .collect::<Result<Vec<_>, Rpf9Error>>()?;
        load(&read, keys)
    }

    /// Resources whose recovered name is `name`, ignoring case, with or without its `.#xx` suffix.
    pub fn by_name(&self, name: &str) -> Result<Vec<Resource>, Rpf9Error> {
        let key = name.to_lowercase();
        let read = self.db.begin_read()?;
        let table = read.open_table(BY_NAME)?;
        let keys = table
            .range((key.as_str(), 0, 0)..=(key.as_str(), u32::MAX, u64::MAX))?
            .map(|row| {
                let row = row?;
                let key = row.0.value();
                Ok((key.1, key.2))
            })
            .collect::<Result<Vec<_>, Rpf9Error>>()?;
        load(&read, keys)
    }

    /// Every resource with this content, in any archive.
    pub fn by_content_hash(&self, hash: &[u8; 32]) -> Result<Vec<Resource>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(BY_CONTENT)?;
        let key = content_key(hash);
        let keys = table
            .range((key, 0, 0)..=(key, u32::MAX, u64::MAX))?
            .map(|row| {
                let row = row?;
                let key = row.0.value();
                Ok((key.1, key.2))
            })
            .collect::<Result<Vec<_>, Rpf9Error>>()?;
        Ok(load(&read, keys)?
            .into_iter()
            .filter(|resource| &resource.content_hash == hash)
            .collect())
    }

    /// Resources matching `query`, in archive and offset order.
    pub fn list(&self, query: &Query) -> Result<Vec<Resource>, Rpf9Error> {
        let limit = query.limit.unwrap_or(usize::MAX);
        let read = self.db.begin_read()?;
        let mut keys: Vec<(u32, u64)> = Vec::new();
        let kind_filter = query.kind.map(Kind::code);
        match query.find.as_deref().map(str::to_lowercase) {
            None => {
                if let Some(code) = kind_filter {
                    let table = read.open_table(BY_KIND)?;
                    for row in table
                        .range((code, 0, 0)..=(code, u32::MAX, u64::MAX))?
                        .take(limit)
                    {
                        let key = row?.0.value();
                        keys.push((key.1, key.2));
                    }
                } else {
                    let table = read.open_table(ASSETS)?;
                    for row in table.iter()?.take(limit) {
                        keys.push(row?.0.value());
                    }
                }
            }
            Some(text) => {
                let mut found: Vec<(u32, u64)> = Vec::new();
                let names = read.open_table(BY_NAME)?;
                for row in names.iter()? {
                    let row = row?;
                    let key = row.0.value();
                    if key.0.contains(&text) {
                        found.push((key.1, key.2));
                    }
                }
                let digits = text.trim_start_matches("0x");
                if digits.len() == 8 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    let hash = u32::from_str_radix(digits, 16).unwrap_or(0);
                    let table = read.open_table(BY_NAME_HASH)?;
                    for row in table.range((hash, 0, 0)..=(hash, u32::MAX, u64::MAX))? {
                        let key = row?.0.value();
                        found.push((key.1, key.2));
                    }
                }
                if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    let mut hash = [0u8; 32];
                    for (index, byte) in hash.iter_mut().enumerate() {
                        *byte =
                            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap_or(0);
                    }
                    let table = read.open_table(BY_CONTENT)?;
                    let key = content_key(&hash);
                    for row in table.range((key, 0, 0)..=(key, u32::MAX, u64::MAX))? {
                        let key = row?.0.value();
                        found.push((key.1, key.2));
                    }
                }
                let archives = self.archives()?;
                let matching: Vec<u32> = archives
                    .iter()
                    .filter(|info| info.name.to_lowercase().contains(&text))
                    .map(|info| info.id)
                    .collect();
                if !matching.is_empty() {
                    let table = read.open_table(ASSETS)?;
                    for id in matching {
                        for row in table.range((id, 0)..=(id, u64::MAX))? {
                            found.push(row?.0.value());
                        }
                    }
                }
                found.sort_unstable();
                found.dedup();
                keys = found;
            }
        }
        let mut out = Vec::new();
        for resource in load(&read, keys)? {
            if kind_filter.is_none_or(|code| resource.kind.code() == code) {
                out.push(resource);
                if out.len() == limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// How many resources there are of each kind.
    pub fn counts(&self) -> Result<BTreeMap<Kind, u64>, Rpf9Error> {
        let read = self.db.begin_read()?;
        let table = read.open_table(BY_KIND)?;
        let mut counts = BTreeMap::new();
        for row in table.iter()? {
            let code = row?.0.value().0;
            if let Some(kind) = Kind::from_code(code) {
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }
}

fn load(read: &ReadTransaction, keys: Vec<(u32, u64)>) -> Result<Vec<Resource>, Rpf9Error> {
    let table = read.open_table(ASSETS)?;
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(row) = table.get(key)? {
            out.push(decode(row.value())?);
        }
    }
    Ok(out)
}

fn insert_rows(write: &WriteTransaction, resources: &[Resource]) -> Result<(), Rpf9Error> {
    let mut assets = write.open_table(ASSETS)?;
    let mut by_kind = write.open_table(BY_KIND)?;
    let mut by_hash = write.open_table(BY_NAME_HASH)?;
    let mut by_name = write.open_table(BY_NAME)?;
    let mut by_content = write.open_table(BY_CONTENT)?;
    for resource in resources {
        let (archive, offset) = key_of(resource);
        let bytes = postcard::to_stdvec(resource)?;
        assets.insert((archive, offset), bytes.as_slice())?;
        by_kind.insert((resource.kind.code(), archive, offset), ())?;
        if let Some(hash) = resource.name_hash {
            by_hash.insert((hash, archive, offset), ())?;
        }
        for key in name_keys(resource) {
            by_name.insert((key.as_str(), archive, offset), ())?;
        }
        by_content.insert((content_key(&resource.content_hash), archive, offset), ())?;
    }
    Ok(())
}

fn remove_keys(
    resource: &Resource,
    by_kind: &mut redb::Table<(u8, u32, u64), ()>,
    by_hash: &mut redb::Table<(u32, u32, u64), ()>,
    by_name: &mut redb::Table<(&str, u32, u64), ()>,
    by_content: &mut redb::Table<(u128, u32, u64), ()>,
) -> Result<(), Rpf9Error> {
    let (archive, offset) = key_of(resource);
    by_kind.remove((resource.kind.code(), archive, offset))?;
    if let Some(hash) = resource.name_hash {
        by_hash.remove((hash, archive, offset))?;
    }
    for key in name_keys(resource) {
        by_name.remove((key.as_str(), archive, offset))?;
    }
    by_content.remove((content_key(&resource.content_hash), archive, offset))?;
    Ok(())
}
