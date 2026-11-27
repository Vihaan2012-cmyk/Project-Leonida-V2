//! A small XML reader and the escaping rules shared with the RBF0 writer.
//!
//! The reader keeps text exactly (no whitespace normalisation) and accepts numeric character references to any
//! scalar value, including control characters, which the writer uses so that every string survives a round trip.

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlNode {
    Element(XmlElement),
    Text(String),
}

impl XmlElement {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_element_children(&self) -> bool {
        self.children
            .iter()
            .any(|c| matches!(c, XmlNode::Element(_)))
    }

    /// All text children joined.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|c| match c {
                XmlNode::Text(t) => Some(t.as_str()),
                XmlNode::Element(_) => None,
            })
            .collect()
    }
}

const MAX_XML_DEPTH: usize = 300;

fn illegal(c: char) -> bool {
    matches!(c, '\0'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}')
}

fn escape(out: &mut String, text: &str, attr: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\n' if attr => out.push_str("&#10;"),
            '\t' if attr => out.push_str("&#9;"),
            c if illegal(c) => out.push_str(&format!("&#x{:X};", c as u32)),
            c => out.push(c),
        }
    }
}

pub fn escape_text(out: &mut String, text: &str) {
    escape(out, text, false);
}

pub fn escape_attr(out: &mut String, text: &str) {
    escape(out, text, true);
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

fn err<T>(p: &Parser, message: &str) -> Result<T> {
    let line = p.src[..p.pos.min(p.src.len())].matches('\n').count() + 1;
    Err(Error::Xml(format!("line {line}: {message}")))
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn skip_to(&mut self, end: &str) -> Result<&'a str> {
        match self.rest().find(end) {
            Some(i) => {
                let body = &self.rest()[..i];
                self.pos += i + end.len();
                Ok(body)
            }
            None => err(self, &format!("missing {end}")),
        }
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len();
    }

    fn name(&mut self) -> Result<String> {
        let rest = self.rest();
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '>' | '=' | '<' | '"' | '\''))
            .unwrap_or(rest.len());
        if end == 0 {
            return err(self, "expected a name");
        }
        self.pos += end;
        Ok(rest[..end].to_owned())
    }

    fn entity(&mut self, out: &mut String) -> Result<()> {
        let rest = self.rest();
        let Some(semi) = rest.find(';').filter(|&i| i <= 12) else {
            return err(self, "unterminated entity");
        };
        let body = &rest[1..semi];
        let c = match body {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = body.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = body.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                match code.and_then(char::from_u32) {
                    Some(c) => c,
                    None => return err(self, "unknown entity"),
                }
            }
        };
        out.push(c);
        self.pos += semi + 1;
        Ok(())
    }

    fn attr_value(&mut self) -> Result<String> {
        let quote = match self.rest().chars().next() {
            Some(q @ ('"' | '\'')) => q,
            _ => return err(self, "expected a quoted attribute value"),
        };
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.rest().chars().next() else {
                return err(self, "unterminated attribute value");
            };
            if c == quote {
                self.pos += 1;
                return Ok(out);
            }
            if c == '&' {
                self.entity(&mut out)?;
            } else {
                out.push(c);
                self.pos += c.len_utf8();
            }
        }
    }
}

/// Parse a document and return its root element. Comments, the declaration and processing instructions are dropped.
pub fn parse(src: &str) -> Result<XmlElement> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut p = Parser { src, pos: 0 };
    let mut stack: Vec<XmlElement> = Vec::new();
    let mut root: Option<XmlElement> = None;
    loop {
        if stack.is_empty() {
            p.skip_space();
        }
        if p.rest().is_empty() {
            break;
        }
        if p.eat("<?") {
            p.skip_to("?>")?;
        } else if p.eat("<!--") {
            p.skip_to("-->")?;
        } else if p.eat("<![CDATA[") {
            let body = p.skip_to("]]>")?;
            match stack.last_mut() {
                Some(top) => push_text(top, body),
                None => return err(&p, "text outside the root element"),
            }
        } else if p.rest().starts_with("<!") {
            return err(&p, "DOCTYPE and other declarations are not supported");
        } else if p.eat("</") {
            let name = p.name()?;
            p.skip_space();
            if !p.eat(">") {
                return err(&p, "expected >");
            }
            let Some(done) = stack.pop() else {
                return err(&p, "unexpected closing tag");
            };
            if done.name != name {
                return err(&p, &format!("</{name}> closes <{}>", done.name));
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(XmlNode::Element(done)),
                None => root = Some(done),
            }
        } else if p.eat("<") {
            let name = p.name()?;
            let mut element = XmlElement {
                name,
                attrs: Vec::new(),
                children: Vec::new(),
            };
            let empty = loop {
                p.skip_space();
                if p.eat("/>") {
                    break true;
                }
                if p.eat(">") {
                    break false;
                }
                let key = p.name()?;
                p.skip_space();
                if !p.eat("=") {
                    return err(&p, "expected = after an attribute name");
                }
                p.skip_space();
                let value = p.attr_value()?;
                if element.attrs.iter().any(|(k, _)| *k == key) {
                    return err(&p, &format!("attribute {key} repeated"));
                }
                element.attrs.push((key, value));
            };
            if empty {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(XmlNode::Element(element)),
                    None if root.is_none() => root = Some(element),
                    None => return err(&p, "more than one root element"),
                }
            } else {
                if stack.len() >= MAX_XML_DEPTH {
                    return err(&p, "nesting is too deep");
                }
                if stack.is_empty() && root.is_some() {
                    return err(&p, "more than one root element");
                }
                stack.push(element);
            }
        } else {
            let Some(top) = stack.last_mut() else {
                return err(&p, "text outside the root element");
            };
            let rest = p.rest();
            let end = rest.find(['<', '&']).unwrap_or(rest.len());
            if end > 0 {
                push_text(top, &rest[..end]);
                p.pos += end;
            } else {
                let mut s = String::new();
                p.entity(&mut s)?;
                push_text(stack.last_mut().expect("checked above"), &s);
            }
        }
    }
    if !stack.is_empty() {
        return err(&p, "unclosed element");
    }
    root.ok_or_else(|| Error::Xml("no root element".into()))
}

fn push_text(top: &mut XmlElement, text: &str) {
    if let Some(XmlNode::Text(last)) = top.children.last_mut() {
        last.push_str(text);
    } else {
        top.children.push(XmlNode::Text(text.to_owned()));
    }
}

/// The name of the first element in an XML document, for sorting files into categories. `None` when the bytes are not
/// text that opens with an element (truncated starts, binary data).
pub fn root_name(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(64 * 1024)];
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).ok()?,
    };
    let mut rest = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("<?") {
            rest = &r[r.find("?>")? + 2..];
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->")? + 3..];
        } else {
            let r = rest.strip_prefix('<')?;
            let end = r.find(|c: char| c.is_whitespace() || matches!(c, '/' | '>'))?;
            let name = &r[..end];
            return (!name.is_empty() && !name.starts_with('!')).then(|| name.to_owned());
        }
    }
}
