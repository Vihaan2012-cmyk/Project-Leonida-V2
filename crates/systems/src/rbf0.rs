//! RBF0, RAGE's binary form of an XML data file: a record stream with an inline descriptor table.

use std::collections::HashMap;

use crate::error::{Error, Result};

pub const MAGIC: &[u8; 4] = b"RBF0";
const PROLOGUE: [u8; 4] = [0xFB, 0xFF, 0x01, 0x00];
pub const MAX_DEPTH: usize = 256;
const MAX_DESCRIPTORS: usize = 0xFD;

/// Width of integers and floats. The 4-byte prologue `FB FF 01 00` selects wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Wide,
    Narrow,
}

/// A parsed stream: the value width and the single root element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub width: Width,
    pub root: Element,
}

/// An element: its leading attribute values, then content in stream order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<Value>,
    pub content: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Value(Value),
    /// The raw bytes of a text record (NUL-terminated in every measured stream).
    Text(Vec<u8>),
}

/// A named scalar record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub name: String,
    pub data: ValueData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueData {
    Int(u64),
    /// IEEE bits: a 64-bit double in wide streams, a 32-bit float in the low half in narrow ones.
    Float(u64),
    Bool(bool),
    Vec3([u64; 3]),
    Str(String),
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or(Error::Truncated(self.pos))?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn scalar(&mut self, width: Width) -> Result<u64> {
        Ok(match width {
            Width::Wide => u64::from_le_bytes(self.take(8)?.try_into().unwrap()),
            Width::Narrow => u64::from(self.u32()?),
        })
    }

    fn string(&mut self) -> Result<String> {
        let len = usize::from(self.u16()?);
        let at = self.pos;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::InvalidUtf8(at))
    }
}

struct Frame {
    element: Element,
    attrs_left: u16,
    offset: usize,
}

/// Parse one complete resource. Fails closed: anything the writer would not reproduce byte for byte is an error.
pub fn parse(bytes: &[u8]) -> Result<Document> {
    if bytes.len() < 4 || &bytes[..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    let mut r = Reader { buf: bytes, pos: 4 };
    let width = if bytes[4..].starts_with(&PROLOGUE) {
        r.pos += 4;
        Width::Wide
    } else {
        Width::Narrow
    };
    let mut names: Vec<String> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let root = loop {
        let at = r.pos;
        let [b0, b1] = r.take(2)?.try_into().unwrap();
        match (b0, b1) {
            (0xFF, 0xFF) => {
                let frame = stack.pop().ok_or(Error::NoOpenElement(at))?;
                if frame.attrs_left > 0 {
                    return Err(Error::AttributeCount(frame.offset));
                }
                match stack.last_mut() {
                    Some(parent) => parent.element.content.push(Node::Element(frame.element)),
                    None => break frame.element,
                }
            }
            (0xFD, 0xFF) => {
                let len = r.u32()? as usize;
                let text = r.take(len)?.to_vec();
                let frame = stack.last_mut().ok_or(Error::NoOpenElement(at))?;
                if frame.attrs_left > 0 {
                    return Err(Error::AttributeCount(frame.offset));
                }
                frame.element.content.push(Node::Text(text));
            }
            (index, kind) => {
                if usize::from(index) >= MAX_DESCRIPTORS || usize::from(index) > names.len() {
                    return Err(Error::BadDescriptor { index, offset: at });
                }
                if usize::from(index) == names.len() {
                    let name = r.string()?;
                    if names.contains(&name) {
                        return Err(Error::DuplicateDescriptor(name));
                    }
                    names.push(name);
                }
                let name = names[usize::from(index)].clone();
                if kind == 0x00 {
                    let (w0, w1, attrs) = (r.u16()?, r.u16()?, r.u16()?);
                    if w0 != 0 || w1 != 0 {
                        return Err(Error::HeaderWords(at));
                    }
                    if stack.len() >= MAX_DEPTH {
                        return Err(Error::TooDeep(MAX_DEPTH));
                    }
                    stack.push(Frame {
                        element: Element {
                            name,
                            attrs: Vec::new(),
                            content: Vec::new(),
                        },
                        attrs_left: attrs,
                        offset: at,
                    });
                    continue;
                }
                let data = match kind {
                    0x10 => ValueData::Int(r.scalar(width)?),
                    0x20 => ValueData::Bool(true),
                    0x30 => ValueData::Bool(false),
                    0x40 => ValueData::Float(r.scalar(width)?),
                    0x50 => ValueData::Vec3([r.scalar(width)?, r.scalar(width)?, r.scalar(width)?]),
                    0x60 => ValueData::Str(r.string()?),
                    kind => return Err(Error::UnknownRecord { kind, offset: at }),
                };
                let frame = stack.last_mut().ok_or(Error::NoOpenElement(at))?;
                let value = Value { name, data };
                if frame.attrs_left > 0 {
                    frame.attrs_left -= 1;
                    frame.element.attrs.push(value);
                } else {
                    frame.element.content.push(Node::Value(value));
                }
            }
        }
    };
    if r.pos != bytes.len() {
        return Err(Error::TrailingBytes(bytes.len() - r.pos));
    }
    Ok(Document { width, root })
}

struct Writer {
    out: Vec<u8>,
    width: Width,
    names: HashMap<String, u8>,
}

impl Writer {
    fn record(&mut self, name: &str, kind: u8) -> Result<()> {
        if let Some(&index) = self.names.get(name) {
            self.out.extend([index, kind]);
            return Ok(());
        }
        if self.names.len() >= MAX_DESCRIPTORS {
            return Err(Error::TooManyDescriptors);
        }
        let len = u16::try_from(name.len()).map_err(|_| Error::TooLong {
            what: "name",
            len: name.len(),
        })?;
        let index = self.names.len() as u8;
        self.names.insert(name.to_owned(), index);
        self.out.extend([index, kind]);
        self.out.extend(len.to_le_bytes());
        self.out.extend(name.as_bytes());
        Ok(())
    }

    fn scalar(&mut self, v: u64) -> Result<()> {
        match self.width {
            Width::Wide => self.out.extend(v.to_le_bytes()),
            Width::Narrow => {
                let v = u32::try_from(v).map_err(|_| Error::ValueRange(32))?;
                self.out.extend(v.to_le_bytes());
            }
        }
        Ok(())
    }

    fn value(&mut self, value: &Value) -> Result<()> {
        let name = value.name.as_str();
        match &value.data {
            ValueData::Int(v) => {
                self.record(name, 0x10)?;
                self.scalar(*v)
            }
            ValueData::Bool(b) => self.record(name, if *b { 0x20 } else { 0x30 }),
            ValueData::Float(v) => {
                self.record(name, 0x40)?;
                self.scalar(*v)
            }
            ValueData::Vec3(v) => {
                self.record(name, 0x50)?;
                v.iter().try_for_each(|&c| self.scalar(c))
            }
            ValueData::Str(s) => {
                let len = u16::try_from(s.len()).map_err(|_| Error::TooLong {
                    what: "string",
                    len: s.len(),
                })?;
                self.record(name, 0x60)?;
                self.out.extend(len.to_le_bytes());
                self.out.extend(s.as_bytes());
                Ok(())
            }
        }
    }

    fn element(&mut self, e: &Element, depth: usize) -> Result<()> {
        if depth >= MAX_DEPTH {
            return Err(Error::TooDeep(MAX_DEPTH));
        }
        let attrs = u16::try_from(e.attrs.len()).map_err(|_| Error::TooLong {
            what: "attribute list",
            len: e.attrs.len(),
        })?;
        self.record(&e.name, 0x00)?;
        self.out.extend([0, 0, 0, 0]);
        self.out.extend(attrs.to_le_bytes());
        for a in &e.attrs {
            self.value(a)?;
        }
        for node in &e.content {
            match node {
                Node::Element(child) => self.element(child, depth + 1)?,
                Node::Value(v) => self.value(v)?,
                Node::Text(t) => {
                    let len = u32::try_from(t.len()).map_err(|_| Error::TooLong {
                        what: "text",
                        len: t.len(),
                    })?;
                    self.out.extend([0xFD, 0xFF]);
                    self.out.extend(len.to_le_bytes());
                    self.out.extend(t);
                }
            }
        }
        self.out.extend([0xFF, 0xFF]);
        Ok(())
    }
}

/// Write a document as RBF0. `encode(parse(b)) == b` for every `b` that parses.
pub fn encode(doc: &Document) -> Result<Vec<u8>> {
    let mut w = Writer {
        out: MAGIC.to_vec(),
        width: doc.width,
        names: HashMap::new(),
    };
    if doc.width == Width::Wide {
        w.out.extend(PROLOGUE);
    }
    w.element(&doc.root, 0)?;
    Ok(w.out)
}
