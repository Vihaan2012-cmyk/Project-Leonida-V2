use serde::{Deserialize, Serialize};

/// Where a segment's bytes go once inflated: pointers `0x5xxxxxxx` address the virtual part, `0x6xxxxxxx` the
/// physical (graphics) part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Space {
    Virtual,
    Physical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    /// Stored as is.
    Raw,
    /// zlib stream (RFC 1950).
    Zlib,
    /// Bare deflate data, as in `shaders.rpf` records.
    Deflate,
    /// Zstandard frame. No archive of the package is known to use it; `read` supports it for packages that do.
    Zstd,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::Raw => "raw",
            Codec::Zlib => "zlib",
            Codec::Deflate => "deflate",
            Codec::Zstd => "zstd",
        }
    }
}

/// One stored piece of a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub offset: u64,
    pub stored: u32,
    pub size: u32,
    pub codec: Codec,
    pub space: Space,
}

/// What a resource is, judged from its first bytes (and, for model pages, a name found inside).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// `0DRP` page directory (version 122) with its payload segments.
    PageDir,
    Psin,
    Rbf0,
    /// `RSC7` or `RSC8` header.
    Rsc,
    /// `RDS5` or `TTPF` texture pool.
    TexturePool,
    Xml,
    Text,
    Drawable,
    Fragment,
    DrawableDictionary,
    /// A system page (zero head, `0x5xxxxxxx` pointer at +8) with its graphics pages.
    SystemPage,
    /// A graphics page that starts with `42 40 00 00`.
    GraphicsPage,
    /// A zlib stream with no recognised head.
    Raw,
    Bink2,
    /// An AWC audio bank.
    Awc,
    /// A shader container record of `shaders.rpf`.
    Shader,
}

impl Kind {
    pub const ALL: [Kind; 16] = [
        Kind::PageDir,
        Kind::Psin,
        Kind::Rbf0,
        Kind::Rsc,
        Kind::TexturePool,
        Kind::Xml,
        Kind::Text,
        Kind::Drawable,
        Kind::Fragment,
        Kind::DrawableDictionary,
        Kind::SystemPage,
        Kind::GraphicsPage,
        Kind::Raw,
        Kind::Bink2,
        Kind::Awc,
        Kind::Shader,
    ];

    /// Short lowercase name, as accepted by `Kind::parse`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::PageDir => "pagedir",
            Kind::Psin => "psin",
            Kind::Rbf0 => "rbf0",
            Kind::Rsc => "rsc",
            Kind::TexturePool => "texpool",
            Kind::Xml => "xml",
            Kind::Text => "text",
            Kind::Drawable => "drawable",
            Kind::Fragment => "fragment",
            Kind::DrawableDictionary => "drawable-dict",
            Kind::SystemPage => "system",
            Kind::GraphicsPage => "graphics",
            Kind::Raw => "raw",
            Kind::Bink2 => "bink2",
            Kind::Awc => "awc",
            Kind::Shader => "shader",
        }
    }

    pub fn code(self) -> u8 {
        Kind::ALL.iter().position(|&kind| kind == self).unwrap_or(0) as u8
    }

    pub fn from_code(code: u8) -> Option<Kind> {
        Kind::ALL.get(usize::from(code)).copied()
    }

    /// A kind by its name, or by the four-byte magic of the resources it covers (`0DRP`, `PSIN`, `KB2n` ...),
    /// ignoring case.
    pub fn parse(text: &str) -> Option<Kind> {
        let lower = text.to_ascii_lowercase();
        if let Some(kind) = Kind::ALL.iter().find(|kind| kind.name() == lower) {
            return Some(*kind);
        }
        match lower.as_str() {
            "0drp" => Some(Kind::PageDir),
            "rsc7" | "rsc8" => Some(Kind::Rsc),
            "rds5" | "ttpf" => Some(Kind::TexturePool),
            "adat" => Some(Kind::Awc),
            "dxbc" => Some(Kind::Shader),
            "dr" | "#dr" => Some(Kind::Drawable),
            "ft" | "#ft" => Some(Kind::Fragment),
            "dd" | "#dd" => Some(Kind::DrawableDictionary),
            other if other.starts_with("kb2") => Some(Kind::Bink2),
            _ => None,
        }
    }
}

/// How a resource's bytes are laid out once inflated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub virtual_size: u64,
    pub physical_size: u64,
    pub segments: Vec<Segment>,
    /// False when the data ran out (a missing continuation) before the resource was whole.
    pub complete: bool,
}

/// A validated resource of an archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// Index of the archive in the asset index; 0 when scanned outside one.
    pub archive: u32,
    /// Archive offset of the first stored byte.
    pub offset: u64,
    /// Stored bytes from the first segment's start to the last one's end.
    pub stored: u64,
    /// Inflated bytes in all segments.
    pub size: u64,
    pub kind: Kind,
    /// The first four bytes of the resource's data (the shader container's `DXBC`).
    pub magic: [u8; 4],
    /// The resource's own version field where its head has one (`0DRP` 122, `RSC8` 8001, `PSIN` +8), else 0.
    pub version: u32,
    pub layout: Layout,
    /// Lower-cased joaat of the recovered name without its `.#xx` suffix.
    pub name_hash: Option<u32>,
    pub name: Option<String>,
    /// Every name recovered from the resource, the first being `name`.
    pub names: Vec<String>,
    /// SHA-256 of the inflated bytes, virtual part first.
    pub content_hash: [u8; 32],
}

impl Resource {
    pub fn magic_text(&self) -> String {
        self.magic
            .iter()
            .map(|&byte| {
                if (0x20..0x7F).contains(&byte) {
                    byte as char
                } else {
                    '.'
                }
            })
            .collect()
    }

    pub fn content_hash_hex(&self) -> String {
        hex(&self.content_hash)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}
