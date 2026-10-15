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

use crate::describe::Describe;
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
        /// Parsed from `<a></a>`: written with an end tag although empty (xml/TestProbeXmlContracts.al:
        /// WriteTo_EmptyElementFromParsedText); `<a/>` and a created element are written `<a />`.
        open: bool,
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
        /// Made by `XmlDeclaration.Create`: the encoding and standalone attributes are written even when empty
        /// (xml/TestProbeXmlContracts.al:WriteTo_DeclarationAndIndent).
        explicit: bool,
    },
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// `Exception of type 'Microsoft.Dynamics.Nav.Types.Exceptions.NavNCLArgumentException' was thrown.` (code `Argument`).
fn nav_argument() -> Signal {
    error(
        "Exception of type 'Microsoft.Dynamics.Nav.Types.Exceptions.NavNCLArgumentException' was thrown.",
        "Argument",
    )
}

fn error(message: impl Into<String>, code: &str) -> Signal {
    Signal::Error(crate::native::AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
    })
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
                // An XmlDocument variable that was never assigned is an empty document
                // (xml/TestXmlRuntimeContracts.al:XmlDocument_Add_SecondRoot_Throws adds to one).
                if name == "XmlDocument" {
                    *h.borrow_mut() = LibraryValue::Xml(XmlCell {
                        data: XData::Document {
                            decl: None,
                            children: Vec::new(),
                        },
                        parent: None,
                    });
                    Ok(h.clone())
                } else {
                    unsupported(format!("use of an unassigned {name}"))
                }
            } else {
                unsupported(format!("{name} used as an XML node"))
            }
        }
        other => unsupported(format!("{} used as an XML node", other.describe())),
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
            open,
        } => XData::Element {
            prefix: prefix.clone(),
            local: local.clone(),
            ns: ns.clone(),
            attrs: attrs.iter().map(deep_clone).collect(),
            children: children.iter().map(deep_clone).collect(),
            open: *open,
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
            explicit,
        } => XData::Declaration {
            version: version.clone(),
            encoding: encoding.clone(),
            standalone: standalone.clone(),
            explicit: *explicit,
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
    /// Not well-formed, with the text and position of the .NET exception BC reports
    /// (xml/TestXmlRuntimeContracts.al:XmlDocument_ReadFrom_Invalid_ReturnsFalse_AndStatementFormThrows).
    Failed {
        message: String,
        line: usize,
        pos: usize,
        /// Whether `XmlDocument.ReadFrom` is known to word its error with this message; an XmlPort
        /// import words its errors with the .NET messages in every case.
        dom: bool,
    },
    Unsupported(&'static str),
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    /// The names of the open elements, outermost first.
    open: Vec<String>,
    /// In-scope namespace declarations, innermost last.
    scope: Vec<(String, String)>,
    depth: usize,
    /// Parsing for `XmlDocument.ReadFrom`, whose errors are worded differently from an XmlPort import's.
    dom: bool,
    /// Where the root element starts.
    root_at: usize,
}

type PResult<T> = Result<T, ParseError>;

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.i..]
    }
    /// A well-formedness error of `XmlDocument.ReadFrom`, worded as .NET words it, at byte offset `at`
    /// (xml/TestProbeXmlContracts.al:ReadFrom_Invalid_Statement).
    fn fail_at<T>(&self, what: &str, at: usize) -> PResult<T> {
        let (line, pos) = self.line_pos(at);
        Err(ParseError::Failed {
            message: format!("{what} Line {line}, position {pos}."),
            line,
            pos,
            dom: true,
        })
    }
    /// Line and position (both 1-based, in UTF-16 units) of byte offset `at`.
    fn line_pos(&self, at: usize) -> (usize, usize) {
        let before = &self.s[..at];
        let line = before.matches('\n').count() + 1;
        let start = before.rfind('\n').map_or(0, |n| n + 1);
        let pos = before[start..].encode_utf16().count() + 1;
        (line, pos)
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
    /// The text of `raw` with its references replaced; `base` is the byte offset of `raw` in the document, for the
    /// position of an undeclared entity.
    fn decode(&self, raw: &str, attr: bool, base: usize) -> PResult<String> {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        let mut off = 0usize;
        while let Some(c) = chars.next() {
            let here = off;
            off += c.len_utf8();
            match c {
                '&' => {
                    let mut ent = String::new();
                    loop {
                        match chars.next() {
                            Some(';') => {
                                off += 1;
                                break;
                            }
                            Some(x) if ent.len() < 12 => {
                                off += x.len_utf8();
                                ent.push(x);
                            }
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
                        e => {
                            return self.fail_at(&format!("Reference to undeclared entity '{e}'."), base + here + 1);
                        }
                    }
                }
                '<' if attr => return Err(ParseError::Invalid),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                        off += 1;
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
            return self.fail_at(
                &format!("'{q}' is an unexpected token. The expected token is '\"' or '''."),
                self.i,
            );
        }
        self.bump(1);
        let end = self.rest().find(q).ok_or(ParseError::Invalid)?;
        let raw = self.s[self.i..self.i + end].to_string();
        let base = self.i;
        self.bump(end + 1);
        self.decode(&raw, true, base)
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
        // Only version 1.0 reads (xml/TestProbeXmlContracts.al:ReadFrom_Invalid_ResultUsed: 9.9 is refused).
        if version != "1.0" {
            return Err(ParseError::Invalid);
        }
        Ok(new_node(XData::Declaration {
            version,
            encoding,
            standalone,
            explicit: false,
        }))
    }

    fn comment(&mut self) -> PResult<Handle> {
        self.expect("<!--")?;
        let Some(end) = self.rest().find("-->") else {
            return self.fail_at(
                "Unexpected end of file while parsing Comment has occurred.",
                self.s.len(),
            );
        };
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
        let name_at = self.i;
        let qn = self.name()?;
        self.open.push(qn.clone());
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
            let an_at = self.i;
            let an = self.name()?;
            self.skip_ws();
            self.expect("=")?;
            self.skip_ws();
            let av = self.quoted()?;
            if raw_attrs.iter().any(|(n, _)| *n == an) {
                return self.fail_at(&format!("'{an}' is a duplicate attribute name."), an_at);
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
                    let close_at = self.i;
                    let close = self.name()?;
                    self.skip_ws();
                    self.expect(">")?;
                    if close != qn {
                        let (sl, sp) = self.line_pos(name_at);
                        let (line, pos) = self.line_pos(close_at);
                        return Err(ParseError::Failed {
                            message: format!(
                                "The '{qn}' start tag on line {sl} position {sp} does not match the end tag of '{close}'. Line {line}, position {pos}."
                            ),
                            line,
                            pos,
                            dom: true,
                        });
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
                } else if self.peek().is_none() && self.dom {
                    // xml/TestProbeXmlContracts.al:ReadFrom_Invalid_Statement (`<a>`): worded as an error at the root.
                    return self.fail_at("Data at the root level is invalid.", self.root_at);
                } else if self.peek().is_none() {
                    // Pinned for an XmlPort import (xmlport/TestXmlPortExportImportContracts.al:
                    // XmlPortImport_MalformedXml_Throws).
                    let (line, pos) = self.line_pos(self.s.len());
                    let names: Vec<&str> = self.open.iter().rev().map(String::as_str).collect();
                    return Err(ParseError::Failed {
                        message: format!(
                            "Unexpected end of file has occurred. The following elements are not closed: {}. Line {line}, position {pos}.",
                            names.join(", ")
                        ),
                        line,
                        pos,
                        dom: false,
                    });
                } else {
                    let end = self.rest().find('<').unwrap_or(self.rest().len());
                    let raw = self.s[self.i..self.i + end].to_string();
                    self.bump(end);
                    if raw.contains("]]>") {
                        return Err(ParseError::Invalid);
                    }
                    // White space is kept, inside and outside the root
                    // (xml/TestXmlRuntimeContracts.al:XmlDocument_ReadFrom_KeepsWhiteSpaceInsideAndOutsideTheRoot).
                    let text = self.decode(&raw, false, self.i - end)?;
                    children.push(new_node(XData::Text(text)));
                }
            }
        }
        self.scope.truncate(scope_mark);
        self.open.pop();
        self.depth -= 1;
        let open = !self_closed && children.is_empty();
        Ok(new_node(XData::Element {
            prefix,
            local,
            ns,
            attrs,
            children,
            open,
        }))
    }

    /// `<!DOCTYPE name ... [ internal subset ] >`.
    fn skip_doctype(&mut self) -> PResult<()> {
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let rest = self.rest().to_string();
        for (n, c) in rest.char_indices() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '[') => depth += 1,
                (None, ']') => depth -= 1,
                (None, '>') if depth <= 0 => {
                    self.bump(n + 1);
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(ParseError::Invalid)
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
            let ws_start = self.i;
            self.skip_ws();
            if self.i > ws_start {
                let ws = self.s[ws_start..self.i].replace("\r\n", "\n").replace('\r', "\n");
                children.push(new_node(XData::Text(ws)));
            }
            if self.peek().is_none() {
                break;
            }
            if self.starts("<!--") {
                children.push(self.comment()?);
            } else if self.starts("<?") {
                children.push(self.pi()?);
            } else if self.starts("<!DOCTYPE") {
                // A document type is read and dropped: GetDocumentType answers false and WriteTo writes none
                // (xml/TestProbeXmlContracts.al:DocumentType_ReadWithDoctype).
                self.skip_doctype()?;
            } else if self.starts("<") && !root_seen {
                if !self.rest()[1..].chars().next().is_some_and(is_name_start) {
                    return self.fail_at("Data at the root level is invalid.", self.i);
                }
                self.root_at = self.i;
                children.push(self.element()?);
                root_seen = true;
            } else if !self.starts("<") {
                return self.fail_at("Data at the root level is invalid.", self.i);
            } else {
                return self.fail_at("There are multiple root elements.", self.i + 1);
            }
        }
        if !root_seen {
            return Err(ParseError::Failed {
                message: "Root element is missing.".into(),
                line: 0,
                pos: 0,
                dom: true,
            });
        }
        Ok(new_node(XData::Document { decl, children }))
    }
}

/// A .NET XML exception: its message and the position it names.
struct Failure {
    message: String,
    line: usize,
    pos: usize,
    dom: bool,
}

/// `Ok(Err(detail))`: not well-formed XML; `detail` is BC's exception text with its position
/// where the runtime knows it.
fn parse_document_detail(text: &str, dom: bool) -> Result<Result<Handle, Option<Failure>>, Signal> {
    let mut p = Parser {
        s: text,
        i: 0,
        open: Vec::new(),
        scope: Vec::new(),
        depth: 0,
        dom,
        root_at: 0,
    };
    match p.document() {
        Ok(d) => Ok(Ok(d)),
        Err(ParseError::Invalid) => Ok(Err(None)),
        Err(ParseError::Failed {
            message,
            line,
            pos,
            dom,
        }) => Ok(Err(Some(Failure {
            message,
            line,
            pos,
            dom,
        }))),
        Err(ParseError::Unsupported(w)) => unsupported(w),
    }
}

// ---------------------------------------------------------- serialising

pub(crate) fn escape_text(s: &str, out: &mut String) {
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

pub(crate) fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
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

/// The state of .NET's indenting text writer, which BC writes through: two spaces per level, LF,
/// and no indentation inside an element that has text (mixed content). The rules, each pinned by
/// xml/TestXmlRuntimeContracts.al:XmlDocument_WriteTo_HasDeclarationAndTwoSpaceIndentation,
/// :XmlElement_WriteTo_MixedContentIsNotIndented and
/// :XmlDocument_ReadFrom_KeepsWhiteSpaceInsideAndOutsideTheRoot.
struct Writer {
    out: String,
    indent: bool,
    level: usize,
    /// The current element has had text, CDATA or white space written into it.
    mixed: bool,
    /// `mixed` of the enclosing elements.
    saved: Vec<bool>,
    /// Where the last text ended; a markup node starts on a new line when more was written since.
    text_end: usize,
    scope: Scope,
}

impl Writer {
    fn new(indent: bool) -> Writer {
        Writer {
            out: String::new(),
            indent,
            level: 0,
            mixed: false,
            saved: Vec::new(),
            text_end: 0,
            scope: Vec::new(),
        }
    }

    fn break_line(&mut self) {
        if self.indent && !self.mixed && self.out.len() != self.text_end {
            self.out.push('\n');
            for _ in 0..self.level {
                self.out.push_str("  ");
            }
        }
    }

    fn text(&mut self, s: &str) {
        self.mixed = true;
        self.out.push_str(s);
        self.text_end = self.out.len();
    }

    fn markup(&mut self, s: &str) {
        self.break_line();
        self.out.push_str(s);
    }

    fn node(&mut self, h: &Handle) -> Result<(), Signal> {
        enum Plan {
            Element {
                prefix: String,
                local: String,
                ns: String,
                attrs: Vec<Handle>,
                children: Vec<Handle>,
                open: bool,
            },
            Text(String),
            CData(String),
            Markup(String),
            Skip,
        }
        let plan = with(h, |d| match d {
            XData::Element {
                prefix,
                local,
                ns,
                attrs,
                children,
                open,
            } => Plan::Element {
                prefix: prefix.clone(),
                local: local.clone(),
                ns: ns.clone(),
                attrs: attrs.clone(),
                children: children.clone(),
                open: *open,
            },
            XData::Text(t) => {
                let mut s = String::new();
                escape_text(t, &mut s);
                Plan::Text(s)
            }
            XData::CData(t) => Plan::CData(format!("<![CDATA[{t}]]>")),
            XData::Comment(t) => Plan::Markup(format!("<!--{t}-->")),
            XData::Pi { target, data } => Plan::Markup(if data.is_empty() {
                format!("<?{target}?>")
            } else {
                format!("<?{target} {data}?>")
            }),
            XData::Attribute { .. } | XData::Declaration { .. } | XData::Document { .. } => Plan::Skip,
        });
        match plan {
            Plan::Text(s) | Plan::CData(s) => self.text(&s),
            Plan::Markup(s) => self.markup(&s),
            Plan::Skip => return unsupported(format!("serialising {} as markup", kind(h))),
            Plan::Element {
                prefix,
                local,
                ns,
                attrs,
                children,
                open,
            } => self.element(&prefix, &local, &ns, &attrs, &children, open)?,
        }
        Ok(())
    }

    fn element(
        &mut self,
        prefix: &str,
        local: &str,
        ns: &str,
        attrs: &[Handle],
        children: &[Handle],
        open: bool,
    ) -> Result<(), Signal> {
        let mark = self.scope.len();
        // Declarations written on this element: explicit ones first, in their order.
        let mut written: Vec<String> = Vec::new();
        let mut plain: Vec<(String, String, String, String)> = Vec::new(); // prefix, local, ns, value
        for a in attrs {
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
                self.scope.push((declared.clone(), av.clone()));
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
        let visible = mapped(&self.scope, prefix);
        if visible.as_deref().unwrap_or("") != ns {
            let mut s = String::new();
            escape_attr(ns, &mut s);
            extra = if prefix.is_empty() {
                format!(" xmlns=\"{s}\"")
            } else {
                format!(" xmlns:{prefix}=\"{s}\"")
            };
            self.scope.push((prefix.to_string(), ns.to_string()));
        }
        self.break_line();
        self.level += 1;
        self.saved.push(self.mixed);
        self.mixed = false;
        self.out.push('<');
        self.out.push_str(&qname(prefix, local));
        self.out.push_str(&extra);
        for w in &written {
            self.out.push_str(w);
        }
        for (ap, al, ans, av) in &plain {
            let use_prefix = if ans.is_empty() {
                String::new()
            } else if !ap.is_empty() && mapped(&self.scope, ap).as_deref() == Some(ans.as_str()) {
                ap.clone()
            } else if let Some((p, _)) = self.scope.iter().rev().find(|(p, u)| !p.is_empty() && u == ans) {
                p.clone()
            } else {
                return unsupported("serialising a namespaced attribute without a declared prefix");
            };
            let mut s = String::new();
            escape_attr(av, &mut s);
            self.out.push_str(&format!(" {}=\"{s}\"", qname(&use_prefix, al)));
        }
        if children.is_empty() && !open {
            self.out.push_str(" />");
            self.level -= 1;
        } else {
            self.out.push('>');
            let content_start = self.out.len();
            for c in children {
                self.node(c)?;
            }
            self.level -= 1;
            if self.out.len() != content_start {
                // The end tag goes on its own line, unless the element has text.
                self.break_line();
            }
            self.out.push_str("</");
            self.out.push_str(&qname(prefix, local));
            self.out.push('>');
        }
        self.mixed = self.saved.pop().unwrap_or(false);
        self.scope.truncate(mark);
        Ok(())
    }
}

/// The text of `h` as BC writes it. A document gets its XML declaration first (its own, else
/// `version="1.0" encoding="utf-16"`). The namespace declarations of `h`'s ancestors do not
/// matter when a node is written on its own: it declares what it needs.
fn serialize(h: &Handle) -> Result<String, Signal> {
    serialize_with(h, true)
}

/// [`serialize`] with or without the indentation (`XmlWriteOptions.PreserveWhitespace` writes none).
fn serialize_with(h: &Handle, indent: bool) -> Result<String, Signal> {
    let mut w = Writer::new(indent);
    let is_doc = with(h, |d| matches!(d, XData::Document { .. }));
    if is_doc {
        let decl = with(h, |d| match d {
            XData::Document { decl, .. } => decl.clone(),
            _ => None,
        });
        let text = match decl {
            Some(d) => with(&d, |x| match x {
                XData::Declaration {
                    version,
                    encoding,
                    standalone,
                    explicit,
                } => {
                    let mut t = format!("<?xml version=\"{version}\"");
                    if *explicit || !encoding.is_empty() {
                        t.push_str(&format!(" encoding=\"{encoding}\""));
                    }
                    if *explicit || !standalone.is_empty() {
                        t.push_str(&format!(" standalone=\"{standalone}\""));
                    }
                    t.push_str("?>");
                    t
                }
                _ => unreachable!("a declaration slot holds a declaration"),
            }),
            None => "<?xml version=\"1.0\" encoding=\"utf-16\"?>".to_string(),
        };
        w.markup(&text);
        for c in children_of(h) {
            w.node(&c)?;
        }
    } else {
        w.node(h)?;
    }
    Ok(w.out)
}

/// The content of `h` without indentation (`InnerXml`).
fn serialize_inner(h: &Handle) -> Result<String, Signal> {
    let mut w = Writer::new(false);
    for c in children_of(h) {
        w.node(&c)?;
    }
    Ok(w.out)
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
            // Text is accepted as content and becomes a text node
            // (xml/TestXmlRuntimeContracts.al:XmlElement_Add_Text_IsAccepted_AndAdjacentTextsMerge).
            Value::Text(t) | Value::Code(t) => {
                out.push(new_node(XData::Text(t.clone())));
                continue;
            }
            other => {
                return unsupported(format!(
                    "{} as XML content (only XML nodes and text are pinned)",
                    other.describe()
                ));
            }
        };
        if with(&h, |d| matches!(d, XData::Document { .. })) {
            // xml/TestProbeXmlContracts.al:Add_NodeIntoWrongContainer_Error, `document_into_element_via_add_node`.
            return Err(nav_argument());
        }
        out.push(if parent_of(&h).is_some() { deep_clone(&h) } else { h });
    }
    Ok(out)
}

/// Whether `node` may be added to `container`. `removed` are children that go first (a replace).
fn check_fits(container: &Handle, node: &Handle, removed: &[Handle]) -> Result<(), Signal> {
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
                .filter(|c| !removed.iter().any(|r| Rc::ptr_eq(r, c)))
                .any(|c| with(c, |d| matches!(d, XData::Element { .. })));
            if has_root {
                // xml/TestXmlRuntimeContracts.al:XmlDocument_Add_SecondRoot_Throws
                return Err(error(
                    "The requested operation cannot be performed in this context.",
                    "InvalidOperation",
                ));
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
        match (c_kind, n_kind) {
            // xml/TestProbeXmlContracts.al:Add_NodeIntoWrongContainer_Error: a text or an attribute into a document.
            ("XmlDocument", "XmlText" | "XmlCData" | "XmlAttribute") => Err(nav_argument()),
            _ => unsupported(format!("adding {n_kind} to {c_kind} (error text not pinned yet)")),
        }
    }
}

/// Inserts `nodes` into `container` at child position `at` (`None` appends).
fn insert(container: &Handle, at: Option<usize>, nodes: Vec<Handle>) -> Result<(), Signal> {
    for n in &nodes {
        check_fits(container, n, &[])?;
    }
    let mut at = at;
    for n in nodes {
        match kind(&n) {
            "XmlAttribute" => {
                let dup = attrs_of(container).iter().any(|a| same_attr_name(a, &n));
                if dup {
                    // xml/TestProbeXmlContracts.al:Add_AttributeTwice_Error (the same name, and the same instance)
                    return Err(error(
                        "The requested operation cannot be performed in this context.",
                        "InvalidOperation",
                    ));
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

// ------------------------------------------------------------- XPath 1.0 (the part the corpus pins)

/// Why an XPath did not produce a node set.
enum XFail {
    /// `Expression must evaluate to a node-set.` (xml/TestXmlRuntimeContracts.al:XmlDocument_SelectNodes_NotANodeSet_Throws).
    NotNodeSet,
    /// Not valid XPath, or a prefix nobody declared: a call whose result is used answers false
    /// (xml/TestProbeXmlContracts.al:XPath_MalformedStatement, XPath_NamespaceManager); the statement form is refused.
    Malformed(String),
    Signal(Signal),
}

impl From<Signal> for XFail {
    fn from(s: Signal) -> Self {
        XFail::Signal(s)
    }
}

type XRes<T> = Result<T, XFail>;

fn malformed<T>(why: impl Into<String>) -> XRes<T> {
    Err(XFail::Malformed(why.into()))
}

/// Prefix to namespace declarations of an `XmlNamespaceManager`.
type Prefixes<'a> = Option<&'a [(String, String)]>;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Current,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Attribute,
}

#[derive(Debug, Clone)]
enum Test {
    /// `prefix:local` or `local`. An unprefixed name matches only nodes in no namespace
    /// (XPath 1.0; xml/TestXmlRuntimeContracts.al:XmlDocument_SelectNodes_DefaultNamespace_NeedsAPrefixedName).
    Name(Option<String>, String),
    Any,
    Text,
    Node,
    Comment,
    Pi,
}

#[derive(Debug, Clone)]
struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<Expr>,
}

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Str(String),
    /// An absolute (`true`) or relative location path.
    Path(bool, Vec<Step>),
    /// `a | b`
    Union(Vec<Expr>),
    Call(String, Vec<Expr>),
    Cmp(Box<Expr>, String, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Arith(Box<Expr>, char, Box<Expr>),
    /// A filter on a function result or a parenthesised expression followed by steps.
    Filter(Box<Expr>, Vec<Step>),
}

struct XParser<'a> {
    s: Vec<char>,
    i: usize,
    prefixes: Prefixes<'a>,
}

impl XParser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn starts(&self, t: &str) -> bool {
        t.chars().enumerate().all(|(k, c)| self.s.get(self.i + k) == Some(&c))
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.starts(t) {
            self.i += t.chars().count();
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Option<String> {
        let start = self.i;
        if !self.peek().is_some_and(is_name_start) {
            return None;
        }
        while self.peek().is_some_and(is_name_char) {
            self.i += 1;
        }
        Some(self.s[start..self.i].iter().collect())
    }

    fn expr(&mut self) -> XRes<Expr> {
        self.or()
    }
    fn or(&mut self) -> XRes<Expr> {
        let mut l = self.and()?;
        while self.word("or") {
            let r = self.and()?;
            l = Expr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> XRes<Expr> {
        let mut l = self.cmp()?;
        while self.word("and") {
            let r = self.cmp()?;
            l = Expr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn word(&mut self, w: &str) -> bool {
        self.ws();
        let save = self.i;
        if self.starts(w) && !self.s.get(self.i + w.len()).copied().is_some_and(is_name_char) {
            self.i += w.len();
            true
        } else {
            self.i = save;
            false
        }
    }
    fn cmp(&mut self) -> XRes<Expr> {
        let mut l = self.additive()?;
        loop {
            self.ws();
            let op = ["!=", "<=", ">=", "=", "<", ">"].into_iter().find(|o| self.starts(o));
            let Some(op) = op else { return Ok(l) };
            self.i += op.len();
            let r = self.additive()?;
            l = Expr::Cmp(Box::new(l), op.to_string(), Box::new(r));
        }
    }
    fn additive(&mut self) -> XRes<Expr> {
        let mut l = self.multiplicative()?;
        loop {
            self.ws();
            let op = match self.peek() {
                Some('+') => '+',
                // `-` is an operator only when a name char does not follow directly (names may hold `-`).
                Some('-') => '-',
                _ => return Ok(l),
            };
            self.i += 1;
            let r = self.multiplicative()?;
            l = Expr::Arith(Box::new(l), op, Box::new(r));
        }
    }
    fn multiplicative(&mut self) -> XRes<Expr> {
        let mut l = self.unary()?;
        loop {
            self.ws();
            if self.peek() == Some('*') {
                self.i += 1;
                let r = self.unary()?;
                l = Expr::Arith(Box::new(l), '*', Box::new(r));
            } else if self.word("div") {
                let r = self.unary()?;
                l = Expr::Arith(Box::new(l), '/', Box::new(r));
            } else if self.word("mod") {
                let r = self.unary()?;
                l = Expr::Arith(Box::new(l), '%', Box::new(r));
            } else {
                return Ok(l);
            }
        }
    }
    fn unary(&mut self) -> XRes<Expr> {
        self.ws();
        if self.peek() == Some('-') {
            self.i += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.union()
    }
    fn union(&mut self) -> XRes<Expr> {
        let mut parts = vec![self.path_expr()?];
        while self.eat("|") {
            parts.push(self.path_expr()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::Union(parts)
        })
    }

    fn path_expr(&mut self) -> XRes<Expr> {
        self.ws();
        match self.peek() {
            Some('/') => self.location(true),
            Some('\'') | Some('"') => {
                let q = self.peek().unwrap();
                self.i += 1;
                let start = self.i;
                while self.peek().is_some_and(|c| c != q) {
                    self.i += 1;
                }
                if self.peek().is_none() {
                    return malformed("unterminated literal");
                }
                let t: String = self.s[start..self.i].iter().collect();
                self.i += 1;
                Ok(Expr::Str(t))
            }
            Some(c) if c.is_ascii_digit() || c == '.' && self.s.get(self.i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.i += 1;
                }
                let t: String = self.s[start..self.i].iter().collect();
                t.parse().map(Expr::Num).or_else(|_| malformed("number"))
            }
            Some('(') => {
                self.i += 1;
                let e = self.expr()?;
                if !self.eat(")") {
                    return malformed("unclosed (");
                }
                self.filter_tail(e)
            }
            Some('$') => malformed("variables are not supported"),
            Some(c) if is_name_start(c) => {
                // A function call is a name followed by `(`, unless it is a node test or an axis.
                let save = self.i;
                let name = self.name().unwrap();
                self.ws();
                let is_node_test = matches!(name.as_str(), "text" | "node" | "comment" | "processing-instruction");
                if self.peek() == Some('(') && !is_node_test {
                    self.i += 1;
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expr()?);
                            if self.eat(",") {
                                continue;
                            }
                            if self.eat(")") {
                                break;
                            }
                            return malformed("unclosed function call");
                        }
                    }
                    return self.filter_tail(Expr::Call(name, args));
                }
                self.i = save;
                self.location(false)
            }
            Some(_) => self.location(false),
            None => malformed("empty expression"),
        }
    }

    /// Steps after a function call or a parenthesised expression: `(a)/b`, `f()[1]`.
    fn filter_tail(&mut self, e: Expr) -> XRes<Expr> {
        self.ws();
        if self.peek() == Some('/') {
            let mut steps = Vec::new();
            while self.peek() == Some('/') {
                self.i += 1;
                if self.peek() == Some('/') {
                    self.i += 1;
                    steps.push(Step {
                        axis: Axis::DescendantOrSelf,
                        test: Test::Node,
                        preds: vec![],
                    });
                }
                steps.push(self.step()?);
                self.ws();
            }
            return Ok(Expr::Filter(Box::new(e), steps));
        }
        Ok(e)
    }

    fn location(&mut self, absolute_allowed: bool) -> XRes<Expr> {
        let mut steps = Vec::new();
        let mut absolute = false;
        self.ws();
        if absolute_allowed && self.peek() == Some('/') {
            absolute = true;
            self.i += 1;
            if self.peek() == Some('/') {
                self.i += 1;
                steps.push(Step {
                    axis: Axis::DescendantOrSelf,
                    test: Test::Node,
                    preds: vec![],
                });
                steps.push(self.step()?);
            } else {
                self.ws();
                // `/` alone is the root.
                if self.peek().is_none() || matches!(self.peek(), Some(')' | ']' | '|' | ',' | '=' | '!' | '<' | '>')) {
                    return Ok(Expr::Path(true, steps));
                }
                steps.push(self.step()?);
            }
        } else {
            steps.push(self.step()?);
        }
        loop {
            self.ws();
            if self.peek() == Some('/') {
                self.i += 1;
                if self.peek() == Some('/') {
                    self.i += 1;
                    steps.push(Step {
                        axis: Axis::DescendantOrSelf,
                        test: Test::Node,
                        preds: vec![],
                    });
                }
                steps.push(self.step()?);
            } else {
                break;
            }
        }
        Ok(Expr::Path(absolute, steps))
    }

    fn step(&mut self) -> XRes<Step> {
        self.ws();
        let (axis, test);
        if self.starts("..") {
            self.i += 2;
            axis = Axis::Parent;
            test = Test::Node;
        } else if self.peek() == Some('.') {
            self.i += 1;
            axis = Axis::Current;
            test = Test::Node;
        } else {
            let mut ax = Axis::Child;
            if self.peek() == Some('@') {
                self.i += 1;
                ax = Axis::Attribute;
            } else {
                // `axis::`
                let save = self.i;
                if let Some(n) = self.name() {
                    if self.starts("::") {
                        self.i += 2;
                        ax = match n.as_str() {
                            "child" => Axis::Child,
                            "descendant" => Axis::Descendant,
                            "descendant-or-self" => Axis::DescendantOrSelf,
                            "self" => Axis::Current,
                            "parent" => Axis::Parent,
                            "ancestor" => Axis::Ancestor,
                            "ancestor-or-self" => Axis::AncestorOrSelf,
                            "following-sibling" => Axis::FollowingSibling,
                            "preceding-sibling" => Axis::PrecedingSibling,
                            "attribute" => Axis::Attribute,
                            other => return malformed(format!("axis {other}")),
                        };
                    } else {
                        self.i = save;
                    }
                }
            }
            axis = ax;
            self.ws();
            if self.peek() == Some('*') {
                self.i += 1;
                test = Test::Any;
            } else {
                // A name may hold one `:`.
                let Some(first) = self.name() else {
                    return malformed("a step without a name");
                };
                let mut qn = first;
                if self.peek() == Some(':') && self.s.get(self.i + 1) != Some(&':') {
                    self.i += 1;
                    if self.peek() == Some('*') {
                        self.i += 1;
                        qn.push_str(":*");
                    } else {
                        let Some(l) = self.name() else {
                            return malformed("a prefix without a name");
                        };
                        qn = format!("{qn}:{l}");
                    }
                }
                self.ws();
                if self.peek() == Some('(') {
                    self.i += 1;
                    if !self.eat(")") {
                        return malformed("node test with arguments");
                    }
                    test = match qn.as_str() {
                        "text" => Test::Text,
                        "node" => Test::Node,
                        "comment" => Test::Comment,
                        "processing-instruction" => Test::Pi,
                        other => return malformed(format!("function {other}() as a step")),
                    };
                } else {
                    let (pre, local) = match qn.split_once(':') {
                        Some((p, l)) => (Some(p.to_string()), l.to_string()),
                        None => (None, qn),
                    };
                    if pre.is_some() && self.prefixes.is_none() {
                        return malformed("a prefixed name needs a namespace manager");
                    }
                    test = Test::Name(pre, local);
                }
            }
        }
        let mut preds = Vec::new();
        loop {
            self.ws();
            if self.peek() != Some('[') {
                break;
            }
            self.i += 1;
            preds.push(self.expr()?);
            if !self.eat("]") {
                return malformed("unclosed [");
            }
        }
        Ok(Step { axis, test, preds })
    }
}

fn parse_xpath(p: &str, prefixes: Prefixes) -> XRes<Expr> {
    // `/r/@@@`: more than one `@` in a row is the pinned "not a node set" (a path that cannot be evaluated).
    if p.contains("@@") {
        return Err(XFail::NotNodeSet);
    }
    let mut parser = XParser {
        s: p.chars().collect(),
        i: 0,
        prefixes,
    };
    let e = parser.expr()?;
    parser.ws();
    if parser.i != parser.s.len() {
        return malformed("trailing characters");
    }
    Ok(e)
}

#[derive(Clone)]
enum XVal {
    Nodes(Vec<Handle>),
    Num(f64),
    Str(String),
    Bool(bool),
}

struct XCtx<'a> {
    prefixes: Prefixes<'a>,
    /// Document order of every node of the tree the query started in.
    order: std::collections::HashMap<*const RefCell<LibraryValue>, usize>,
}

fn doc_order(root: &Handle, out: &mut std::collections::HashMap<*const RefCell<LibraryValue>, usize>) {
    let n = out.len();
    out.insert(Rc::as_ptr(root), n);
    for a in attrs_of(root) {
        let n = out.len();
        out.insert(Rc::as_ptr(&a), n);
    }
    for c in children_of(root) {
        doc_order(&c, out);
    }
}

fn string_value(h: &Handle) -> String {
    with(h, |d| match d {
        XData::Attribute { value, .. } => Some(value.clone()),
        XData::Text(t) | XData::CData(t) | XData::Comment(t) => Some(t.clone()),
        XData::Pi { data, .. } => Some(data.clone()),
        _ => None,
    })
    .unwrap_or_else(|| {
        let mut s = String::new();
        inner_text(h, &mut s);
        s
    })
}

fn to_str(v: &XVal) -> String {
    match v {
        XVal::Str(s) => s.clone(),
        XVal::Num(n) => {
            if n.fract() == 0.0 && n.is_finite() {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        XVal::Bool(b) => b.to_string(),
        XVal::Nodes(n) => n.first().map(string_value).unwrap_or_default(),
    }
}

fn to_num(v: &XVal) -> f64 {
    match v {
        XVal::Num(n) => *n,
        XVal::Bool(b) => f64::from(u8::from(*b)),
        other => to_str(other).trim().parse().unwrap_or(f64::NAN),
    }
}

fn to_bool(v: &XVal) -> bool {
    match v {
        XVal::Bool(b) => *b,
        XVal::Num(n) => *n != 0.0 && !n.is_nan(),
        XVal::Str(s) => !s.is_empty(),
        XVal::Nodes(n) => !n.is_empty(),
    }
}

fn node_matches(h: &Handle, test: &Test, principal_attr: bool, ctx: &XCtx) -> XRes<bool> {
    let wanted = |pre: &Option<String>| -> XRes<String> {
        match pre {
            None => Ok(String::new()),
            Some(pre) => ctx
                .prefixes
                .and_then(|m| m.iter().rev().find(|(p, _)| p == pre))
                .map(|(_, u)| u.clone())
                .map_or_else(|| malformed(format!("prefix {pre} is not declared")), Ok),
        }
    };
    Ok(match test {
        Test::Name(pre, n) => {
            let want = wanted(pre)?;
            with(h, |d| match d {
                XData::Element { local, ns, .. } if !principal_attr => *ns == want && local == n,
                XData::Attribute { local, ns, .. } if principal_attr => *ns == want && local == n,
                _ => false,
            })
        }
        Test::Any => with(h, |d| match d {
            XData::Element { .. } => !principal_attr,
            XData::Attribute { ns, .. } => principal_attr && ns != XMLNS_URI,
            _ => false,
        }),
        Test::Node => true,
        Test::Text => with(h, |d| matches!(d, XData::Text(_) | XData::CData(_))),
        Test::Comment => with(h, |d| matches!(d, XData::Comment(_))),
        Test::Pi => with(h, |d| matches!(d, XData::Pi { .. })),
    })
}

fn descendants(h: &Handle, out: &mut Vec<Handle>) {
    for c in children_of(h) {
        out.push(c.clone());
        descendants(&c, out);
    }
}

fn sort_dedup(mut v: Vec<Handle>, ctx: &XCtx) -> Vec<Handle> {
    v.sort_by_key(|h| ctx.order.get(&Rc::as_ptr(h)).copied().unwrap_or(usize::MAX));
    v.dedup_by(|a, b| Rc::ptr_eq(a, b));
    v
}

/// The nodes of `step` from `node`, in axis order (reverse axes nearest first).
fn axis_nodes(node: &Handle, step: &Step, ctx: &XCtx) -> XRes<Vec<Handle>> {
    let mut cand: Vec<Handle> = Vec::new();
    match step.axis {
        Axis::Child => cand = children_of(node),
        Axis::Descendant => descendants(node, &mut cand),
        Axis::DescendantOrSelf => {
            cand.push(node.clone());
            descendants(node, &mut cand);
        }
        Axis::Current => cand.push(node.clone()),
        Axis::Parent => cand.extend(parent_of(node)),
        Axis::Ancestor | Axis::AncestorOrSelf => {
            if step.axis == Axis::AncestorOrSelf {
                cand.push(node.clone());
            }
            let mut cur = parent_of(node);
            while let Some(p) = cur {
                cand.push(p.clone());
                cur = parent_of(&p);
            }
        }
        Axis::FollowingSibling | Axis::PrecedingSibling => {
            if let Some(p) = parent_of(node) {
                let sibs = children_of(&p);
                if let Some(at) = sibs.iter().position(|c| Rc::ptr_eq(c, node)) {
                    if step.axis == Axis::FollowingSibling {
                        cand = sibs[at + 1..].to_vec();
                    } else {
                        cand = sibs[..at].iter().rev().cloned().collect();
                    }
                }
            }
        }
        Axis::Attribute => cand = attrs_of(node),
    }
    let principal_attr = step.axis == Axis::Attribute;
    let mut found = Vec::new();
    for c in cand {
        if node_matches(&c, &step.test, principal_attr, ctx)? {
            found.push(c);
        }
    }
    for p in &step.preds {
        let last = found.len();
        let mut keep = Vec::new();
        for (i, n) in found.iter().enumerate() {
            let v = eval(p, n, i + 1, last, ctx)?;
            let pass = match v {
                XVal::Num(x) => x == (i + 1) as f64,
                other => to_bool(&other),
            };
            if pass {
                keep.push(n.clone());
            }
        }
        found = keep;
    }
    Ok(found)
}

fn run_steps(start: Vec<Handle>, steps: &[Step], ctx: &XCtx) -> XRes<Vec<Handle>> {
    let mut current = start;
    for step in steps {
        let mut next: Vec<Handle> = Vec::new();
        for c in &current {
            next.extend(axis_nodes(c, step, ctx)?);
        }
        current = sort_dedup(next, ctx);
    }
    Ok(current)
}

fn eval(e: &Expr, node: &Handle, pos: usize, last: usize, ctx: &XCtx) -> XRes<XVal> {
    Ok(match e {
        Expr::Num(n) => XVal::Num(*n),
        Expr::Str(s) => XVal::Str(s.clone()),
        Expr::Path(absolute, steps) => {
            let start = if *absolute { root_of(node) } else { node.clone() };
            XVal::Nodes(run_steps(vec![start], steps, ctx)?)
        }
        Expr::Union(parts) => {
            let mut all = Vec::new();
            for p in parts {
                match eval(p, node, pos, last, ctx)? {
                    XVal::Nodes(n) => all.extend(n),
                    _ => return Err(XFail::NotNodeSet),
                }
            }
            XVal::Nodes(sort_dedup(all, ctx))
        }
        Expr::Filter(inner, steps) => match eval(inner, node, pos, last, ctx)? {
            XVal::Nodes(n) => XVal::Nodes(run_steps(n, steps, ctx)?),
            _ => return Err(XFail::NotNodeSet),
        },
        Expr::Neg(x) => XVal::Num(-to_num(&eval(x, node, pos, last, ctx)?)),
        Expr::Arith(a, op, b) => {
            let (x, y) = (
                to_num(&eval(a, node, pos, last, ctx)?),
                to_num(&eval(b, node, pos, last, ctx)?),
            );
            XVal::Num(match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => x / y,
                _ => x % y,
            })
        }
        Expr::And(a, b) => {
            XVal::Bool(to_bool(&eval(a, node, pos, last, ctx)?) && to_bool(&eval(b, node, pos, last, ctx)?))
        }
        Expr::Or(a, b) => {
            XVal::Bool(to_bool(&eval(a, node, pos, last, ctx)?) || to_bool(&eval(b, node, pos, last, ctx)?))
        }
        Expr::Cmp(a, op, b) => {
            let (x, y) = (eval(a, node, pos, last, ctx)?, eval(b, node, pos, last, ctx)?);
            XVal::Bool(compare(&x, op, &y))
        }
        Expr::Call(name, args) => {
            let mut vals = Vec::new();
            for a in args {
                vals.push(eval(a, node, pos, last, ctx)?);
            }
            let first_node = |vals: &[XVal]| match vals.first() {
                Some(XVal::Nodes(n)) => n.first().cloned(),
                None => Some(node.clone()),
                _ => None,
            };
            match (name.as_str(), vals.len()) {
                ("last", 0) => XVal::Num(last as f64),
                ("position", 0) => XVal::Num(pos as f64),
                ("count", 1) => match &vals[0] {
                    XVal::Nodes(n) => XVal::Num(n.len() as f64),
                    _ => return Err(XFail::NotNodeSet),
                },
                ("local-name", _) => XVal::Str(
                    first_node(&vals)
                        .map(|n| {
                            with(&n, |d| match d {
                                XData::Element { local, .. } | XData::Attribute { local, .. } => local.clone(),
                                XData::Pi { target, .. } => target.clone(),
                                _ => String::new(),
                            })
                        })
                        .unwrap_or_default(),
                ),
                ("name", _) => XVal::Str(
                    first_node(&vals)
                        .map(|n| {
                            with(&n, |d| match d {
                                XData::Element { prefix, local, .. } | XData::Attribute { prefix, local, .. } => {
                                    qname(prefix, local)
                                }
                                _ => String::new(),
                            })
                        })
                        .unwrap_or_default(),
                ),
                ("string", 0) => XVal::Str(string_value(node)),
                ("string", 1) => XVal::Str(to_str(&vals[0])),
                ("number", 1) => XVal::Num(to_num(&vals[0])),
                ("not", 1) => XVal::Bool(!to_bool(&vals[0])),
                ("true", 0) => XVal::Bool(true),
                ("false", 0) => XVal::Bool(false),
                ("boolean", 1) => XVal::Bool(to_bool(&vals[0])),
                ("contains", 2) => XVal::Bool(to_str(&vals[0]).contains(&to_str(&vals[1]))),
                ("starts-with", 2) => XVal::Bool(to_str(&vals[0]).starts_with(&to_str(&vals[1]))),
                ("string-length", 1) => XVal::Num(to_str(&vals[0]).chars().count() as f64),
                ("normalize-space", 1) => XVal::Str(to_str(&vals[0]).split_whitespace().collect::<Vec<_>>().join(" ")),
                (other, n) => return malformed(format!("function {other} with {n} arguments")),
            }
        }
    })
}

fn compare(a: &XVal, op: &str, b: &XVal) -> bool {
    let cmp_num = |x: f64, y: f64| match op {
        "=" => x == y,
        "!=" => x != y,
        "<" => x < y,
        "<=" => x <= y,
        ">" => x > y,
        _ => x >= y,
    };
    let cmp_str = |x: &str, y: &str| match op {
        "=" => x == y,
        "!=" => x != y,
        _ => cmp_num(
            x.trim().parse().unwrap_or(f64::NAN),
            y.trim().parse().unwrap_or(f64::NAN),
        ),
    };
    match (a, b) {
        (XVal::Nodes(n), XVal::Nodes(m)) => n
            .iter()
            .any(|x| m.iter().any(|y| cmp_str(&string_value(x), &string_value(y)))),
        (XVal::Nodes(n), XVal::Num(y)) | (XVal::Num(y), XVal::Nodes(n)) => {
            let flip = matches!(a, XVal::Num(_));
            n.iter().any(|x| {
                let v: f64 = string_value(x).trim().parse().unwrap_or(f64::NAN);
                if flip { cmp_num(*y, v) } else { cmp_num(v, *y) }
            })
        }
        (XVal::Nodes(n), XVal::Str(s)) => n.iter().any(|x| cmp_str(&string_value(x), s)),
        (XVal::Str(s), XVal::Nodes(n)) => n.iter().any(|x| cmp_str(s, &string_value(x))),
        (XVal::Bool(_), _) | (_, XVal::Bool(_)) => {
            let (x, y) = (to_bool(a), to_bool(b));
            match op {
                "=" => x == y,
                "!=" => x != y,
                _ => cmp_num(f64::from(u8::from(x)), f64::from(u8::from(y))),
            }
        }
        (XVal::Num(_), _) | (_, XVal::Num(_)) => cmp_num(to_num(a), to_num(b)),
        _ => cmp_str(&to_str(a), &to_str(b)),
    }
}

/// The nodes `path` selects from `ctx`. A result that is not a node set (a number, a string) selects nothing.
fn xpath(ctx: &Handle, path: &str, prefixes: Prefixes) -> XRes<Vec<Handle>> {
    let expr = parse_xpath(path, prefixes)?;
    let root = root_of(ctx);
    let mut order = std::collections::HashMap::new();
    doc_order(&root, &mut order);
    let xc = XCtx { prefixes, order };
    match eval(&expr, ctx, 1, 1, &xc)? {
        XVal::Nodes(n) => Ok(n),
        _ => Ok(Vec::new()),
    }
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

fn read_from_text(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    read_text_into(c, r, a, 1)
}

/// `XmlDocument.ReadFrom(Text, XmlReadOptions, var XmlDocument)`: the document reads as the form without options
/// does, whether the options are the default or `PreserveWhitespace` is true or false, for white space inside the
/// text, around the root and between tags (misc/W4XmlJsonHttp.al: W4 Xml Options Tests.ReadFrom_*, BC 27.5 and 29.0).
fn read_from_text_options(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    read_text_into(c, r, a, 2)
}

/// `XmlReadOptions.PreserveWhitespace(Value)`: the probes see no difference in what is read, so the setting changes
/// nothing here.
fn read_options_preserve_whitespace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if !matches!(r, Some(Value::Library(..))) {
        return unsupported("PreserveWhitespace on something that is not an XmlReadOptions");
    }
    let Some(Value::Boolean(_)) = a.first() else {
        return unsupported("PreserveWhitespace with something that is not a Boolean");
    };
    Ok(Value::Null)
}

fn read_text_into(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value], out: usize) -> NResult {
    let text = arg_text(a, 0)?;
    match parse_document_detail(&text, true)? {
        Ok(doc) => {
            a[out] = tagged("XmlDocument", doc);
            Ok(Value::Boolean(true))
        }
        Err(_) if c.ret_used() => Ok(Value::Boolean(false)),
        Err(Some(Failure {
            message,
            line,
            pos,
            dom: true,
        })) => Err(error(
            format!(
                "The following exception was encountered when processing XML data: '{message}' at line {line} and position {pos}."
            ),
            "Xml:Xml",
        )),
        Err(_) => unsupported(
            "XmlDocument.ReadFrom of invalid XML, result unused (the error text is pinned only for a mismatched end tag)",
        ),
    }
}

/// `XmlDocument.ReadFrom(InStream, var XmlDocument)`: reads the stream to its end and parses it as
/// the text form does (handlers/TestReportDatasetHelpLink.al:RenderDataset reads what `Report.SaveAs`
/// wrote, a UTF-8 document with a byte order mark). The encoding is the byte order mark's, else UTF-8;
/// a UTF-16 document and bytes that are not UTF-8 are refused (not pinned).
fn read_from_stream(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    // The stream is read from its position without moving it: `EOS` stays false afterwards (xml/TestProbeXmlContracts.al:
    // ReadFrom_Stream_Encodings, `ok=Yes eos=No`).
    let bytes = match a.first() {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::InStream(s) => s.rest(),
            _ => return unsupported("XmlDocument.ReadFrom of something that is not an InStream"),
        },
        other => return unsupported(format!("XmlDocument.ReadFrom of {}", other.describe())),
    };
    // A byte order mark names the encoding; without one, UTF-8, else UTF-16 little endian when the second byte is zero
    // (a text written with TextEncoding::UTF16). Bytes that are neither read as no document.
    let decoded: Option<String> = if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|p| {
                if le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    } else if bytes.len() >= 2 && bytes.len() % 2 == 0 && bytes[0] != 0 && bytes[1] == 0 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        String::from_utf16(&units).ok()
    } else {
        let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        std::str::from_utf8(body).ok().map(str::to_string)
    };
    let Some(text) = decoded else {
        return if c.ret_used() {
            Ok(Value::Boolean(false))
        } else {
            unsupported(
                "XmlDocument.ReadFrom of a stream whose bytes are no UTF-8 or UTF-16 text, result unused (error text not pinned)",
            )
        };
    };
    let mut args = [Value::Text(text), a[1].clone()];
    let result = read_from_text(c, None, &mut args)?;
    a[1] = args[1].clone();
    Ok(result)
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
        open: false,
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
        open: false,
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
            explicit: true,
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
                // xml/TestXmlRuntimeContracts.al:XmlElement_NamespaceLookups: the default namespace
                // has an empty prefix.
                Some(None) => {
                    a[1] = Value::Text(String::new());
                    return Ok(Value::Boolean(true));
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
    // The empty prefix has no namespace and leaves the variable alone
    // (xml/TestXmlRuntimeContracts.al:XmlElement_NamespaceLookups).
    if prefix.is_empty() && c.ret_used() {
        return Ok(Value::Boolean(false));
    }
    if prefix.is_empty() {
        // xml/TestProbeXmlContracts.al:GetNamespaceOfPrefix_Empty_Statement
        return Err(nav_argument());
    }
    match lookup_namespace(&h, &prefix) {
        Some(u) => {
            a[1] = Value::Text(u);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => Err(nav_argument()),
    }
}

// ----- structure

fn add(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    // A text given as a string joins the text node it follows; an XmlText node is its own node
    // (xml/TestProbeXmlContracts.al:AdjacentTextNodes_NotMerged).
    for arg in a.iter() {
        if let Value::Text(t) | Value::Code(t) = arg {
            let last = children_of(&h).last().cloned();
            if let Some(last) = last.filter(|l| with(l, |d| matches!(d, XData::Text(_)))) {
                with(&last, |d| {
                    if let XData::Text(x) = d {
                        x.push_str(t);
                    }
                });
                continue;
            }
        }
        insert(&h, None, content_nodes(std::slice::from_ref(arg))?)?;
    }
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
        check_fits(&p, n, std::slice::from_ref(&h))?;
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
    let old = children_of(&h);
    for n in &nodes {
        check_fits(&h, n, &old)?;
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

/// Document types are not modelled (the parser refuses a DOCTYPE), so a document never has one:
/// the Boolean form is false and the statement form raises
/// (xml/TestXmlRuntimeContracts.al:XmlDocument_GetDocumentType_WithoutDoctype).
fn get_document_type(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    recv(r)?;
    if c.ret_used() {
        Ok(Value::Boolean(false))
    } else {
        Err(error("The XML node is not set.", "XmlNodeNotFound"))
    }
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
        // xml/TestProbeXmlContracts.al:GetDocument_Detached_Statement
        Err(error("The XML node is not set.", "XmlNodeNotFound"))
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
        // xml/TestProbeXmlContracts.al:GetParent_Root_Statement
        None => Err(error("The XML node is not set.", "XmlNodeNotFound")),
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
                // xml/TestProbeXmlContracts.al:XPath_CountsForPaths (AsXmlElement on an attribute and on a text).
                Err(error(
                    format!(
                        "Unable to convert from Microsoft.Dynamics.Nav.Runtime.Nav{} to Microsoft.Dynamics.Nav.Runtime.Nav{}.",
                        kind(&h),
                        $kind
                    ),
                    "Conversion",
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

/// `IsXmlText` is true for a text node only; a CDATA section is not text
/// (xml/TestXmlRuntimeContracts.al:XmlNode_CDataIsNotText).
fn is_text(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    Ok(Value::Boolean(kind(&h) == "XmlText"))
}
fn as_text(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = recv(r)?;
    if kind(&h) == "XmlText" {
        Ok(tagged("XmlText", h))
    } else {
        Err(error(
            format!(
                "Unable to convert from Microsoft.Dynamics.Nav.Runtime.Nav{} to Microsoft.Dynamics.Nav.Runtime.NavXmlText.",
                kind(&h)
            ),
            "Conversion",
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
    Ok(Value::Text(serialize_inner(&h)?))
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

/// Runs an XPath for a SelectNodes or SelectSingleNode call: `None` is a path that is no XPath (or names a prefix nobody
/// declared) when the result is used, which answers false; the statement form raises the pinned "not a node set" error and
/// refuses the other cases (xml/TestProbeXmlContracts.al:XPath_MalformedStatement, XPath_NamespaceManager).
fn select(c: &mut dyn Ctx, h: &Handle, path: &str, prefixes: Prefixes) -> Result<Option<Vec<Handle>>, Signal> {
    match xpath(h, path, prefixes) {
        Ok(v) => Ok(Some(v)),
        Err(XFail::Signal(s)) => Err(s),
        Err(XFail::NotNodeSet) if !c.ret_used() => Err(error("Expression must evaluate to a node-set.", "Xml:XPath")),
        Err(XFail::Malformed(why)) if !c.ret_used() => {
            unsupported(format!("XPath `{path}`: {why} (statement form, error text not pinned)"))
        }
        Err(_) => Ok(None),
    }
}

fn select_single_node(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let found = select(c, &h, &arg_text(a, 0)?, None)?;
    single_result(c, found, a, 1)
}

fn single_result(c: &mut dyn Ctx, found: Option<Vec<Handle>>, a: &mut [Value], slot: usize) -> NResult {
    match found.and_then(|f| f.into_iter().next()) {
        Some(n) => {
            a[slot] = tagged("XmlNode", n);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("SelectSingleNode without a match, result unused (error text not pinned yet)"),
    }
}

fn select_nodes(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let found = select(c, &h, &arg_text(a, 0)?, None)?;
    a[1] = list_of(found.clone().unwrap_or_default());
    Ok(Value::Boolean(found.is_some()))
}

/// The prefixes of the `XmlNamespaceManager` in argument `i`.
fn manager_prefixes(a: &[Value], i: usize) -> Result<Vec<(String, String)>, Signal> {
    match a.get(i) {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::XmlNamespaces(v) => Ok(v.clone()),
            LibraryValue::Empty => Ok(Vec::new()),
            _ => unsupported("an XmlNamespaceManager that is not one"),
        },
        other => unsupported(format!("{} as an XmlNamespaceManager", other.describe())),
    }
}

fn select_nodes_ns(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let prefixes = manager_prefixes(a, 1)?;
    let found = select(c, &h, &arg_text(a, 0)?, Some(&prefixes))?;
    a[2] = list_of(found.clone().unwrap_or_default());
    Ok(Value::Boolean(found.is_some()))
}

fn select_single_node_ns(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let prefixes = manager_prefixes(a, 1)?;
    let found = select(c, &h, &arg_text(a, 0)?, Some(&prefixes))?;
    single_result(c, found, a, 2)
}

/// The receiver of an `XmlNamespaceManager` method, made on first use.
fn manager_cell(r: Option<&mut Value>) -> Result<Handle, Signal> {
    match r {
        Some(Value::Library(_, h)) => {
            if matches!(&*h.borrow(), LibraryValue::Empty) {
                *h.borrow_mut() = LibraryValue::XmlNamespaces(Vec::new());
            }
            if matches!(&*h.borrow(), LibraryValue::XmlNamespaces(_)) {
                Ok(h.clone())
            } else {
                unsupported("an XmlNamespaceManager method on something else")
            }
        }
        _ => unsupported("an XmlNamespaceManager method without a receiver"),
    }
}

/// `NameTable(NewValue)` is accepted and has no effect (names are compared as text);
/// reading it back is not pinned.
/// (xml/TestXmlRuntimeContracts.al:XmlDocument_SelectNodes_DefaultNamespace_NeedsAPrefixedName)
fn manager_name_table(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    manager_cell(r)?;
    if a.is_empty() {
        return unsupported("XmlNamespaceManager.NameTable read back (not pinned)");
    }
    Ok(Value::Null)
}

fn manager_add_namespace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = manager_cell(r)?;
    let (prefix, uri) = (arg_text(a, 0)?, arg_text(a, 1)?);
    if let LibraryValue::XmlNamespaces(v) = &mut *h.borrow_mut() {
        v.push((prefix, uri));
    }
    Ok(Value::Null)
}

/// `LookupNamespace(Prefix, var Uri)` and `HasNamespace(Prefix)` (xml/TestProbeXmlContracts.al:XPath_NamespaceManager:
/// `p` gives `Yes urn:p`, an unknown prefix `No`). A prefix added twice answers the last one.
fn manager_lookup_namespace(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = manager_cell(r)?;
    let prefix = arg_text(a, 0)?;
    let found = match &*h.borrow() {
        LibraryValue::XmlNamespaces(v) => v.iter().rev().find(|(p, _)| *p == prefix).map(|(_, u)| u.clone()),
        _ => None,
    };
    match found {
        Some(u) => {
            a[1] = Value::Text(u);
            Ok(Value::Boolean(true))
        }
        None if c.ret_used() => Ok(Value::Boolean(false)),
        None => unsupported("XmlNamespaceManager.LookupNamespace of an unknown prefix, result unused (not pinned)"),
    }
}

fn manager_has_namespace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = manager_cell(r)?;
    let prefix = arg_text(a, 0)?;
    Ok(Value::Boolean(match &*h.borrow() {
        LibraryValue::XmlNamespaces(v) => v.iter().any(|(p, _)| *p == prefix),
        _ => false,
    }))
}

fn write_to_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    a[0] = Value::Text(serialize(&h)?);
    Ok(Value::Boolean(true))
}

/// `WriteTo([XmlWriteOptions,] OutStream)`: UTF-8 with a byte order mark, whatever the encoding of the
/// stream, the declaration saying `utf-8`; `PreserveWhitespace(true)` writes the document unindented
/// (xml/TestXmlDocumentWriteToOutStreamContracts.al). A document whose own declaration names another
/// encoding or none is refused: nothing pins what is written then.
fn write_to_outstream(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = recv(r)?;
    let (options, stream) = match a.len() {
        1 => (None, 0),
        _ => (Some(&a[0]), 1),
    };
    let indent = match options {
        None => true,
        Some(Value::Library(_, cell)) => match &*cell.borrow() {
            LibraryValue::XmlWriteOptions { preserve_whitespace } => !preserve_whitespace,
            LibraryValue::Empty => true,
            _ => return unsupported("WriteTo with something that is not an XmlWriteOptions"),
        },
        Some(_) => return unsupported("WriteTo with something that is not an XmlWriteOptions"),
    };
    let text = serialize_with(&h, indent)?;
    let is_doc = with(&h, |d| matches!(d, XData::Document { .. }));
    if is_doc && !text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"") {
        return unsupported("WriteTo(OutStream) of a document that does not declare encoding utf-8 (not pinned)");
    }
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.as_bytes());
    crate::stream::write_raw_bytes(&mut a[stream], &bytes)?;
    Ok(Value::Boolean(true))
}

/// `XmlWriteOptions.PreserveWhitespace(Value)` (xml/TestXmlDocumentWriteToOutStreamContracts.al).
fn write_options_preserve_whitespace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("PreserveWhitespace on something that is not an XmlWriteOptions");
    };
    let Some(Value::Boolean(on)) = a.first() else {
        return unsupported("PreserveWhitespace with something that is not a Boolean");
    };
    *cell.borrow_mut() = LibraryValue::XmlWriteOptions {
        preserve_whitespace: *on,
    };
    Ok(Value::Null)
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

/// The nodes of an `XmlNodeList` as `XmlNode` values, for `foreach`. `None` for any other value.
pub fn node_list_items(v: &Value) -> Option<Vec<Value>> {
    let Value::Library(_, h) = v else { return None };
    match &*h.borrow() {
        LibraryValue::XmlList(items) => Some(items.iter().map(|n| tagged("XmlNode", n.clone())).collect()),
        _ => None,
    }
}

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
/// The element tree of a document, for XmlPort import (`xmlport.rs`). `Ok(None)`: not
/// well-formed; `Err`: well-formed but uses namespaces or other features the reader does not
/// carry over.
pub(crate) fn read_tree(text: &str) -> Result<Result<crate::xmlport::Elem, Option<String>>, String> {
    use crate::xmlport::{Elem, Node};
    fn convert(h: &Handle) -> Result<Elem, String> {
        let (prefix, local, ns, attrs) = with(h, |d| match d {
            XData::Element {
                prefix,
                local,
                ns,
                attrs,
                ..
            } => (prefix.clone(), local.clone(), ns.clone(), attrs.clone()),
            _ => unreachable!("callers pass elements"),
        });
        if !prefix.is_empty() || !ns.is_empty() {
            return Err("namespaces in an XmlPort document".into());
        }
        let mut out = Elem::new(&local);
        for a in attrs {
            let (p, l, n, v) = with(&a, |d| match d {
                XData::Attribute {
                    prefix,
                    local,
                    ns,
                    value,
                } => (prefix.clone(), local.clone(), ns.clone(), value.clone()),
                _ => unreachable!("attributes of an element"),
            });
            if !p.is_empty() || !n.is_empty() || l == "xmlns" {
                return Err("namespace declarations in an XmlPort document".into());
            }
            out.attrs.push((l, v));
        }
        for c in children_of(h) {
            enum K {
                Element,
                Text(String),
                Skip,
            }
            match with(&c, |d| match d {
                XData::Element { .. } => K::Element,
                XData::Text(t) | XData::CData(t) => K::Text(t.clone()),
                _ => K::Skip,
            }) {
                K::Element => out.children.push(Node::Elem(convert(&c)?)),
                K::Text(t) => out.children.push(Node::Text(t)),
                K::Skip => {}
            }
        }
        Ok(out)
    }
    let doc = match parse_document_detail(text, false) {
        Ok(Ok(d)) => d,
        Ok(Err(f)) => return Ok(Err(f.map(|f| f.message))),
        Err(Signal::Unsupported(why)) => return Err(why),
        Err(other) => return Err(other.describe()),
    };
    let root = children_of(&doc)
        .into_iter()
        .find(|c| with(c, |d| matches!(d, XData::Element { .. })))
        .ok_or("a document without a root element")?;
    convert(&root).map(Ok)
}

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
        ("XmlDocument", "ReadFrom", ["Text", "XmlReadOptions", "XmlDocument"]) => read_from_text_options,
        ("XmlDocument", "ReadFrom", ["InStream", "XmlDocument"]) => read_from_stream,
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
        (_, "SelectNodes", ["Text", "XmlNamespaceManager", "XmlNodeList"]) if is_node => select_nodes_ns,
        (_, "SelectSingleNode", ["Text", "XmlNamespaceManager", "XmlNode"]) if is_node => select_single_node_ns,
        ("XmlNamespaceManager", "NameTable", [] | ["XmlNameTable"]) => manager_name_table,
        ("XmlNamespaceManager", "AddNamespace", ["Text", "Text"]) => manager_add_namespace,
        ("XmlNamespaceManager", "LookupNamespace", ["Text", "Text"]) => manager_lookup_namespace,
        ("XmlNamespaceManager", "HasNamespace", ["Text"]) => manager_has_namespace,
        (_, "WriteTo", ["Text"]) if is_node && first_by_var => write_to_text,
        ("XmlDocument" | "XmlElement", "WriteTo", ["OutStream"] | ["XmlWriteOptions", "OutStream"]) => {
            write_to_outstream
        }
        ("XmlWriteOptions", "PreserveWhitespace", ["Boolean"]) => write_options_preserve_whitespace,
        ("XmlReadOptions", "PreserveWhitespace", ["Boolean"]) => read_options_preserve_whitespace,
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
