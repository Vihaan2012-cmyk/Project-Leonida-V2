//! A demangler for MSVC type names as stored in RTTI type descriptors (`.?AVFoo@Bar@@`).
//! It covers classes, structs, unions, enums, templates with type and integer arguments, and
//! pointer and reference arguments. Anything else returns `None`.

/// `A::B` form of the decorated name in a type descriptor, e.g. `.?AVB@A@@` gives `A::B`.
pub fn type_name(decorated: &str) -> Option<String> {
    let rest = decorated.strip_prefix(".?A")?;
    let rest = match rest.as_bytes().first()? {
        b'V' | b'U' | b'T' => &rest[1..],
        b'W' => rest.strip_prefix("W4")?,
        _ => return None,
    };
    let mut parser = Parser::new(rest);
    let name = parser.qualified()?;
    (parser.at_end() && !name.is_empty()).then_some(name)
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
    names: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Parser {
            text: text.as_bytes(),
            at: 0,
            names: Vec::new(),
        }
    }

    fn at_end(&self) -> bool {
        self.at == self.text.len()
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.text[self.at..].starts_with(prefix.as_bytes())
    }

    fn remember(&mut self, name: &str) {
        if self.names.len() < 10 {
            self.names.push(name.to_string());
        }
    }

    fn identifier(&mut self) -> Option<String> {
        let rest = &self.text[self.at..];
        let end = rest.iter().position(|b| *b == b'@')?;
        let name = std::str::from_utf8(&rest[..end]).ok()?.to_string();
        self.at += end + 1;
        Some(name)
    }

    /// Scopes innermost first, ended by `@`.
    fn qualified(&mut self) -> Option<String> {
        let mut parts = Vec::new();
        loop {
            if self.eat(b'@') {
                break;
            }
            parts.push(self.fragment()?);
        }
        parts.reverse();
        Some(parts.join("::"))
    }

    fn fragment(&mut self) -> Option<String> {
        let first = self.peek()?;
        if first.is_ascii_digit() {
            self.at += 1;
            return self.names.get(usize::from(first - b'0')).cloned();
        }
        if self.starts_with("?$") {
            self.at += 2;
            let outer = std::mem::take(&mut self.names);
            let name = self.identifier();
            if let Some(name) = &name {
                self.remember(name);
            }
            let args = name.and_then(|name| Some((name, self.template_arguments()?)));
            self.names = outer;
            let (name, args) = args?;
            let full = format!("{name}<{}>", args.join(","));
            self.remember(&full);
            return Some(full);
        }
        if self.starts_with("?A") {
            self.at += 2;
            self.identifier()?;
            let name = "(anonymous namespace)".to_string();
            self.remember(&name);
            return Some(name);
        }
        if first == b'?' {
            return None;
        }
        let name = self.identifier()?;
        self.remember(&name);
        Some(name)
    }

    fn template_arguments(&mut self) -> Option<Vec<String>> {
        let mut args = Vec::new();
        while !self.eat(b'@') {
            args.push(self.argument()?);
        }
        Some(args)
    }

    fn argument(&mut self) -> Option<String> {
        if self.starts_with("$$T") {
            self.at += 3;
            return Some("std::nullptr_t".into());
        }
        if self.starts_with("$$C") {
            self.at += 3;
            let qualifier = self.qualifier()?;
            let inner = self.argument()?;
            return Some(format!("{qualifier}{inner}"));
        }
        if self.starts_with("$0") {
            self.at += 2;
            return self.number().map(|n| n.to_string());
        }
        if self.starts_with("$") {
            return None;
        }
        self.value_type()
    }

    fn number(&mut self) -> Option<i64> {
        let negative = self.eat(b'?');
        let first = self.peek()?;
        let value = if first.is_ascii_digit() {
            self.at += 1;
            i64::from(first - b'0') + 1
        } else {
            let mut value: i64 = 0;
            loop {
                let digit = self.peek()?;
                self.at += 1;
                match digit {
                    b'@' => break,
                    b'A'..=b'P' => {
                        value = value
                            .checked_mul(16)?
                            .checked_add(i64::from(digit - b'A'))?
                    }
                    _ => return None,
                }
            }
            value
        };
        Some(if negative { -value } else { value })
    }

    fn qualifier(&mut self) -> Option<&'static str> {
        let text = match self.peek()? {
            b'A' => "",
            b'B' => "const ",
            b'C' => "volatile ",
            b'D' => "const volatile ",
            _ => return None,
        };
        self.at += 1;
        Some(text)
    }

    fn value_type(&mut self) -> Option<String> {
        let first = self.peek()?;
        self.at += 1;
        let builtin = match first {
            b'X' => "void",
            b'D' => "char",
            b'C' => "signed char",
            b'E' => "unsigned char",
            b'F' => "short",
            b'G' => "unsigned short",
            b'H' => "int",
            b'I' => "unsigned int",
            b'J' => "long",
            b'K' => "unsigned long",
            b'M' => "float",
            b'N' => "double",
            b'O' => "long double",
            b'V' | b'U' | b'T' => return self.qualified(),
            b'W' => {
                return if self.eat(b'4') {
                    self.qualified()
                } else {
                    None
                };
            }
            b'P' | b'Q' | b'R' | b'S' | b'A' | b'B' => return self.pointer(first),
            b'_' => {
                let second = self.peek()?;
                self.at += 1;
                match second {
                    b'N' => "bool",
                    b'J' => "__int64",
                    b'K' => "unsigned __int64",
                    b'W' => "wchar_t",
                    b'S' => "char16_t",
                    b'U' => "char32_t",
                    b'H' => "__int128",
                    b'I' => "unsigned __int128",
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(builtin.to_string())
    }

    fn pointer(&mut self, kind: u8) -> Option<String> {
        self.eat(b'E');
        let pointee = self.qualifier()?;
        let inner = self.value_type()?;
        let sigil = if matches!(kind, b'A' | b'B') {
            '&'
        } else {
            '*'
        };
        let own = match kind {
            b'Q' | b'S' => " const",
            _ => "",
        };
        Some(format!("{pointee}{inner}{sigil}{own}"))
    }
}

#[cfg(test)]
mod tests {
    use super::type_name;

    #[test]
    fn nested_scopes_read_outermost_first() {
        assert_eq!(type_name(".?AVFoo@Bar@@").as_deref(), Some("Bar::Foo"));
        assert_eq!(type_name(".?AVC@B@A@@").as_deref(), Some("A::B::C"));
    }

    #[test]
    fn structs_unions_and_enums() {
        assert_eq!(type_name(".?AUPoint@@").as_deref(), Some("Point"));
        assert_eq!(type_name(".?ATBits@Math@@").as_deref(), Some("Math::Bits"));
        assert_eq!(type_name(".?AW4Mode@Foo@@").as_deref(), Some("Foo::Mode"));
    }

    #[test]
    fn templates_with_nested_templates() {
        assert_eq!(
            type_name(".?AV?$vector@HV?$allocator@H@std@@@std@@").as_deref(),
            Some("std::vector<int,std::allocator<int>>")
        );
    }

    #[test]
    fn back_references_inside_template_arguments() {
        assert_eq!(
            type_name(".?AV?$Pair@VFoo@ns@@V1@@ns@@").as_deref(),
            Some("ns::Pair<ns::Foo,Foo>")
        );
    }

    #[test]
    fn back_references_in_the_scope_chain() {
        assert_eq!(type_name(".?AVA@A@@").as_deref(), Some("A::A"));
        assert_eq!(type_name(".?AV1@@").as_deref(), None);
    }

    #[test]
    fn integer_and_pointer_arguments() {
        assert_eq!(
            type_name(".?AV?$Array@M$0BA@@@").as_deref(),
            Some("Array<float,16>")
        );
        assert_eq!(
            type_name(".?AV?$Holder@PEAVFoo@@PEBD@@").as_deref(),
            Some("Holder<Foo*,const char*>")
        );
        assert_eq!(type_name(".?AV?$Small@$00@@").as_deref(), Some("Small<1>"));
    }

    #[test]
    fn anonymous_namespaces() {
        assert_eq!(
            type_name(".?AVImpl@?A0x1234abcd@@").as_deref(),
            Some("(anonymous namespace)::Impl")
        );
    }

    #[test]
    fn rejects_what_it_cannot_read() {
        assert_eq!(type_name("Foo"), None);
        assert_eq!(type_name(".?AVFoo"), None);
        assert_eq!(type_name(".?AV?0@@"), None);
        assert_eq!(type_name(".?AX@@"), None);
        assert_eq!(type_name(".?AVFoo@@trailing"), None);
    }
}
