//! Minimal XML parser for the waywallen-display protocol description.
//!
//! This is NOT a general-purpose XML parser. It handles exactly the
//! subset used by `protocol/waywallen_display_v1.xml`:
//!
//!   - Optional `<?xml ...?>` declaration
//!   - `<!-- ... -->` comments
//!   - Element tags with attributes (`name="value"`, `name='value'`)
//!   - Self-closing tags (`<tag ... />`)
//!   - Nested elements
//!   - No entities, no CDATA, no text content between siblings
//!
//! If the XML grows fancier features (namespaces, entities, etc.)
//! replace this with a real parser.

use std::fmt;

#[derive(Debug)]
pub struct ParseError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at byte {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(pos: usize, msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError { pos, msg: msg.into() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgType {
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
    String,
    Array(Box<ArgType>),
    KvList,
    Rect,
}

impl ArgType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "u32" => Self::U32,
            "i32" => Self::I32,
            "u64" => Self::U64,
            "i64" => Self::I64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "string" => Self::String,
            "kv_list" => Self::KvList,
            "rect" => Self::Rect,
            "array" => Self::Array(Box::new(Self::U32)), // placeholder, filled by element attr
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: String,
    pub ty: ArgType,
}

#[derive(Debug, Clone)]
pub enum FdSpec {
    None,
    Fixed(u32),
    /// Product of field names, e.g. "count * planes_per_buffer".
    Product(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub name: String,
    pub opcode: u16,
    pub args: Vec<Arg>,
    pub fds: FdSpec,
}

#[derive(Debug, Clone)]
pub struct Protocol {
    pub name: String,
    pub version: u32,
    pub requests: Vec<Message>,
    pub events: Vec<Message>,
}

/// Minimal XML DOM node.
#[derive(Debug)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

pub fn parse_protocol(src: &str) -> Result<Protocol, ParseError> {
    let mut p = Parser { src: src.as_bytes(), pos: 0 };
    p.skip_prolog()?;
    let root = p.parse_element()?;
    if root.name != "protocol" {
        return err(0, format!("root must be <protocol>, got <{}>", root.name));
    }
    let name = root
        .attr("name")
        .ok_or_else(|| ParseError { pos: 0, msg: "protocol missing name".into() })?
        .to_string();
    let version: u32 = root
        .attr("version")
        .ok_or_else(|| ParseError { pos: 0, msg: "protocol missing version".into() })?
        .parse()
        .map_err(|_| ParseError { pos: 0, msg: "protocol version not u32".into() })?;

    let mut requests = Vec::new();
    let mut events = Vec::new();
    for child in &root.children {
        match child.name.as_str() {
            "request" => requests.push(parse_message(child)?),
            "event" => events.push(parse_message(child)?),
            other => return err(0, format!("unknown top-level element <{other}>")),
        }
    }

    Ok(Protocol { name, version, requests, events })
}

fn parse_message(node: &Node) -> Result<Message, ParseError> {
    let name = node
        .attr("name")
        .ok_or_else(|| ParseError { pos: 0, msg: format!("<{}> missing name", node.name) })?
        .to_string();
    let opcode: u16 = node
        .attr("opcode")
        .ok_or_else(|| ParseError { pos: 0, msg: format!("<{}> missing opcode", node.name) })?
        .parse()
        .map_err(|_| ParseError { pos: 0, msg: "opcode not u16".into() })?;

    let mut args = Vec::new();
    let mut fds = FdSpec::None;
    for child in &node.children {
        match child.name.as_str() {
            "arg" => args.push(parse_arg(child)?),
            "fds" => fds = parse_fds(child)?,
            other => return err(0, format!("unknown message child <{other}>")),
        }
    }
    Ok(Message { name, opcode, args, fds })
}

fn parse_arg(node: &Node) -> Result<Arg, ParseError> {
    let name = node
        .attr("name")
        .ok_or_else(|| ParseError { pos: 0, msg: "<arg> missing name".into() })?
        .to_string();
    let ty_str = node
        .attr("type")
        .ok_or_else(|| ParseError { pos: 0, msg: "<arg> missing type".into() })?;
    let ty = if ty_str == "array" {
        let elem_str = node.attr("element").ok_or_else(|| ParseError {
            pos: 0,
            msg: "<arg type=\"array\"> missing element".into(),
        })?;
        let elem = ArgType::parse(elem_str).ok_or_else(|| ParseError {
            pos: 0,
            msg: format!("unknown array element type {elem_str}"),
        })?;
        match elem {
            ArgType::Array(_) | ArgType::KvList => {
                return err(0, "arrays of arrays / kv_list not supported");
            }
            _ => ArgType::Array(Box::new(elem)),
        }
    } else {
        ArgType::parse(ty_str)
            .ok_or_else(|| ParseError { pos: 0, msg: format!("unknown type {ty_str}") })?
    };
    Ok(Arg { name, ty })
}

fn parse_fds(node: &Node) -> Result<FdSpec, ParseError> {
    if let Some(s) = node.attr("count") {
        let n: u32 = s
            .parse()
            .map_err(|_| ParseError { pos: 0, msg: "fds count not u32".into() })?;
        if n == 0 {
            Ok(FdSpec::None)
        } else {
            Ok(FdSpec::Fixed(n))
        }
    } else if let Some(expr) = node.attr("count_expr") {
        // Support only "a * b * c ..." — whitespace-separated field names
        // joined by `*`. Anything fancier is rejected.
        let parts: Vec<String> = expr
            .split('*')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            return err(0, "empty count_expr");
        }
        for p in &parts {
            if !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return err(0, format!("count_expr field not an ident: {p}"));
            }
        }
        Ok(FdSpec::Product(parts))
    } else {
        err(0, "<fds> missing count or count_expr")
    }
}

// ---------- Tokenizer / tree builder ----------

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }
    fn peek(&self) -> u8 {
        self.src[self.pos]
    }
    fn starts_with(&self, needle: &[u8]) -> bool {
        self.src[self.pos..].starts_with(needle)
    }
    fn advance(&mut self, n: usize) {
        self.pos += n;
    }
    fn skip_whitespace(&mut self) {
        while !self.eof() && self.peek().is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn skip_comments_and_ws(&mut self) -> Result<(), ParseError> {
        loop {
            self.skip_whitespace();
            if self.eof() {
                return Ok(());
            }
            if self.starts_with(b"<!--") {
                self.advance(4);
                while !self.eof() && !self.starts_with(b"-->") {
                    self.advance(1);
                }
                if self.eof() {
                    return err(self.pos, "unterminated comment");
                }
                self.advance(3);
                continue;
            }
            return Ok(());
        }
    }
    fn skip_prolog(&mut self) -> Result<(), ParseError> {
        self.skip_comments_and_ws()?;
        if self.starts_with(b"<?xml") {
            while !self.eof() && !self.starts_with(b"?>") {
                self.advance(1);
            }
            if self.eof() {
                return err(self.pos, "unterminated xml decl");
            }
            self.advance(2);
        }
        self.skip_comments_and_ws()?;
        Ok(())
    }

    fn parse_element(&mut self) -> Result<Node, ParseError> {
        self.skip_comments_and_ws()?;
        if self.eof() || self.peek() != b'<' {
            return err(self.pos, "expected element");
        }
        self.advance(1);
        let name = self.parse_name()?;
        let mut attrs = Vec::new();
        loop {
            self.skip_whitespace();
            if self.eof() {
                return err(self.pos, "unterminated tag");
            }
            if self.peek() == b'/' {
                self.advance(1);
                if self.eof() || self.peek() != b'>' {
                    return err(self.pos, "expected '>' after '/'");
                }
                self.advance(1);
                return Ok(Node { name, attrs, children: Vec::new() });
            }
            if self.peek() == b'>' {
                self.advance(1);
                break;
            }
            let k = self.parse_name()?;
            self.skip_whitespace();
            if self.eof() || self.peek() != b'=' {
                return err(self.pos, "expected '='");
            }
            self.advance(1);
            self.skip_whitespace();
            let v = self.parse_attr_value()?;
            attrs.push((k, v));
        }

        let mut children = Vec::new();
        loop {
            self.skip_comments_and_ws()?;
            if self.starts_with(b"</") {
                self.advance(2);
                let close = self.parse_name()?;
                if close != name {
                    return err(self.pos, format!("mismatched close </{close}> vs <{name}>"));
                }
                self.skip_whitespace();
                if self.eof() || self.peek() != b'>' {
                    return err(self.pos, "expected '>' on close tag");
                }
                self.advance(1);
                return Ok(Node { name, attrs, children });
            }
            children.push(self.parse_element()?);
        }
    }

    fn parse_name(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        while !self.eof() {
            let c = self.peek();
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b':' {
                self.advance(1);
            } else {
                break;
            }
        }
        if start == self.pos {
            return err(self.pos, "expected name");
        }
        Ok(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
    }

    fn parse_attr_value(&mut self) -> Result<String, ParseError> {
        if self.eof() {
            return err(self.pos, "expected attribute value");
        }
        let quote = self.peek();
        if quote != b'"' && quote != b'\'' {
            return err(self.pos, "expected quote");
        }
        self.advance(1);
        let start = self.pos;
        while !self.eof() && self.peek() != quote {
            self.advance(1);
        }
        if self.eof() {
            return err(self.pos, "unterminated attribute");
        }
        let v = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        self.advance(1);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_trivial() {
        let src = r#"<?xml version="1.0"?>
            <protocol name="foo" version="1">
                <request name="hello" opcode="1">
                    <arg name="x" type="u32"/>
                </request>
            </protocol>"#;
        let p = parse_protocol(src).unwrap();
        assert_eq!(p.name, "foo");
        assert_eq!(p.version, 1);
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.requests[0].name, "hello");
        assert_eq!(p.requests[0].opcode, 1);
        assert_eq!(p.requests[0].args[0].name, "x");
        assert_eq!(p.requests[0].args[0].ty, ArgType::U32);
    }

    #[test]
    fn parse_all_types() {
        let src = r#"<protocol name="t" version="1">
            <event name="m" opcode="1">
                <arg name="a" type="u32"/>
                <arg name="b" type="i32"/>
                <arg name="c" type="u64"/>
                <arg name="d" type="i64"/>
                <arg name="e" type="f32"/>
                <arg name="f" type="f64"/>
                <arg name="g" type="string"/>
                <arg name="h" type="rect"/>
                <arg name="i" type="kv_list"/>
                <arg name="j" type="array" element="u32"/>
                <arg name="k" type="array" element="string"/>
            </event>
        </protocol>"#;
        let p = parse_protocol(src).unwrap();
        let m = &p.events[0];
        assert_eq!(m.args.len(), 11);
        assert_eq!(m.args[9].ty, ArgType::Array(Box::new(ArgType::U32)));
        assert_eq!(m.args[10].ty, ArgType::Array(Box::new(ArgType::String)));
    }

    #[test]
    fn parse_fds() {
        let src = r#"<protocol name="t" version="1">
            <event name="a" opcode="1"><fds count="1"/></event>
            <event name="b" opcode="2"><fds count="0"/></event>
            <event name="c" opcode="3"><fds count_expr="count * planes_per_buffer"/></event>
        </protocol>"#;
        let p = parse_protocol(src).unwrap();
        assert!(matches!(p.events[0].fds, FdSpec::Fixed(1)));
        assert!(matches!(p.events[1].fds, FdSpec::None));
        match &p.events[2].fds {
            FdSpec::Product(parts) => {
                assert_eq!(parts, &vec!["count".to_string(), "planes_per_buffer".to_string()]);
            }
            _ => panic!("expected Product"),
        }
    }

    #[test]
    fn comments_and_whitespace() {
        let src = r#"
            <!-- header comment -->
            <?xml version="1.0"?>
            <!-- another -->
            <protocol name="t" version="1">
                <!-- inside -->
                <request name="r" opcode="1"/>
            </protocol>"#;
        let p = parse_protocol(src).unwrap();
        assert_eq!(p.requests.len(), 1);
    }
}
