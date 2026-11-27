//! RBF0 documents as readable, exactly reversible XML.
//!
//! ```text
//! <rbf0 width="wide">
//!   <Root name="a">                     element; its leading string values are attributes
//!     <Item x-int="3"/>                 a value record: x-int, x-float, x-vec3, x-bool or x-str
//!     <Text>some text</Text>            a text record (NUL-terminated); x-enc="raw" or "hex" otherwise
//!   </Root>
//! </rbf0>
//! ```
//!
//! Names that are not plain XML names, or start with `x-`, use `<x-element name="...">`, `<x-value name="...">` and
//! `<x-attr name="...">`. A text record that cannot be inline is an `<x-text>` child. Attributes that are not plain
//! strings are `<x-attr>` children before everything else.

use crate::error::{Error, Result};
use crate::rbf0::{Document, Element, Node, Value, ValueData, Width};
use crate::xml::{self, XmlElement, XmlNode};

const MARKERS: [&str; 5] = ["x-int", "x-float", "x-vec3", "x-bool", "x-str"];

fn plain_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !s.starts_with("x-")
        && s != "rbf0"
}

fn float_text(bits: u64, width: Width) -> String {
    match width {
        Width::Wide => {
            let f = f64::from_bits(bits);
            if f.is_nan() {
                format!("nan:{bits:#x}")
            } else {
                format!("{f:?}")
            }
        }
        Width::Narrow => {
            let f = f32::from_bits(bits as u32);
            if f.is_nan() {
                format!("nan:{bits:#x}")
            } else {
                format!("{f:?}")
            }
        }
    }
}

fn parse_float(text: &str, width: Width) -> Result<u64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("nan:0x") {
        return u64::from_str_radix(hex, 16)
            .map_err(|_| Error::Xml(format!("bad NaN bits {text:?}")));
    }
    let bad = || Error::Xml(format!("bad float {text:?}"));
    Ok(match width {
        Width::Wide => text.parse::<f64>().map_err(|_| bad())?.to_bits(),
        Width::Narrow => u64::from(text.parse::<f32>().map_err(|_| bad())?.to_bits()),
    })
}

fn marker(value: &Value, width: Width) -> (&'static str, String) {
    match &value.data {
        ValueData::Int(v) => ("x-int", v.to_string()),
        ValueData::Float(b) => ("x-float", float_text(*b, width)),
        ValueData::Bool(b) => ("x-bool", b.to_string()),
        ValueData::Vec3(v) => (
            "x-vec3",
            v.iter()
                .map(|&b| float_text(b, width))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        ValueData::Str(s) => ("x-str", s.clone()),
    }
}

fn open(out: &mut String, depth: usize, tag: &str, name: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push('<');
    if plain_name(name) {
        out.push_str(name);
    } else {
        out.push_str(tag);
        out.push_str(" name=\"");
        xml::escape_attr(out, name);
        out.push('"');
    }
}

fn attr(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(key);
    out.push_str("=\"");
    xml::escape_attr(out, value);
    out.push('"');
}

fn write_value(out: &mut String, depth: usize, tag: &str, value: &Value, width: Width) {
    if tag == "x-attr" {
        out.push_str(&"  ".repeat(depth));
        out.push_str("<x-attr name=\"");
        xml::escape_attr(out, &value.name);
        out.push('"');
    } else {
        open(out, depth, "x-value", &value.name);
    }
    let (key, text) = marker(value, width);
    attr(out, key, &text);
    out.push_str("/>\n");
}

fn text_form(bytes: &[u8]) -> (Option<&'static str>, String) {
    if let Some((0, head)) = bytes.split_last()
        && let Ok(s) = std::str::from_utf8(head)
    {
        return (None, s.to_owned());
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (Some("raw"), s.to_owned()),
        Err(_) => (
            Some("hex"),
            bytes.iter().map(|b| format!("{b:02x}")).collect(),
        ),
    }
}

fn write_element(out: &mut String, depth: usize, e: &Element, width: Width) {
    let mut seen = std::collections::HashSet::new();
    let fallback = !plain_name(&e.name);
    let pretty_attrs = e.attrs.iter().all(|a| {
        matches!(a.data, ValueData::Str(_))
            && plain_name(&a.name)
            && !(fallback && a.name == "name")
            && seen.insert(a.name.as_str())
    });
    open(out, depth, "x-element", &e.name);
    if pretty_attrs {
        for a in &e.attrs {
            if let ValueData::Str(s) = &a.data {
                attr(out, &a.name, s);
            }
        }
    }
    let inline = match (pretty_attrs, e.content.as_slice()) {
        (true, [Node::Text(t)]) => Some(text_form(t)).filter(|(_, s)| !s.is_empty()),
        _ => None,
    };
    if let Some((enc, text)) = inline {
        if let Some(enc) = enc {
            attr(out, "x-enc", enc);
        }
        out.push('>');
        xml::escape_text(out, &text);
        out.push_str("</");
        out.push_str(if plain_name(&e.name) {
            &e.name
        } else {
            "x-element"
        });
        out.push_str(">\n");
        return;
    }
    if pretty_attrs && e.content.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    if !pretty_attrs {
        for a in &e.attrs {
            write_value(out, depth + 1, "x-attr", a, width);
        }
    }
    for node in &e.content {
        match node {
            Node::Element(child) => write_element(out, depth + 1, child, width),
            Node::Value(v) => write_value(out, depth + 1, "x-value", v, width),
            Node::Text(t) => {
                let (enc, text) = text_form(t);
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str("<x-text");
                if let Some(enc) = enc {
                    attr(out, "x-enc", enc);
                }
                if text.is_empty() {
                    out.push_str("/>\n");
                } else {
                    out.push('>');
                    xml::escape_text(out, &text);
                    out.push_str("</x-text>\n");
                }
            }
        }
    }
    out.push_str(&"  ".repeat(depth));
    out.push_str("</");
    out.push_str(if plain_name(&e.name) {
        &e.name
    } else {
        "x-element"
    });
    out.push_str(">\n");
}

/// Render a document as XML.
pub fn to_xml(doc: &Document) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rbf0 width=\"");
    out.push_str(match doc.width {
        Width::Wide => "wide",
        Width::Narrow => "narrow",
    });
    out.push_str("\">\n");
    write_element(&mut out, 1, &doc.root, doc.width);
    out.push_str("</rbf0>\n");
    out
}

fn bad<T>(message: String) -> Result<T> {
    Err(Error::Xml(message))
}

fn decode_text(enc: Option<&str>, content: &str) -> Result<Vec<u8>> {
    match enc {
        None => {
            let mut bytes = content.as_bytes().to_vec();
            bytes.push(0);
            Ok(bytes)
        }
        Some("raw") => Ok(content.as_bytes().to_vec()),
        Some("hex") => {
            let digits: Vec<u8> = content.bytes().collect();
            if !digits.len().is_multiple_of(2) {
                return bad("odd number of hex digits in text".into());
            }
            digits
                .chunks(2)
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|p| u8::from_str_radix(p, 16).ok())
                        .ok_or_else(|| Error::Xml("bad hex digit in text".into()))
                })
                .collect()
        }
        Some(other) => bad(format!("unknown x-enc {other:?}")),
    }
}

fn value_of(x: &XmlElement, name: String, width: Width) -> Result<Value> {
    let marks: Vec<&str> = x
        .attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| MARKERS.contains(k))
        .collect();
    let [mark] = marks.as_slice() else {
        return bad(format!("value {name:?} needs exactly one of {MARKERS:?}"));
    };
    if !x.children.is_empty() {
        return bad(format!("value {name:?} has content"));
    }
    let text = x.attr(mark).unwrap_or_default();
    let data = match *mark {
        "x-int" => ValueData::Int(
            text.trim()
                .parse()
                .map_err(|_| Error::Xml(format!("bad integer {text:?}")))?,
        ),
        "x-float" => ValueData::Float(parse_float(text, width)?),
        "x-bool" => match text.trim() {
            "true" => ValueData::Bool(true),
            "false" => ValueData::Bool(false),
            other => return bad(format!("bad bool {other:?}")),
        },
        "x-vec3" => {
            let parts: Vec<&str> = text.split_whitespace().collect();
            let [a, b, c] = parts.as_slice() else {
                return bad(format!("a vec3 needs three numbers, got {text:?}"));
            };
            ValueData::Vec3([
                parse_float(a, width)?,
                parse_float(b, width)?,
                parse_float(c, width)?,
            ])
        }
        _ => ValueData::Str(text.to_owned()),
    };
    Ok(Value { name, data })
}

fn is_value(x: &XmlElement) -> bool {
    x.name == "x-value"
        || x.name == "x-attr"
        || x.attrs.iter().any(|(k, _)| MARKERS.contains(&k.as_str()))
}

fn named(x: &XmlElement, special: &str) -> Result<String> {
    if x.name == special {
        x.attr("name")
            .map(str::to_owned)
            .ok_or_else(|| Error::Xml(format!("<{special}> without a name")))
    } else {
        Ok(x.name.clone())
    }
}

fn convert(x: &XmlElement, width: Width) -> Result<Element> {
    let name = named(x, "x-element")?;
    let mut element = Element {
        name,
        attrs: Vec::new(),
        content: Vec::new(),
    };
    let mut enc = None;
    for (k, v) in &x.attrs {
        if k == "x-enc" {
            enc = Some(v.as_str());
        } else if k.starts_with("x-") {
            return bad(format!("unknown attribute {k:?} on {:?}", element.name));
        } else if !(x.name == "x-element" && k == "name") {
            element.attrs.push(Value {
                name: k.clone(),
                data: ValueData::Str(v.clone()),
            });
        }
    }
    if !x.has_element_children() {
        let text = x.text();
        if !text.is_empty() {
            element.content.push(Node::Text(decode_text(enc, &text)?));
        } else if enc.is_some() {
            return bad(format!("x-enc on {:?} without text", element.name));
        }
        return Ok(element);
    }
    if enc.is_some() {
        return bad(format!(
            "x-enc on {:?}, which has child elements",
            element.name
        ));
    }
    let mut leading = true;
    for child in &x.children {
        match child {
            XmlNode::Text(t) => {
                if !t.trim().is_empty() {
                    return bad(format!("text mixed with elements in {:?}", element.name));
                }
            }
            XmlNode::Element(c) if c.name == "x-attr" => {
                if !leading {
                    return bad(format!("<x-attr> after content in {:?}", element.name));
                }
                element.attrs.push(value_of(c, named(c, "x-attr")?, width)?);
            }
            XmlNode::Element(c) => {
                leading = false;
                if c.name == "x-text" {
                    if c.has_element_children() {
                        return bad("<x-text> has child elements".into());
                    }
                    let mut enc = None;
                    for (k, v) in &c.attrs {
                        if k != "x-enc" {
                            return bad(format!("unknown attribute {k:?} on <x-text>"));
                        }
                        enc = Some(v.as_str());
                    }
                    element
                        .content
                        .push(Node::Text(decode_text(enc, &c.text())?));
                } else if is_value(c) {
                    element
                        .content
                        .push(Node::Value(value_of(c, named(c, "x-value")?, width)?));
                } else {
                    element.content.push(Node::Element(convert(c, width)?));
                }
            }
        }
    }
    Ok(element)
}

/// Read a document written by [`to_xml`] (or edited by hand).
pub fn from_xml(src: &str) -> Result<Document> {
    let root = xml::parse(src)?;
    if root.name != "rbf0" {
        return bad(format!("root element is <{}>, expected <rbf0>", root.name));
    }
    let width = match root.attr("width") {
        Some("wide") => Width::Wide,
        Some("narrow") => Width::Narrow,
        other => return bad(format!("width must be wide or narrow, got {other:?}")),
    };
    let mut elements = root.children.iter().filter_map(|c| match c {
        XmlNode::Element(e) => Some(e),
        XmlNode::Text(_) => None,
    });
    let (Some(top), None) = (elements.next(), elements.next()) else {
        return bad("<rbf0> must hold exactly one root element".into());
    };
    Ok(Document {
        width,
        root: convert(top, width)?,
    })
}
