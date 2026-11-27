//! Bytes in, readable text out, and back. The archive reader supplies the bytes; loose files work the same way.

use crate::catalog;
use crate::error::{Error, Result};
use crate::rbf0::{self, MAGIC};
use crate::rbf0_xml;
use crate::text::{self, TextFile};
use crate::xml;

/// What a file's bytes are, judged only by the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// RBF0 binary XML.
    Rbf0,
    /// Plain XML text, already readable, with its root element's name when found.
    Xml(Option<String>),
    /// A `0DRP` resource holding string tables.
    TextTable,
    Unknown,
}

pub fn sniff(bytes: &[u8]) -> Kind {
    if bytes.starts_with(MAGIC) {
        return Kind::Rbf0;
    }
    if let Some(root) = xml::root_name(bytes) {
        return Kind::Xml(Some(root));
    }
    if bytes.get(0x10..0x14) == Some(b"0DRP") && text::read_tables(bytes).is_ok() {
        return Kind::TextTable;
    }
    Kind::Unknown
}

/// One converted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub kind: Kind,
    /// Where it belongs, such as `Systems/UI/UIScreen.xml`.
    pub path: String,
    pub data: Vec<u8>,
}

/// Convert one file to its readable form. `name_hint` is the file's own name when the archive reader knows it.
pub fn export(bytes: &[u8], name_hint: Option<&str>) -> Result<Exported> {
    let kind = sniff(bytes);
    match &kind {
        Kind::Rbf0 => {
            let doc = rbf0::parse(bytes)?;
            let path = catalog::systems_path(&doc.root.name, name_hint, "xml");
            Ok(Exported {
                kind,
                path,
                data: rbf0_xml::to_xml(&doc).into_bytes(),
            })
        }
        Kind::Xml(root) => Ok(Exported {
            path: catalog::systems_path(root.as_deref().unwrap_or("Unsorted"), name_hint, "xml"),
            data: bytes.to_vec(),
            kind,
        }),
        Kind::TextTable => {
            let file = text::read_tables(bytes)?;
            Ok(Exported {
                path: catalog::text_path(&file.language),
                data: file.to_json().into_bytes(),
                kind,
            })
        }
        Kind::Unknown => Err(Error::Unsupported(
            "this file's format is not decoded; see the NOT_DECODED.md notes under Systems".into(),
        )),
    }
}

/// Convert readable text back to the game's form. An `<rbf0>` XML document becomes binary. A text-table JSON is
/// patched into `base`, the resource it was exported from. Any other XML is already the game's form and is returned
/// as it is.
pub fn import(input: &[u8], base: Option<&[u8]>) -> Result<Vec<u8>> {
    let trimmed = input.trim_ascii_start();
    if trimmed.starts_with(b"{") {
        let base = base.ok_or_else(|| {
            Error::Unsupported(
                "a text JSON needs the original resource it was exported from (--base)".into(),
            )
        })?;
        let file = TextFile::from_json(
            std::str::from_utf8(input).map_err(|e| Error::Json(e.to_string()))?,
        )?;
        return text::patch_tables(base, &file);
    }
    match xml::root_name(input).as_deref() {
        Some("rbf0") => {
            let src = std::str::from_utf8(input).map_err(|e| Error::Xml(e.to_string()))?;
            rbf0::encode(&rbf0_xml::from_xml(src)?)
        }
        Some(_) => Ok(input.to_vec()),
        None => Err(Error::Unsupported("not XML or a text-table JSON".into())),
    }
}
