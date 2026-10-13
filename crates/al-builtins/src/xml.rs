//! XmlDocument, XmlElement, XmlAttribute, XmlText, XmlCData, XmlComment,
//! XmlProcessingInstruction, XmlDeclaration, XmlNode, XmlNodeList and
//! XmlAttributeCollection.
//!
//! Every node type is a handle onto one node tree (`Rc<RefCell<LibraryValue>>`),
//! like JSON: `XmlElement.AsXmlNode()`, `XmlNode.AsXmlElement()` and `GetRoot`
//! return handles to the same node, so an edit through any of them is visible
//! everywhere (xml/TestXmlDeep.al Remove / ReplaceWith act on nodes fetched from
//! node lists). Lists and attribute collections are views. Behaviour the corpus
//! does not pin raises `Signal::Unsupported` (see docs/probes-needed.md).

use crate::json::Handle;
use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub const XMLNS_URI: &str = "http://www.w3.org/2000/xmlns/";
const XML_URI: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug)]
pub struct XmlCell {
    pub data: XData,
    pub parent: Option<Weak<RefCell<LibraryValue>>>,
}

#[derive(Debug)]
pub enum XData {
    Document {
        decl: Option<Handle>,
        children: Vec<Handle>,
    },
    Element {
        prefix: String,
        local: String,
        ns: String,
        attrs: Vec<Handle>,
        children: Vec<Handle>,
    },
    Attribute {
        prefix: String,
        local: String,
        ns: String,
        value: String,
    },
    Text(String),
    CData(String),
    Comment(String),
    Pi {
        target: String,
        data: String,
    },
    Declaration {
        version: String,
        encoding: String,
        standalone: String,
    },
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn new_node(data: XData) -> Handle {
    let h = Rc::new(RefCell::new(LibraryValue::Xml(XmlCell { data, parent: None })));
    adopt_children(&h);
    h
}

fn tagged(name: &str, h: Handle) -> Value {
    Value::Library(name.to_string(), h)
}

fn is_xml(h: &Handle) -> bool {
    matches!(&*h.borrow(), LibraryValue::Xml(_))
}

/// The node behind an XML handle value.
fn node_of(v: &Value) -> Result<Handle, Signal> {
    match v {
        Value::Library(name, h) => {
            if is_xml(h) {
                Ok(h.clone())
            } else if matches!(&*h.borrow(), LibraryValue::Empty) {
                unsupported(format!("use of an unassigned {name}"))
            } else {
                unsupported(format!("{name} used as an XML node"))
            }
        }
        other => unsupported(format!("{other:?} used as an XML node")),
    }
}

fn recv(r: Option<&mut Value>) -> Result<Handle, Signal> {
    match r {
        Some(v) => node_of(v),
        None => unsupported("XML method without a receiver"),
    }
}

fn with<T>(h: &Handle, f: impl FnOnce(&mut XData) -> T) -> T {
    match &mut *h.borrow_mut() {
        LibraryValue::Xml(c) => f(&mut c.data),
        _ => unreachable!("checked by node_of"),
    }
}

fn parent_of(h: &Handle) -> Option<Handle> {
    match &*h.borrow() {
        LibraryValue::Xml(c) => c.parent.as_ref().and_then(Weak::upgrade),
        _ => None,
    }
}

fn set_parent(child: &Handle, parent: Option<&Handle>) {
    if let LibraryValue::Xml(c) = &mut *child.borrow_mut() {
        c.parent = parent.map(Rc::downgrade);
    }
}

fn adopt_children(h: &Handle) {
    let (kids, extra): (Vec<Handle>, Vec<Handle>) = with(h, |d| match d {
        XData::Document { decl, children } => (children.clone(), decl.iter().cloned().collect()),
        XData::Element { attrs, children, .. } => (children.clone(), attrs.clone()),
        _ => (Vec::new(), Vec::new()),
    });
    for k in kids.iter().chain(&extra) {
        set_parent(k, Some(h));
    }
}

fn children_of(h: &Handle) -> Vec<Handle> {
    with(h, |d| match d {
        XData::Document { children, .. } | XData::Element { children, .. } => children.clone(),
        _ => Vec::new(),
    })
}

fn attrs_of(h: &Handle) -> Vec<Handle> {
    with(h, |d| match d {
        XData::Element { attrs, .. } => attrs.clone(),
        _ => Vec::new(),
    })
}

fn kind(h: &Handle) -> &'static str {
    with(h, |d| match d {
        XData::Document { .. } => "XmlDocument",
        XData::Element { .. } => "XmlElement",
        XData::Attribute { .. } => "XmlAttribute",
        XData::Text(_) => "XmlText",
        XData::CData(_) => "XmlCData",
        XData::Comment(_) => "XmlComment",
        XData::Pi { .. } => "XmlProcessingInstruction",
        XData::Declaration { .. } => "XmlDeclaration",
    })
}

fn deep_clone(h: &Handle) -> Handle {
    let data = with(h, |d| match d {
        XData::Document { decl, children } => XData::Document {
            decl: decl.as_ref().map(deep_clone),
            children: children.iter().map(deep_clone).collect(),
        },
        XData::Element {
            prefix,
            local,
            ns,
            attrs,
            children,
        } => XData::Element {
            prefix: prefix.clone(),
            local: local.clone(),
            ns: ns.clone(),
            attrs: attrs.iter().map(deep_clone).collect(),
            children: children.iter().map(deep_clone).collect(),
        },
        XData::Attribute {
            prefix,
            local,
            ns,
            value,
        } => XData::Attribute {
            prefix: prefix.clone(),
            local: local.clone(),
            ns: ns.clone(),
            value: value.clone(),
        },
        XData::Text(t) => XData::Text(t.clone()),
        XData::CData(t) => XData::CData(t.clone()),
        XData::Comment(t) => XData::Comment(t.clone()),
        XData::Pi { target, data } => XData::Pi {
            target: target.clone(),
            data: data.clone(),
        },
        XData::Declaration {
            version,
            encoding,
            standalone,
        } => XData::Declaration {
            version: version.clone(),
            encoding: encoding.clone(),
            standalone: standalone.clone(),
        },
    });
    new_node(data)
}

fn root_of(h: &Handle) -> Handle {
    let mut cur = h.clone();
    while let Some(p) = parent_of(&cur) {
        cur = p;
    }
    cur
}

fn is_ancestor_or_self(maybe_ancestor: &Handle, h: &Handle) -> bool {
    let mut cur = Some(h.clone());
    while let Some(c) = cur {
        if Rc::ptr_eq(&c, maybe_ancestor) {
            return true;
        }
        cur = parent_of(&c);
    }
    false
}

// ------------------------------------------------------------- names

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\u{b7}')
}

fn is_ncname(s: &str) -> bool {
    let mut it = s.chars();
    it.next().is_some_and(is_name_start) && it.all(is_name_char)
}

fn qname(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_string()
    } else {
        format!("{prefix}:{local}")
    }
}

fn is_xmlns_decl(h: &Handle) -> bool {
    with(h, |d| matches!(d, XData::Attribute { ns, .. } if ns == XMLNS_URI))
}

/// Declared namespace of `prefix` as seen from `h` (walking up through ancestors).
fn lookup_namespace(h: &Handle, prefix: &str) -> Option<String> {
    let mut cur = Some(h.clone());
    while let Some(c) = cur {
        for a in attrs_of(&c) {
            let hit = with(&a, |d| match d {
                XData::Attribute {
                    prefix: p,
                    local,
                    ns,
                    value,
                } if ns == XMLNS_URI => {
                    // `xmlns="u"` is local `xmlns` with no prefix; `xmlns:p="u"` is prefix `xmlns`, local `p`.
                    let declared = if p.is_empty() && local == "xmlns" {
                        Some("")
                    } else if p == "xmlns" {
                        Some(local.as_str())
                    } else {
                        None
                    };
                    declared.filter(|d| *d == prefix).map(|_| value.clone())
                }
                _ => None,
            });
            if hit.is_some() {
                return hit;
            }
        }
        cur = parent_of(&c);
    }
    None
}

// ------------------------------------------------------------ parsing

#[derive(Debug)]
enum ParseError {
    Invalid,
    Unsupported(&'static str),
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    /// In-scope namespace declarations, innermost last.
    scope: Vec<(String, String)>,
    depth: usize,
}

type PResult<T> = Result<T, ParseError>;

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.i..]
    }
    fn starts(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn bump(&mut self, n: usize) {
        self.i += n;
    }
    fn skip_ws(&mut self) -> bool {
        let before = self.i;
        while self.peek().is_some_and(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) {
            self.i += 1;
        }
        self.i > before
    }
    fn expect(&mut self, p: &str) -> PResult<()> {
        if self.starts(p) {
            self.bump(p.len());
            Ok(())
        } else {
            Err(ParseError::Invalid)
        }
    }

    fn name(&mut self) -> PResult<String> {
        let start = self.i;
        let mut first = true;
        while let Some(c) = self.peek() {
            let ok = if first {
                is_name_start(c) || c == ':'
            } else {
                is_name_char(c) || c == ':'
            };
            if !ok {
                break;
            }
            first = false;
            self.i += c.len_utf8();
        }
        if self.i == start {
            Err(ParseError::Invalid)
        } else {
            Ok(self.s[start..self.i].to_string())
        }
    }

    /// Decodes entity references in `raw`; `attr` also normalises white space.
    fn decode(&self, raw: &str, attr: bool) -> PResult<String> {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '&' => {
                    let mut ent = String::new();
                    loop {
                        match chars.next() {
                            Some(';') => break,
                            Some(x) if ent.len() < 12 => ent.push(x),
                            _ => return Err(ParseError::Invalid),
                        }
                    }
                    match ent.as_str() {
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "amp" => out.push('&'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        e if e.starts_with("#x") => out.push(
                            u32::from_str_radix(&e[2..], 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or(ParseError::Invalid)?,
                        ),
                        e if e.starts_with('#') => out.push(
                            e[1..]
                                .parse::<u32>()
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or(ParseError::Invalid)?,
                        ),
                        _ => return Err(ParseError::Invalid),
                    }
                }
                '<' if attr => return Err(ParseError::Invalid),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(if attr { ' ' } else { '\n' });
                }
                '\n' | '\t' if attr => out.push(' '),
                c => out.push(c),
            }
        }
        Ok(out)
    }

    fn quoted(&mut self) -> PResult<String> {
        let q = self.peek().ok_or(ParseError::Invalid)?;
        if q != '"' && q != '\'' {
            return Err(ParseError::Invalid);
        }
        self.bump(1);
        let end = self.rest().find(q).ok_or(ParseError::Invalid)?;
        let raw = self.s[self.i..self.i + end].to_string();
        self.bump(end + 1);
        self.decode(&raw, true)
    }

    fn declaration(&mut self) -> PResult<Handle> {
        self.expect("<?xml")?;
        let (mut version, mut encoding, mut standalone) = (String::new(), String::new(), String::new());
        loop {
            let had_ws = self.skip_ws();
            if self.starts("?>") {
                self.bump(2);
                break;
            }
            if !had_ws {
                return Err(ParseError::Invalid);
            }
            let n = self.name()?;
            self.skip_ws();
            self.expect("=")?;
            self.skip_ws();
            let v = self.quoted()?;
            match n.as_str() {
                "version" => version = v,
                "encoding" => encoding = v,
                "standalone" => standalone = v,
                _ => return Err(ParseError::Invalid),
            }
        }
        if version.is_empty() {
            return Err(ParseError::Invalid);
        }
        Ok(new_node(XData::Declaration {
            version,
            encoding,
            standalone,
        }))
    }

    fn comment(&mut self) -> PResult<Handle> {
        self.expect("<!--")?;
        let end = self.rest().find("-->").ok_or(ParseError::Invalid)?;
        let body = self.s[self.i..self.i + end].to_string();
        if body.contains("--") {
            return Err(ParseError::Invalid);
        }
        self.bump(end + 3);
        Ok(new_node(XData::Comment(body)))
    }

    fn pi(&mut self) -> PResult<Handle> {
        self.expect("<?")?;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(ParseError::Invalid);
        }
        let had_ws = self.skip_ws();
        let end = self.rest().find("?>").ok_or(ParseError::Invalid)?;
        let data = self.s[self.i..self.i + end].to_string();
        if !had_ws && !data.is_empty() {
            return Err(ParseError::Invalid);
        }
        self.bump(end + 2);
        Ok(new_node(XData::Pi { target, data }))
    }

    fn resolve(&self, prefix: &str, is_attr: bool) -> PResult<String> {
        let declared = |p: &str| self.scope.iter().rev().find(|(sp, _)| sp == p).map(|(_, u)| u.clone());
        match prefix {
            // An unprefixed attribute is in no namespace; an unprefixed element is in the default one.
            "" if is_attr => Ok(String::new()),
            "" => Ok(declared("").unwrap_or_default()),
            "xml" => Ok(XML_URI.to_string()),
            "xmlns" => Ok(XMLNS_URI.to_string()),
            p => declared(p).ok_or(ParseError::Invalid),
        }
    }

    fn element(&mut self) -> PResult<Handle> {
        self.depth += 1;
        if self.depth > 200 {
            return Err(ParseError::Unsupported("XML nested deeper than 200 levels"));
        }
        self.expect("<")?;
        let qn = self.name()?;
        let mut raw_attrs: Vec<(String, String)> = Vec::new();
        let self_closed = loop {
            let had_ws = self.skip_ws();
            if self.starts("/>") {
                self.bump(2);
                break true;
            }
            if self.starts(">") {
                self.bump(1);
                break false;
            }
            if !had_ws {
                return Err(ParseError::Invalid);
            }
            let an = self.name()?;
            self.skip_ws();
            self.expect("=")?;
            self.skip_ws();
            let av = self.quoted()?;
            if raw_attrs.iter().any(|(n, _)| *n == an) {
                return Err(ParseError::Invalid);
            }
            raw_attrs.push((an, av));
        };
        // Namespace declarations first, so the element's own name sees them.
        let scope_mark = self.scope.len();
        for (n, v) in &raw_attrs {
            if n == "xmlns" {
                self.scope.push((String::new(), v.clone()));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                self.scope.push((p.to_string(), v.clone()));
            }
        }
        let split = |n: &str| match n.split_once(':') {
            Some((p, l)) => (p.to_string(), l.to_string()),
            None => (String::new(), n.to_string()),
        };
        let (prefix, local) = split(&qn);
        if local.contains(':') || !is_ncname(&local) || (!prefix.is_empty() && !is_ncname(&prefix)) {
            return Err(ParseError::Invalid);
        }
        let ns = self.resolve(&prefix, false)?;
        let mut attrs = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        for (n, v) in raw_attrs {
            let (ap, al, ans) = if n == "xmlns" {
                (String::new(), "xmlns".to_string(), XMLNS_URI.to_string())
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                ("xmlns".to_string(), p.to_string(), XMLNS_URI.to_string())
            } else {
                let (p, l) = split(&n);
                if l.contains(':') || !is_ncname(&l) || (!p.is_empty() && !is_ncname(&p)) {
                    return Err(ParseError::Invalid);
                }
                let u = self.resolve(&p, true)?;
                (p, l, u)
            };
            if seen.contains(&(ans.clone(), al.clone())) {
                return Err(ParseError::Invalid);
            }
            seen.push((ans.clone(), al.clone()));
            attrs.push(new_node(XData::Attribute {
                prefix: ap,
                local: al,
                ns: ans,
                value: v,
            }));
        }
        let mut children = Vec::new();
        if !self_closed {
            loop {
                if self.starts("</") {
                    self.bump(2);
                    let close = self.name()?;
                    self.skip_ws();
                    self.expect(">")?;
                    if close != qn {
                        return Err(ParseError::Invalid);
                    }
                    break;
                } else if self.starts("<!--") {
                    children.push(self.comment()?);
                } else if self.starts("<![CDATA[") {
                    self.bump(9);
                    let end = self.rest().find("]]>").ok_or(ParseError::Invalid)?;
                    let body = self.s[self.i..self.i + end].replace("\r\n", "\n").replace('\r', "\n");
                    self.bump(end + 3);
                    children.push(new_node(XData::CData(body)));
                } else if self.starts("<?") {
                    children.push(self.pi()?);
                } else if self.starts("<!") {
                    return Err(ParseError::Invalid);
                } else if self.starts("<") {
                    children.push(self.element()?);
                } else if self.peek().is_none() {
                    return Err(ParseError::Invalid);
                } else {
                    let end = self.rest().find('<').unwrap_or(self.rest().len());
                    let raw = self.s[self.i..self.i + end].to_string();
                    self.bump(end);
                    if raw.contains("]]>") {
                        return Err(ParseError::Invalid);
                    }
                    let text = self.decode(&raw, false)?;
                    if text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) {
                        // Whether insignificant white space is kept is not pinned by the corpus.
                        return Err(ParseError::Unsupported("white-space-only text inside an XML element"));
                    }
                    children.push(new_node(XData::Text(text)));
                }
            }
        }
        self.scope.truncate(scope_mark);
        self.depth -= 1;
        Ok(new_node(XData::Element {
            prefix,
            local,
            ns,
            attrs,
            children,
        }))
    }

    fn document(&mut self) -> PResult<Handle> {
        if self.starts("\u{feff}") {
            self.bump(3);
        }
        let mut decl = None;
        if self.starts("<?xml") && self.rest()[5..].starts_with([' ', '\t', '\r', '\n']) {
            decl = Some(self.declaration()?);
        }
        let mut children = Vec::new();
        let mut root_seen = false;
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                break;
            }
            if self.starts("<!--") {
                children.push(self.comment()?);
            } else if self.starts("<?") {
                children.push(self.pi()?);
            } else if self.starts("<!DOCTYPE") {
                return Err(ParseError::Unsupported("DOCTYPE declarations"));
            } else if self.starts("<") && !root_seen {
                children.push(self.element()?);
                root_seen = true;
            } else {
                return Err(ParseError::Invalid);
            }
        }
        if !root_seen {
            return Err(ParseError::Invalid);
        }
        Ok(new_node(XData::Document { decl, children }))
    }
}

/// `Ok(None)`: not well-formed XML.
fn parse_document(text: &str) -> Result<Option<Handle>, Signal> {
    let mut p = Parser {
        s: text,
        i: 0,
        scope: Vec::new(),
        depth: 0,
    };
    match p.document() {
        Ok(d) => Ok(Some(d)),
        Err(ParseError::Invalid) => Ok(None),
        Err(ParseError::Unsupported(w)) => unsupported(w),
    }
}

// ---------------------------------------------------------- serialising

fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            '\t' => out.push_str("&#x9;"),
            c => out.push(c),
        }
    }
}

/// The prefix mapping visible while writing: `(prefix, uri)`, innermost last.
type Scope = Vec<(String, String)>;

fn mapped(scope: &Scope, prefix: &str) -> Option<String> {
    scope.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.clone())
}

fn write_node(h: &Handle, out: &mut String, scope: &mut Scope) -> Result<(), Signal> {
    enum Plan {
        Element {
            prefix: String,
            local: String,
            ns: String,
            attrs: Vec<Handle>,
            children: Vec<Handle>,
        },
        Leaf(String),
        Skip,
    }
    let plan = with(h, |d| match d {
        XData::Element {
            prefix,
            local,
            ns,
            attrs,
            children,
        } => Plan::Element {
            prefix: prefix.clone(),
            local: local.clone(),
            ns: ns.clone(),
            attrs: attrs.clone(),
            children: children.clone(),
        },
        XData::Text(t) => {
            let mut s = String::new();
            escape_text(t, &mut s);
            Plan::Leaf(s)
        }
        XData::CData(t) => Plan::Leaf(format!("<![CDATA[{t}]]>")),
        XData::Comment(t) => Plan::Leaf(format!("<!--{t}-->")),
        XData::Pi { target, data } => Plan::Leaf(if data.is_empty() {
            format!("<?{target}?>")
        } else {
            format!("<?{target} {data}?>")
        }),
        XData::Attribute { .. } | XData::Declaration { .. } | XData::Document { .. } => Plan::Skip,
    });
    match plan {
        Plan::Leaf(s) => out.push_str(&s),
        Plan::Skip => return unsupported(format!("serialising {} as markup", kind(h))),
        Plan::Element {
            prefix,
            local,
            ns,
            attrs,
            children,
        } => {
            let mark = scope.len();
            // Declarations written on this element: explicit ones first, in their order.
            let mut written: Vec<String> = Vec::new();
            let mut plain: Vec<(String, String, String, String)> = Vec::new(); // prefix, local, ns, value
            for a in &attrs {
                let (ap, al, ans, av) = with(a, |d| match d {
                    XData::Attribute {
                        prefix,
                        local,
                        ns,
                        value,
                    } => (prefix.clone(), local.clone(), ns.clone(), value.clone()),
                    _ => unreachable!("attributes hold attributes"),
                });
                if ans == XMLNS_URI {
                    let declared = if ap == "xmlns" { al.clone() } else { String::new() };
                    scope.push((declared.clone(), av.clone()));
                    let mut s = String::new();
                    escape_attr(&av, &mut s);
                    written.push(if declared.is_empty() {
                        format!(" xmlns=\"{s}\"")
                    } else {
                        format!(" xmlns:{declared}=\"{s}\"")
                    });
                } else {
                    plain.push((ap, al, ans, av));
                }
            }
            // The element's own name may need a declaration the tree does not carry.
            let mut extra = String::new();
            let visible = mapped(scope, &prefix);
            if visible.as_deref().unwrap_or("") != ns {
                let mut s = String::new();
                escape_attr(&ns, &mut s);
                extra = if prefix.is_empty() {
                    format!(" xmlns=\"{s}\"")
                } else {
                    format!(" xmlns:{prefix}=\"{s}\"")
                };
                scope.push((prefix.clone(), ns.clone()));
            }
            out.push('<');
            out.push_str(&qname(&prefix, &local));
            out.push_str(&extra);
            for w in &written {
                out.push_str(w);
            }
            for (ap, al, ans, av) in &plain {
                let use_prefix = if ans.is_empty() {
                    String::new()
                } else if !ap.is_empty() && mapped(scope, ap).as_deref() == Some(ans.as_str()) {
                    ap.clone()
                } else if let Some((p, _)) = scope.iter().rev().find(|(p, u)| !p.is_empty() && u == ans) {
                    p.clone()
                } else {
                    return unsupported("serialising a namespaced attribute without a declared prefix");
                };
                let mut s = String::new();
                escape_attr(av, &mut s);
                out.push_str(&format!(" {}=\"{s}\"", qname(&use_prefix, al)));
            }
            if children.is_empty() {
                out.push_str(" />");
            } else {
                out.push('>');
                for c in &children {
                    write_node(c, out, scope)?;
                }
                out.push_str("</");
                out.push_str(&qname(&prefix, &local));
                out.push('>');
            }
            scope.truncate(mark);
        }
    }
    Ok(())
}

/// The namespace declarations of `h`'s ancestors do not matter when a node is written
/// on its own: it declares what it needs.
fn serialize(h: &Handle) -> Result<String, Signal> {
    let mut out = String::new();
    let is_doc = with(h, |d| matches!(d, XData::Document { .. }));
    if is_doc {
        let has_decl = with(h, |d| matches!(d, XData::Document { decl: Some(_), .. }));
        if has_decl {
            return unsupported("writing a document that has an XML declaration (output form not pinned yet)");
        }
        for c in children_of(h) {
            write_node(&c, &mut out, &mut Vec::new())?;
        }
    } else {
        write_node(h, &mut out, &mut Vec::new())?;
    }
    Ok(out)
}

fn inner_text(h: &Handle, out: &mut String) {
    for c in children_of(h) {
        let (leaf, is_el) = with(&c, |d| match d {
            XData::Text(t) | XData::CData(t) => (Some(t.clone()), false),
            XData::Element { .. } => (None, true),
            _ => (None, false),
        });
        if let Some(t) = leaf {
            out.push_str(&t);
        } else if is_el {
            inner_text(&c, out);
        }
    }
}

// ------------------------------------------------------------- content

/// Nodes from `Content: Any*` arguments; a node that already has a parent is copied.
fn content_nodes(args: &[Value]) -> Result<Vec<Handle>, Signal> {
    let mut out = Vec::new();
    for a in args {
        let h = match a {
            Value::Library(_, h) if is_xml(h) => h.clone(),
            other => return unsupported(format!("{other:?} as XML content (only XML nodes are pinned)")),
        };
        if with(&h, |d| matches!(d, XData::Document { .. })) {
            return unsupported("adding an XmlDocument as content");
        }
        out.push(if parent_of(&h).is_some() { deep_clone(&h) } else { h });
    }
    Ok(out)
}

fn check_fits(container: &Handle, node: &Handle) -> Result<(), Signal> {
    if is_ancestor_or_self(node, container) {
        return unsupported("adding an XML node to itself or its own descendant");
    }
    let c_kind = kind(container);
    let n_kind = kind(node);
    let ok = match (c_kind, n_kind) {
        ("XmlDocument", "XmlComment" | "XmlProcessingInstruction" | "XmlDeclaration") => true,
        ("XmlDocument", "XmlElement") => {
            let has_root = children_of(container)
                .iter()
                .any(|c| with(c, |d| matches!(d, XData::Element { .. })));
            if has_root {
                return unsupported("a second root element in an XmlDocument (error text not pinned yet)");
            }
            true
        }
        ("XmlElement", "XmlElement" | "XmlText" | "XmlCData" | "XmlComment" | "XmlProcessingInstruction") => true,
        ("XmlElement", "XmlAttribute") => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        unsupported(format!("adding {n_kind} to {c_kind} (error text not pinned yet)"))
    }
}

/// Inserts `nodes` into `container` at child position `at` (`None` appends).
fn insert(container: &Handle, at: Option<usize>, nodes: Vec<Handle>) -> Result<(), Signal> {
    for n in &nodes {
        check_fits(container, n)?;
    }
    let mut at = at;
    for n in nodes {
        match kind(&n) {
            "XmlAttribute" => {
                let dup = attrs_of(container).iter().any(|a| same_attr_name(a, &n));
                if dup {
                    return unsupported("adding an attribute that already exists (error text not pinned yet)");
                }
                with(container, |d| {
                    if let XData::Element { attrs, .. } = d {
                        attrs.push(n.clone());
                    }
                });
            }
            "XmlDeclaration" => {
                with(container, |d| {
                    if let XData::Document { decl, .. } = d {
                        *decl = Some(n.clone());
                    }
                });
            }
            _ => with(container, |d| {
                if let XData::Document { children, .. } | XData::Element { children, .. } = d {
                    let pos = at.unwrap_or(children.len()).min(children.len());
                    children.insert(pos, n.clone());
                    at = Some(pos + 1);
                }
            }),
        }
        set_parent(&n, Some(container));
    }
    Ok(())
}

fn same_attr_name(a: &Handle, b: &Handle) -> bool {
    let key = |h: &Handle| {
        with(h, |d| match d {
            XData::Attribute { local, ns, .. } => (local.clone(), ns.clone()),
            _ => (String::new(), String::new()),
        })
    };
    key(a) == key(b)
}

fn detach(h: &Handle) {
    let Some(p) = parent_of(h) else { return };
    with(&p, |d| match d {
        XData::Document { decl, children } => {
            children.retain(|c| !Rc::ptr_eq(c, h));
            if decl.as_ref().is_some_and(|x| Rc::ptr_eq(x, h)) {
                *decl = None;
            }
        }
        XData::Element { attrs, children, .. } => {
            children.retain(|c| !Rc::ptr_eq(c, h));
            attrs.retain(|c| !Rc::ptr_eq(c, h));
        }
        _ => {}
    });
    set_parent(h, None);
}

fn position_in_parent(h: &Handle) -> Result<(Handle, usize), Signal> {
    let Some(p) = parent_of(h) else {
        return unsupported("a sibling operation on a node without a parent (error text not pinned yet)");
    };
    let pos = children_of(&p).iter().position(|c| Rc::ptr_eq(c, h));
    match pos {
        Some(i) => Ok((p, i)),
        None => unsupported("a sibling operation on an attribute or declaration"),
    }
}

// ------------------------------------------------------------- XPath

#[derive(Debug, Clone)]
enum Test {
    Name(String),
    Any,
    Text,
    Node,
    Comment,
}

#[derive(Debug, Clone)]
enum Pred {
    Index(usize),
    AttrEq(String, String),
    AttrExists(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Child,
    DescendantOrSelf,
    ThisNode,
    Parent,
    Attribute,
}

#[derive(Debug, Clone)]
struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<Pred>,
}

fn xpath_unsupported<T>(p: &str, why: &str) -> Result<T, Signal> {
    unsupported(format!("XPath `{p}`: {why} (only simple location paths are pinned)"))
}

/// Returns (absolute, steps).
fn parse_xpath(p: &str) -> Result<(bool, Vec<Step>), Signal> {
    let b: Vec<char> = p.chars().collect();
    let mut i = 0;
    let mut steps = Vec::new();
    let absolute = b.first() == Some(&'/');
    let mut first = true;
    while i < b.len() {
        if b[i] == '/' {
            i += 1;
            if b.get(i) == Some(&'/') {
                i += 1;
                steps.push(Step {
                    axis: Axis::DescendantOrSelf,
                    test: Test::Node,
                    preds: vec![],
                });
            }
        } else if !first {
            return xpath_unsupported(p, "unexpected character");
        }
        first = false;
        if i >= b.len() {
            if steps.is_empty() {
                break;
            }
            return xpath_unsupported(p, "trailing /");
        }
        let (axis, test);
        if b[i] == '.' {
            if b.get(i + 1) == Some(&'.') {
                i += 2;
                axis = Axis::Parent;
            } else {
                i += 1;
                axis = Axis::ThisNode;
            }
            test = Test::Node;
        } else if b[i] == '@' {
            i += 1;
            let s = i;
            while i < b.len() && (is_name_char(b[i]) || b[i] == '*') {
                i += 1;
            }
            let n: String = b[s..i].iter().collect();
            if n.is_empty() {
                return xpath_unsupported(p, "empty attribute name");
            }
            axis = Axis::Attribute;
            test = if n == "*" { Test::Any } else { Test::Name(n) };
        } else if b[i] == '*' {
            i += 1;
            axis = Axis::Child;
            test = Test::Any;
        } else if is_name_start(b[i]) {
            let s = i;
            while i < b.len() && (is_name_char(b[i]) || b[i] == ':' || b[i] == '(' || b[i] == ')') {
                i += 1;
            }
            let n: String = b[s..i].iter().collect();
            axis = Axis::Child;
            test = match n.as_str() {
                "text()" => Test::Text,
                "node()" => Test::Node,
                "comment()" => Test::Comment,
                n if n.contains(':') => return xpath_unsupported(p, "prefixed names need a namespace manager"),
                n if n.contains(['(', ')']) => return xpath_unsupported(p, "functions"),
                n => Test::Name(n.to_string()),
            };
        } else {
            return xpath_unsupported(p, "unsupported syntax");
        }
        let mut preds = Vec::new();
        while b.get(i) == Some(&'[') {
            let close = b[i..]
                .iter()
                .position(|c| *c == ']')
                .map(|c| c + i)
                .ok_or_else(|| Signal::Unsupported(format!("XPath `{p}`: unclosed [")))?;
            let inner: String = b[i + 1..close].iter().collect();
            i = close + 1;
            let inner = inner.trim();
            if let Ok(n) = inner.parse::<usize>() {
                preds.push(Pred::Index(n));
            } else if let Some(rest) = inner.strip_prefix('@') {
                match rest.split_once('=') {
                    Some((n, v)) => {
                        let v = v.trim();
                        let lit = v
                            .strip_prefix('\'')
                            .and_then(|s| s.strip_suffix('\''))
                            .or_else(|| v.strip_prefix('"').and_then(|s| s.strip_suffix('"')));
                        match lit {
                            Some(l) if is_ncname(n.trim()) => preds.push(Pred::AttrEq(n.trim().into(), l.into())),
                            _ => return xpath_unsupported(p, "predicate form"),
                        }
                    }
                    None if is_ncname(rest) => preds.push(Pred::AttrExists(rest.into())),
                    None => return xpath_unsupported(p, "predicate form"),
                }
            } else {
                return xpath_unsupported(p, "predicate form");
            }
        }
        steps.push(Step { axis, test, preds });
    }
    if steps.is_empty() && !absolute {
        return xpath_unsupported(p, "empty path");
    }
    Ok((absolute, steps))
}

fn node_matches(h: &Handle, test: &Test, principal_attr: bool) -> bool {
    with(h, |d| match (d, test) {
        (_, Test::Node) => true,
        (XData::Element { local, ns, .. }, Test::Name(n)) if !principal_attr => ns.is_empty() && local == n,
        (XData::Attribute { local, ns, .. }, Test::Name(n)) if principal_attr => ns.is_empty() && local == n,
        (XData::Element { .. }, Test::Any) => !principal_attr,
        (XData::Attribute { ns, .. }, Test::Any) => principal_attr && ns != XMLNS_URI,
        (XData::Text(_) | XData::CData(_), Test::Text) => true,
        (XData::Comment(_), Test::Comment) => true,
        _ => false,
    })
}

fn attr_value(h: &Handle, name: &str) -> Option<String> {
    attrs_of(h).iter().find_map(|a| {
        with(a, |d| match d {
            XData::Attribute { local, ns, value, .. } if ns.is_empty() && local == name => Some(value.clone()),
            _ => None,
        })
    })
}

fn descendants_or_self(h: &Handle, out: &mut Vec<Handle>) {
    out.push(h.clone());
    for c in children_of(h) {
        descendants_or_self(&c, out);
    }
}

fn eval_step(ctx: &Handle, step: &Step) -> Vec<Handle> {
    let mut found: Vec<Handle> = match step.axis {
        Axis::Child => children_of(ctx)
            .into_iter()
            .filter(|c| node_matches(c, &step.test, false))
            .collect(),
        Axis::Attribute => attrs_of(ctx)
            .into_iter()
            .filter(|a| node_matches(a, &step.test, true))
            .collect(),
        Axis::ThisNode => vec![ctx.clone()],
        Axis::Parent => parent_of(ctx).into_iter().collect(),
        Axis::DescendantOrSelf => {
            let mut v = Vec::new();
            descendants_or_self(ctx, &mut v);
            v
        }
    };
    for p in &step.preds {
        found = match p {
            Pred::Index(n) => found.get(n.wrapping_sub(1)).cloned().into_iter().collect(),
            Pred::AttrEq(n, v) => found
                .into_iter()
                .filter(|h| attr_value(h, n).as_deref() == Some(v.as_str()))
                .collect(),
            Pred::AttrExists(n) => found.into_iter().filter(|h| attr_value(h, n).is_some()).collect(),
        };
    }
    found
}

fn xpath(ctx: &Handle, path: &str) -> Result<Vec<Handle>, Signal> {
    let (absolute, steps) = parse_xpath(path)?;
    let mut current = vec![if absolute { root_of(ctx) } else { ctx.clone() }];
    for step in &steps {
        let mut next: Vec<Handle> = Vec::new();
        for c in &current {
            for n in eval_step(c, step) {
                if !next.iter().any(|e| Rc::ptr_eq(e, &n)) {
                    next.push(n);
                }
            }
        }
        current = next;
    }
    Ok(current)
}

// ------------------------------------------------------------- natives

type NResult = Result<Value, Signal>;

fn list_of(items: Vec<Handle>) -> Value {
    Value::Library(
        "XmlNodeList".into(),
        Rc::new(RefCell::new(LibraryValue::XmlList(items))),
    )
}

fn arg_text(a: &[Value], i: usize) -> Result<String, Signal> {
    a.get(i)
        .and_then(crate::native::text_of)
        .ok_or_else(|| Signal::Unsupported(format!("XML argument {i} is not text")))
}

fn read_from_text(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let text = arg_text(a, 0)?;
    match parse_document(&text)? {
        Some(doc) => {
            a[1] = tagged("XmlDocument", doc);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("XmlDocument.ReadFrom of invalid XML, result unused (error text not pinned yet)"),
    }
}

fn doc_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let doc = new_node(XData::Document {
        decl: None,
        children: Vec::new(),
    });
    insert(&doc, None, content_nodes(a)?)?;
    Ok(tagged("XmlDocument", doc))
}

fn check_name(n: &str) -> Result<(), Signal> {
    if is_ncname(n) {
        Ok(())
    } else {
        unsupported(format!("XML name `{n}` (only plain names without a prefix are pinned)"))
    }
}

fn element_create_name(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let name = arg_text(a, 0)?;
    check_name(&name)?;
    let el = new_node(XData::Element {
        prefix: String::new(),
        local: name,
        ns: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
    });
    insert(&el, None, content_nodes(&a[1..])?)?;
    Ok(tagged("XmlElement", el))
}

fn element_create_ns(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (local, ns) = (arg_text(a, 0)?, arg_text(a, 1)?);
    check_name(&local)?;
    let el = new_node(XData::Element {
        prefix: String::new(),
        local,
        ns,
        attrs: Vec::new(),
        children: Vec::new(),
    });
    insert(&el, None, content_nodes(&a[2..])?)?;
    Ok(tagged("XmlElement", el))
}

fn plain_attribute(name: String, value: String) -> NResult {
    check_name(&name)?;
    Ok(tagged(
        "XmlAttribute",
        new_node(XData::Attribute {
            prefix: String::new(),
            local: name,
            ns: String::new(),
            value,
        }),
    ))
}

fn attribute_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    plain_attribute(arg_text(a, 0)?, arg_text(a, 1)?)
}

fn attribute_create_ns(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (local, ns, value) = (arg_text(a, 0)?, arg_text(a, 1)?, arg_text(a, 2)?);
    if ns.is_empty() {
        return plain_attribute(local, value);
    }
    unsupported("XmlAttribute.Create with a namespace (the prefix it would get is not pinned)")
}

fn attribute_create_ns_decl(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    // xml/TestXmlDeep.al XmlAttribute_CreateNamespaceDeclaration_HasPrefixAndUri: prefix `xmlns`.
    let (prefix, uri) = (arg_text(a, 0)?, arg_text(a, 1)?);
    let (p, l) = if prefix.is_empty() {
        (String::new(), "xmlns".to_string())
    } else {
        check_name(&prefix)?;
        ("xmlns".to_string(), prefix)
    };
    Ok(tagged(
        "XmlAttribute",
        new_node(XData::Attribute {
            prefix: p,
            local: l,
            ns: XMLNS_URI.into(),
            value: uri,
        }),
    ))
}

fn text_like_create(tag: &'static str, a: &[Value]) -> NResult {
    let t = arg_text(a, 0)?;
    Ok(tagged(
        tag,
        new_node(match tag {
            "XmlText" => XData::Text(t),
            "XmlCData" => XData::CData(t),
            _ => XData::Comment(t),
        }),
    ))
}
fn text_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    text_like_create("XmlText", a)
}
fn cdata_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    text_like_create("XmlCData", a)
}
fn comment_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    text_like_create("XmlComment", a)
}

fn pi_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    Ok(tagged(
        "XmlProcessingInstruction",
        new_node(XData::Pi {
            target: arg_text(a, 0)?,
            data: arg_text(a, 1)?,
        }),
    ))
}

fn decl_create(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    Ok(tagged(
        "XmlDeclaration",
        new_node(XData::Declaration {
            version: arg_text(a, 0)?,
            encoding: arg_text(a, 1)?,
            standalone: arg_text(a, 2)?,
        }),
    ))
}

macro_rules! decl_prop {
    ($name:ident, $field:ident) => {
        fn $name(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
            let h = recv(r)?;
            if let Some(v) = a.first() {
                let t = crate::native::text_of(v)
                    .ok_or_else(|| Signal::Unsupported("non-text declaration value".into()))?;
                with(&h, |d| {
                    if let XData::Declaration { $field, .. } = d {
                        *$field = t;
                    }
                });
                return Ok(Value::Null);
            }
            Ok(Value::Text(with(&h, |d| match d {
                XData::Declaration { $field, .. } => $field.clone(),
                _ => String::new(),
            })))
        }
    };
}
decl_prop!(decl_version, version);
decl_prop!(decl_encoding, encoding);
decl_prop!(decl_standalone, standalone);

fn value_prop(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    if let Some(v) = a.first() {
        let t = crate::native::text_of(v).ok_or_else(|| Signal::Unsupported("non-text XML value".into()))?;
        with(&h, |d| match d {
            XData::Attribute { value, .. } => *value = t,
            XData::Text(s) | XData::CData(s) | XData::Comment(s) => *s = t,
            _ => {}
        });
        return Ok(Value::Null);
    }
    Ok(Value::Text(with(&h, |d| match d {
        XData::Attribute { value, .. } => value.clone(),
        XData::Text(s) | XData::CData(s) | XData::Comment(s) => s.clone(),
        _ => String::new(),
    })))
}

fn pi_get_target(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    a[0] = Value::Text(with(&h, |d| match d {
        XData::Pi { target, .. } => target.clone(),
        _ => String::new(),
    }));
    Ok(Value::Boolean(true))
}

fn pi_get_data(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    a[0] = Value::Text(with(&h, |d| match d {
        XData::Pi { data, .. } => data.clone(),
        _ => String::new(),
    }));
    Ok(Value::Boolean(true))
}

// ----- names

fn element_name(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Text(with(&h, |d| match d {
        XData::Element { prefix, local, .. } | XData::Attribute { prefix, local, .. } => qname(prefix, local),
        _ => String::new(),
    })))
}

fn local_name(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Text(with(&h, |d| match d {
        XData::Element { local, .. } | XData::Attribute { local, .. } => local.clone(),
        _ => String::new(),
    })))
}

fn namespace_uri(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Text(with(&h, |d| match d {
        XData::Element { ns, .. } | XData::Attribute { ns, .. } => ns.clone(),
        _ => String::new(),
    })))
}

fn namespace_prefix(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Text(with(&h, |d| match d {
        XData::Attribute { prefix, .. } => prefix.clone(),
        _ => String::new(),
    })))
}

fn is_namespace_declaration(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Boolean(is_xmlns_decl(&recv(r)?)))
}

fn get_prefix_of_namespace(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let uri = arg_text(a, 0)?;
    let mut cur = Some(h);
    while let Some(e) = cur {
        for at in attrs_of(&e) {
            let hit = with(&at, |d| match d {
                XData::Attribute {
                    prefix,
                    local,
                    ns,
                    value,
                } if ns == XMLNS_URI && *value == uri => {
                    Some(if prefix == "xmlns" { Some(local.clone()) } else { None })
                }
                _ => None,
            });
            match hit {
                Some(Some(p)) => {
                    a[1] = Value::Text(p);
                    return Ok(Value::Boolean(true));
                }
                Some(None) => {
                    return unsupported("GetPrefixOfNamespace for a default namespace (result not pinned yet)");
                }
                None => {}
            }
        }
        cur = parent_of(&e);
    }
    if c.ret_used() {
        Ok(Value::Boolean(false))
    } else {
        unsupported("GetPrefixOfNamespace without a match, result unused (error text not pinned yet)")
    }
}

fn get_namespace_of_prefix(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let prefix = arg_text(a, 0)?;
    if prefix.is_empty() {
        return unsupported("GetNamespaceOfPrefix for the empty prefix (result not pinned yet)");
    }
    match lookup_namespace(&h, &prefix) {
        Some(u) => {
            a[1] = Value::Text(u);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("GetNamespaceOfPrefix without a match, result unused (error text not pinned yet)"),
    }
}

// ----- structure

fn add(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    insert(&h, None, content_nodes(a)?)?;
    Ok(Value::Boolean(true))
}

fn add_first(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    insert(&h, Some(0), content_nodes(a)?)?;
    Ok(Value::Boolean(true))
}

fn add_after_self(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let (p, i) = position_in_parent(&h)?;
    insert(&p, Some(i + 1), content_nodes(a)?)?;
    Ok(Value::Boolean(true))
}

fn add_before_self(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let (p, i) = position_in_parent(&h)?;
    insert(&p, Some(i), content_nodes(a)?)?;
    Ok(Value::Boolean(true))
}

fn remove(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    if parent_of(&h).is_none() {
        return if c.ret_used() {
            Ok(Value::Boolean(false))
        } else {
            unsupported("Remove of a node without a parent, result unused (error text not pinned yet)")
        };
    }
    detach(&h);
    Ok(Value::Boolean(true))
}

fn replace_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let (p, i) = position_in_parent(&h)?;
    let nodes = content_nodes(a)?;
    for n in &nodes {
        check_fits(&p, n)?;
    }
    detach(&h);
    insert(&p, Some(i), nodes)?;
    Ok(Value::Boolean(true))
}

fn remove_nodes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    for c in children_of(&h) {
        detach(&c);
    }
    Ok(Value::Null)
}

fn replace_nodes(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let nodes = content_nodes(a)?;
    for n in &nodes {
        check_fits(&h, n)?;
    }
    remove_nodes(c, Some(&mut tagged("XmlElement", h.clone())), &mut [])?;
    insert(&h, None, nodes)?;
    Ok(Value::Boolean(true))
}

fn get_root(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let root = children_of(&h)
        .into_iter()
        .find(|k| with(k, |d| matches!(d, XData::Element { .. })));
    match root {
        Some(e) => {
            a[0] = tagged("XmlElement", e);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("GetRoot of a document without a root, result unused (error text not pinned yet)"),
    }
}

fn get_declaration(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let d = with(&h, |d| match d {
        XData::Document { decl, .. } => decl.clone(),
        _ => None,
    });
    match d {
        Some(x) => {
            a[0] = tagged("XmlDeclaration", x);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("GetDeclaration without a declaration, result unused (error text not pinned yet)"),
    }
}

fn set_declaration(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let d = node_of(&a[0])?;
    if kind(&d) != "XmlDeclaration" {
        return unsupported("SetDeclaration with something that is not an XmlDeclaration");
    }
    let d = if parent_of(&d).is_some() { deep_clone(&d) } else { d };
    if let Some(old) = with(&h, |x| match x {
        XData::Document { decl, .. } => decl.take(),
        _ => None,
    }) {
        set_parent(&old, None);
    }
    with(&h, |x| {
        if let XData::Document { decl, .. } = x {
            *decl = Some(d.clone());
        }
    });
    set_parent(&d, Some(&h));
    Ok(Value::Boolean(true))
}

fn get_document_type(_: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> NResult {
    // xml/TestXmlComplete.al XmlDocument_GetDocumentType_IsCallable pins only that it throws.
    unsupported("XmlDocument.GetDocumentType (document types are not modelled; the error text is not pinned)")
}

fn get_document(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let root = root_of(&h);
    if with(&root, |d| matches!(d, XData::Document { .. })) {
        a[0] = tagged("XmlDocument", root);
        Ok(Value::Boolean(true))
    } else if c.ret_used() {
        Ok(Value::Boolean(false))
    } else {
        unsupported("GetDocument of a node outside a document, result unused (error text not pinned yet)")
    }
}

fn get_parent(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    match parent_of(&h).filter(|p| with(p, |d| matches!(d, XData::Element { .. }))) {
        Some(p) => {
            a[0] = tagged("XmlElement", p);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("GetParent without a parent element, result unused (error text not pinned yet)"),
    }
}

fn as_token(tag: &'static str, r: Option<&mut Value>) -> NResult {
    Ok(tagged(tag, recv(r)?))
}
fn as_xml_node(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    as_token("XmlNode", r)
}

macro_rules! node_kind {
    ($is:ident, $as:ident, $kind:literal) => {
        fn $is(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
            let h = recv(r)?;
            Ok(Value::Boolean(kind(&h) == $kind))
        }
        fn $as(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
            let h = recv(r)?;
            if kind(&h) == $kind {
                Ok(tagged($kind, h))
            } else {
                unsupported(format!(
                    "{} requested from {} (error text not pinned yet)",
                    $kind,
                    kind(&h)
                ))
            }
        }
    };
}
node_kind!(is_attribute, as_attribute, "XmlAttribute");
node_kind!(is_cdata, as_cdata, "XmlCData");
node_kind!(is_comment, as_comment, "XmlComment");
node_kind!(is_declaration, as_declaration, "XmlDeclaration");
node_kind!(is_document, as_document, "XmlDocument");
node_kind!(is_element, as_element, "XmlElement");
node_kind!(is_pi, as_pi, "XmlProcessingInstruction");

/// `IsXmlText` is true for a text node; whether a CDATA section also counts is not pinned.
fn is_text(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    match kind(&h) {
        "XmlText" => Ok(Value::Boolean(true)),
        "XmlCData" => unsupported("XmlNode.IsXmlText on a CDATA section (not pinned yet)"),
        _ => Ok(Value::Boolean(false)),
    }
}
fn as_text(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    if kind(&h) == "XmlText" {
        Ok(tagged("XmlText", h))
    } else {
        unsupported(format!(
            "XmlText requested from {} (error text not pinned yet)",
            kind(&h)
        ))
    }
}

fn has_attributes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Boolean(!attrs_of(&recv(r)?).is_empty()))
}

fn has_elements(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Boolean(
        children_of(&h)
            .iter()
            .any(|c| with(c, |d| matches!(d, XData::Element { .. }))),
    ))
}

fn is_empty(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Boolean(children_of(&recv(r)?).is_empty()))
}

fn inner_text_fn(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let mut s = String::new();
    inner_text(&recv(r)?, &mut s);
    Ok(Value::Text(s))
}

fn inner_xml(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let mut out = String::new();
    for c in children_of(&h) {
        write_node(&c, &mut out, &mut Vec::new())?;
    }
    Ok(Value::Text(out))
}

fn child_elements(h: &Handle, name: Option<(&str, Option<&str>)>) -> Vec<Handle> {
    children_of(h)
        .into_iter()
        .filter(|c| {
            with(c, |d| match (d, name) {
                (XData::Element { .. }, None) => true,
                (XData::Element { local, ns, .. }, Some((n, uri))) => local == n && ns == uri.unwrap_or(""),
                _ => false,
            })
        })
        .collect()
}

fn get_child_elements(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(match a.len() {
        0 => list_of(child_elements(&h, None)),
        1 => {
            let n = arg_text(a, 0)?;
            check_name(&n)?;
            list_of(child_elements(&h, Some((&n, None))))
        }
        _ => {
            let (n, u) = (arg_text(a, 0)?, arg_text(a, 1)?);
            list_of(child_elements(&h, Some((&n, Some(&u)))))
        }
    })
}

fn get_child_nodes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(list_of(children_of(&recv(r)?)))
}

fn all_descendants(h: &Handle, out: &mut Vec<Handle>) {
    for c in children_of(h) {
        out.push(c.clone());
        all_descendants(&c, out);
    }
}

fn get_descendant_nodes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let mut v = Vec::new();
    all_descendants(&recv(r)?, &mut v);
    Ok(list_of(v))
}

fn get_descendant_elements(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let mut v = Vec::new();
    all_descendants(&h, &mut v);
    let want: Option<(String, String)> = match a.len() {
        0 => None,
        1 => {
            let n = arg_text(a, 0)?;
            check_name(&n)?;
            Some((n, String::new()))
        }
        _ => Some((arg_text(a, 0)?, arg_text(a, 1)?)),
    };
    v.retain(|c| {
        with(c, |d| match (d, &want) {
            (XData::Element { .. }, None) => true,
            (XData::Element { local, ns, .. }, Some((n, u))) => local == n && ns == u,
            _ => false,
        })
    });
    Ok(list_of(v))
}

fn select_single_node(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let found = xpath(&h, &arg_text(a, 0)?)?;
    match found.into_iter().next() {
        Some(n) => {
            a[1] = tagged("XmlNode", n);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("SelectSingleNode without a match, result unused (error text not pinned yet)"),
    }
}

fn select_nodes(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    a[1] = list_of(xpath(&h, &arg_text(a, 0)?)?);
    Ok(Value::Boolean(true))
}

fn write_to_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    a[0] = Value::Text(serialize(&h)?);
    Ok(Value::Boolean(true))
}

fn name_table(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    recv(r)?;
    Ok(Value::Library(
        "XmlNameTable".into(),
        Rc::new(RefCell::new(LibraryValue::XmlNameTable)),
    ))
}

// ----- attributes

fn set_attribute(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let (name, value) = (arg_text(a, 0)?, arg_text(a, 1)?);
    check_name(&name)?;
    let existing = attrs_of(&h).into_iter().find(|x| {
        with(
            x,
            |d| matches!(d, XData::Attribute { local, ns, .. } if *local == name && ns.is_empty()),
        )
    });
    match existing {
        Some(x) => with(&x, |d| {
            if let XData::Attribute { value: v, .. } = d {
                *v = value;
            }
        }),
        None => {
            let at = new_node(XData::Attribute {
                prefix: String::new(),
                local: name,
                ns: String::new(),
                value,
            });
            insert(&h, None, vec![at])?;
        }
    }
    Ok(Value::Null)
}

fn remove_attribute(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let target = match a.first() {
        Some(Value::Library(..)) => node_of(&a[0])?,
        _ => {
            let name = arg_text(a, 0)?;
            let found = attrs_of(&h).into_iter().find(|x| {
                with(
                    x,
                    |d| matches!(d, XData::Attribute { local, ns, .. } if *local == name && ns.is_empty()),
                )
            });
            match found {
                Some(x) => x,
                None => return unsupported("RemoveAttribute of a missing attribute (behaviour not pinned yet)"),
            }
        }
    };
    detach(&target);
    Ok(Value::Null)
}

fn remove_all_attributes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    for a in attrs_of(&h) {
        detach(&a);
    }
    Ok(Value::Null)
}

fn attributes(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Library(
        "XmlAttributeCollection".into(),
        Rc::new(RefCell::new(LibraryValue::XmlAttrs(h))),
    ))
}

fn owner_of(r: Option<&mut Value>) -> Result<Handle, Signal> {
    match r {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::XmlAttrs(o) => Ok(o.clone()),
            _ => unsupported("XmlAttributeCollection method on something else"),
        },
        _ => unsupported("XmlAttributeCollection method without a receiver"),
    }
}

fn attrs_count(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Integer(attrs_of(&owner_of(r)?).len() as i32))
}

fn attrs_get_index(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let owner = owner_of(r)?;
    let Value::Integer(i) = a[0] else {
        return unsupported("attribute index that is not an Integer");
    };
    match usize::try_from(i - 1)
        .ok()
        .and_then(|i| attrs_of(&owner).get(i).cloned())
    {
        Some(at) => {
            a[1] = tagged("XmlAttribute", at);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("XmlAttributeCollection.Get out of range, result unused (error text not pinned yet)"),
    }
}

fn attrs_get_name(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let owner = owner_of(r)?;
    let name = arg_text(a, 0)?;
    let found = attrs_of(&owner).into_iter().find(|x| {
        with(
            x,
            |d| matches!(d, XData::Attribute { prefix, local, .. } if qname(prefix, local) == name),
        )
    });
    match found {
        Some(at) => {
            a[1] = tagged("XmlAttribute", at);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("XmlAttributeCollection.Get of a missing name, result unused (error text not pinned yet)"),
    }
}

// ----- node lists

fn list_items(r: Option<&mut Value>) -> Result<Vec<Handle>, Signal> {
    match r {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::XmlList(v) => Ok(v.clone()),
            LibraryValue::Empty => Ok(Vec::new()),
            _ => unsupported("XmlNodeList method on something else"),
        },
        _ => unsupported("XmlNodeList method without a receiver"),
    }
}

fn list_count(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Integer(list_items(r)?.len() as i32))
}

fn list_get(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let items = list_items(r)?;
    let Value::Integer(i) = a[0] else {
        return unsupported("node list index that is not an Integer");
    };
    match usize::try_from(i - 1).ok().and_then(|i| items.get(i).cloned()) {
        Some(n) => {
            a[1] = tagged("XmlNode", n);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("XmlNodeList.Get out of range, result unused (error text not pinned yet)"),
    }
}

// ---------------------------------------------------------- Variant.Is*

macro_rules! variant_is {
    ($($fname:ident => $tag:literal),* $(,)?) => {
        $(fn $fname(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
            Ok(Value::Boolean(r.is_some_and(|v| v.is($tag))))
        })*

        fn variant_is_fn(method: &str) -> Option<NativeFn> {
            Some(match method {
                $(concat!("Is", $tag) => $fname,)*
                _ => return None,
            })
        }
    };
}
variant_is! {
    v_is_xml_attribute => "XmlAttribute",
    v_is_xml_attribute_collection => "XmlAttributeCollection",
    v_is_xml_cdata => "XmlCData",
    v_is_xml_comment => "XmlComment",
    v_is_xml_declaration => "XmlDeclaration",
    v_is_xml_document => "XmlDocument",
    v_is_xml_document_type => "XmlDocumentType",
    v_is_xml_element => "XmlElement",
    v_is_xml_name_table => "XmlNameTable",
    v_is_xml_namespace_manager => "XmlNamespaceManager",
    v_is_xml_node => "XmlNode",
    v_is_xml_node_list => "XmlNodeList",
    v_is_xml_pi => "XmlProcessingInstruction",
    v_is_xml_read_options => "XmlReadOptions",
    v_is_xml_text => "XmlText",
    v_is_xml_write_options => "XmlWriteOptions",
    v_is_json_array => "JsonArray",
    v_is_json_object => "JsonObject",
    v_is_json_token => "JsonToken",
    v_is_json_value => "JsonValue",
}

// --------------------------------------------------------------- dispatch

/// The implementation of one catalog overload, if the corpus pins enough of it.
/// Overloads with streams, read/write options or namespace managers are not implemented.
pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let params: Vec<&str> = sig.params.iter().map(|p| p.ty.as_str()).collect();
    let first_by_var = sig.params.first().is_some_and(|p| p.by_var);
    if sig.receiver == "Variant" && sig.instance {
        return variant_is_fn(&sig.name).filter(|_| params.is_empty());
    }
    let r = sig.receiver.as_str();
    // Every node type shares these.
    let node_types = [
        "XmlDocument",
        "XmlElement",
        "XmlAttribute",
        "XmlText",
        "XmlCData",
        "XmlComment",
        "XmlProcessingInstruction",
        "XmlDeclaration",
        "XmlNode",
    ];
    let is_node = node_types.contains(&r);
    let f: NativeFn = match (r, sig.name.as_str(), params.as_slice()) {
        ("XmlDocument", "ReadFrom", ["Text", "XmlDocument"]) => read_from_text,
        ("XmlDocument", "Create", _) if !sig.instance => doc_create,
        ("XmlElement", "Create", ["Text"] | ["Text", "Any"]) => element_create_name,
        ("XmlElement", "Create", ["Text", "Text"] | ["Text", "Text", "Any"]) => element_create_ns,
        ("XmlAttribute", "Create", ["Text", "Text"]) => attribute_create,
        ("XmlAttribute", "Create", ["Text", "Text", "Text"]) => attribute_create_ns,
        ("XmlAttribute", "CreateNamespaceDeclaration", [_, _]) => attribute_create_ns_decl,
        ("XmlText", "Create", [_]) => text_create,
        ("XmlCData", "Create", [_]) => cdata_create,
        ("XmlComment", "Create", [_]) => comment_create,
        ("XmlProcessingInstruction", "Create", [_, _]) => pi_create,
        ("XmlDeclaration", "Create", [_, _, _]) => decl_create,
        ("XmlDeclaration", "Version", _) => decl_version,
        ("XmlDeclaration", "Encoding", _) => decl_encoding,
        ("XmlDeclaration", "Standalone", _) => decl_standalone,
        ("XmlText" | "XmlCData" | "XmlComment" | "XmlAttribute", "Value", _) => value_prop,
        ("XmlProcessingInstruction", "GetTarget", [_]) => pi_get_target,
        ("XmlProcessingInstruction", "GetData", [_]) => pi_get_data,
        ("XmlElement" | "XmlAttribute", "Name", []) => element_name,
        ("XmlElement" | "XmlAttribute", "LocalName", []) => local_name,
        ("XmlElement" | "XmlAttribute", "NamespaceUri", []) => namespace_uri,
        ("XmlAttribute", "NamespacePrefix", []) => namespace_prefix,
        ("XmlAttribute", "IsNamespaceDeclaration", []) => is_namespace_declaration,
        ("XmlElement", "GetPrefixOfNamespace", [_, _]) => get_prefix_of_namespace,
        ("XmlElement", "GetNamespaceOfPrefix", [_, _]) => get_namespace_of_prefix,
        ("XmlElement" | "XmlDocument", "Add", _) => add,
        ("XmlElement" | "XmlDocument", "AddFirst", _) => add_first,
        (_, "AddAfterSelf", _) if is_node => add_after_self,
        (_, "AddBeforeSelf", _) if is_node => add_before_self,
        (_, "Remove", []) if is_node => remove,
        (_, "ReplaceWith", _) if is_node => replace_with,
        ("XmlElement" | "XmlDocument", "RemoveNodes", []) => remove_nodes,
        ("XmlElement" | "XmlDocument", "ReplaceNodes", _) => replace_nodes,
        ("XmlDocument", "GetRoot", [_]) => get_root,
        ("XmlDocument", "GetDeclaration", [_]) => get_declaration,
        ("XmlDocument", "SetDeclaration", [_]) => set_declaration,
        ("XmlDocument", "GetDocumentType", [_]) => get_document_type,
        ("XmlDocument", "NameTable", []) => name_table,
        (_, "GetDocument", [_]) if is_node => get_document,
        (_, "GetParent", [_]) if is_node => get_parent,
        (_, "AsXmlNode", []) if is_node => as_xml_node,
        ("XmlNode", "AsXmlAttribute", []) => as_attribute,
        ("XmlNode", "AsXmlCData", []) => as_cdata,
        ("XmlNode", "AsXmlComment", []) => as_comment,
        ("XmlNode", "AsXmlDeclaration", []) => as_declaration,
        ("XmlNode", "AsXmlDocument", []) => as_document,
        ("XmlNode", "AsXmlElement", []) => as_element,
        ("XmlNode", "AsXmlProcessingInstruction", []) => as_pi,
        ("XmlNode", "AsXmlText", []) => as_text,
        ("XmlNode", "IsXmlAttribute", []) => is_attribute,
        ("XmlNode", "IsXmlCData", []) => is_cdata,
        ("XmlNode", "IsXmlComment", []) => is_comment,
        ("XmlNode", "IsXmlDeclaration", []) => is_declaration,
        ("XmlNode", "IsXmlDocument", []) => is_document,
        ("XmlNode", "IsXmlElement", []) => is_element,
        ("XmlNode", "IsXmlProcessingInstruction", []) => is_pi,
        ("XmlNode", "IsXmlText", []) => is_text,
        ("XmlElement", "HasAttributes", []) => has_attributes,
        ("XmlElement", "HasElements", []) => has_elements,
        ("XmlElement", "IsEmpty", []) => is_empty,
        ("XmlElement", "InnerText", []) => inner_text_fn,
        ("XmlElement", "InnerXml", []) => inner_xml,
        ("XmlElement" | "XmlDocument", "GetChildElements", [] | ["Text"] | ["Text", "Text"]) => get_child_elements,
        ("XmlElement" | "XmlDocument", "GetChildNodes", []) => get_child_nodes,
        ("XmlElement" | "XmlDocument", "GetDescendantElements", [] | ["Text"] | ["Text", "Text"]) => {
            get_descendant_elements
        }
        ("XmlElement" | "XmlDocument", "GetDescendantNodes", []) => get_descendant_nodes,
        (_, "SelectSingleNode", ["Text", "XmlNode"]) if is_node => select_single_node,
        (_, "SelectNodes", ["Text", "XmlNodeList"]) if is_node => select_nodes,
        (_, "WriteTo", ["Text"]) if is_node && first_by_var => write_to_text,
        ("XmlElement", "SetAttribute", ["Text", "Text"]) => set_attribute,
        ("XmlElement", "RemoveAttribute", ["Text"] | ["XmlAttribute"]) => remove_attribute,
        ("XmlElement", "RemoveAllAttributes", []) => remove_all_attributes,
        ("XmlElement", "Attributes", []) => attributes,
        ("XmlAttributeCollection", "Count", []) => attrs_count,
        ("XmlAttributeCollection", "Get", ["Integer", "XmlAttribute"]) => attrs_get_index,
        ("XmlAttributeCollection", "Get", ["Text", "XmlAttribute"]) => attrs_get_name,
        ("XmlNodeList", "Count", []) => list_count,
        ("XmlNodeList", "Get", ["Integer", "XmlNode"]) => list_get,
        _ => return None,
    };
    Some(f)
}
