//! Pages for the test client (ADR-012): the source-level step that turns
//! expression-valued control properties into routines, and the layout of a
//! page's control tree.
//!
//! `Visible = not HideIt;` is AL code, so it must be resolved and lowered like
//! any other. Rather than teach the checker a second expression entry point,
//! `expand` appends one synthetic procedure per such property to the page
//! object before the file is parsed for good, named from the property and its
//! offset in the original text (`__visible_123`). Original offsets do not move,
//! because the text is only added at the end of an object.

use al_syntax::ast;
use std::collections::BTreeSet;

/// What a synthetic property routine returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    Boolean,
    Text,
    Variant,
}

impl PropType {
    fn al(self) -> &'static str {
        match self {
            PropType::Boolean => "Boolean",
            PropType::Text => "Text",
            PropType::Variant => "Variant",
        }
    }
}

/// Section names whose properties belong to a page control.
const CONTROL_SECTIONS: &[&str] = &["field", "group", "repeater", "part", "action", "systemaction", "modify"];

/// The type a property evaluates to, when it is one the client evaluates.
pub fn dynamic_type(prop: &str) -> Option<PropType> {
    match prop.to_ascii_lowercase().as_str() {
        "visible" | "editable" | "enabled" => Some(PropType::Boolean),
        "autoformatexpression" | "captionclass" => Some(PropType::Text),
        _ => None,
    }
}

/// Whether a property value is a literal rather than an expression.
pub fn is_literal(ty: PropType, value: &str) -> bool {
    let v = value.trim();
    match ty {
        PropType::Boolean => v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false"),
        PropType::Text => v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2 && !v[1..v.len() - 1].contains('\''),
        PropType::Variant => false,
    }
}

/// Name of the synthetic routine for property `prop` of the node at `offset`
/// (counted from the start of its object, so inserting text into an earlier
/// object of the same file does not rename it).
pub fn synthetic_name(prop: &str, offset: usize) -> String {
    format!("__{}_{offset}", prop.to_ascii_lowercase())
}

/// Whether `name` is the name of a routine made by `expand`.
pub fn is_synthetic(name: &str) -> bool {
    name.strip_prefix("__")
        .and_then(|r| r.rsplit_once('_'))
        .is_some_and(|(prop, off)| !prop.is_empty() && !off.is_empty() && off.chars().all(|c| c.is_ascii_digit()))
}

/// How a field control's source expression binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `Rec.Field`
    RecField(String),
    /// A global variable of the page.
    Var(String),
    /// Anything else: evaluated by a synthetic routine, read-only.
    Expr,
}

/// Classifies the source expression `text` of a field control.
pub fn classify_source(text: &str, globals: &BTreeSet<String>) -> Source {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("Rec.").or_else(|| t.strip_prefix("rec.")) {
        let name = rest.trim();
        let unq = name.strip_prefix('"').and_then(|n| n.strip_suffix('"'));
        match unq {
            Some(n) if !n.contains('"') => return Source::RecField(n.to_string()),
            None if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.is_empty() => {
                return Source::RecField(name.to_string());
            }
            _ => {}
        }
    }
    let bare = t.strip_prefix('"').and_then(|n| n.strip_suffix('"')).unwrap_or(t);
    if globals.contains(&bare.to_lowercase()) {
        return Source::Var(bare.to_string());
    }
    Source::Expr
}

/// Global variable names (lower case) declared in `obj`.
pub fn global_names(obj: &ast::Object) -> BTreeSet<String> {
    obj.var_decls()
        .flat_map(|v| v.names())
        .map(|n| n.to_lowercase())
        .collect()
}

/// A control's section and the properties and source of it that need routines:
/// (property name, type, offset used in the name, expression text).
fn needed(section: &ast::Section, base: usize, globals: &BTreeSet<String>) -> Vec<(String, PropType, usize, String)> {
    let mut out = Vec::new();
    for p in section.properties() {
        let name = p.name();
        let Some(ty) = dynamic_type(&name) else { continue };
        let value = p.value_text();
        if !is_literal(ty, &value) {
            out.push((
                name.to_ascii_lowercase(),
                ty,
                usize::from(p.syntax().text_range().start()) - base,
                value,
            ));
        }
    }
    if section.name().eq_ignore_ascii_case("field") {
        let args = section.args();
        if let Some(src) = args.get(1) {
            if classify_source(src, globals) == Source::Expr {
                out.push((
                    "source".into(),
                    PropType::Variant,
                    usize::from(section.syntax().text_range().start()) - base,
                    src.clone(),
                ));
            }
        }
    }
    out
}

/// Every control section of a page object, in source order.
pub fn control_sections(obj: &ast::Object) -> Vec<ast::Section> {
    obj.syntax()
        .descendants()
        .filter_map(ast::Section::cast)
        .filter(|s| CONTROL_SECTIONS.iter().any(|c| s.name().eq_ignore_ascii_case(c)))
        .collect()
}

/// `src` with the synthetic routines of every page object appended, or `None`
/// when no page has an expression-valued property.
pub fn expand(src: &str, symbols: &BTreeSet<String>) -> Option<String> {
    expand_ranges(src, symbols).map(|(text, _)| text)
}

/// `expand`, with the byte ranges of the added text in the returned text (source positions
/// subtract them to give the author's lines).
pub fn expand_ranges(src: &str, symbols: &BTreeSet<String>) -> Option<(String, Vec<(usize, usize)>)> {
    let parse = al_syntax::parse(src, symbols);
    let root = ast::SourceFile::cast(parse.syntax())?;
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for obj in root.objects() {
        if !obj.kind().eq_ignore_ascii_case("page") && !obj.kind().eq_ignore_ascii_case("pageextension") {
            continue;
        }
        let Some(close) = obj.close_brace_offset() else {
            continue;
        };
        let globals = global_names(&obj);
        let base = usize::from(obj.syntax().text_range().start());
        let mut text = String::new();
        for sec in control_sections(&obj) {
            for (prop, ty, offset, expr) in needed(&sec, base, &globals) {
                text.push_str(&format!(
                    "\nprocedure {}(): {} begin exit({}); end;",
                    synthetic_name(&prop, offset),
                    ty.al(),
                    expr
                ));
            }
        }
        if !text.is_empty() {
            inserts.push((close, text));
        }
    }
    if inserts.is_empty() {
        return None;
    }
    let mut out = src.to_string();
    for (at, text) in inserts.iter().rev() {
        out.insert_str(*at, text);
    }
    // Each insert moves by the length of the ones before it.
    let mut added = 0;
    let ranges = inserts
        .iter()
        .map(|(at, text)| {
            let r = (at + added, at + added + text.len());
            added += text.len();
            r
        })
        .collect();
    Some((out, ranges))
}

/// A control found in the page's source, before bindings and routines are resolved.
pub struct RawControl {
    pub name: String,
    pub kind: al_ir::ControlKind,
    pub parent: Option<usize>,
    pub section: ast::Section,
    /// The page extension that declares the control (`None`: the page itself).
    pub owner: Option<usize>,
    /// `modify` blocks of page extensions that apply to this control, in the order the extensions
    /// apply: (extension object, the `modify` section).
    pub modifies: Vec<(usize, ast::Section)>,
}

/// The control tree of a page, from syntax alone, and why the client cannot
/// drive it yet.
pub struct Layout {
    pub controls: Vec<RawControl>,
    pub reasons: Vec<String>,
}

/// A control in the tree the extensions are merged into.
struct Node {
    name: String,
    kind: al_ir::ControlKind,
    section: ast::Section,
    owner: Option<usize>,
    modifies: Vec<(usize, ast::Section)>,
    children: Vec<usize>,
}

/// An `area(...)` of the `layout` or `actions` section: where extensions attach controls by name.
struct Area {
    name: String,
    in_actions: bool,
    children: Vec<usize>,
}

/// A list of sibling controls: the children of an area or of a group-like control.
#[derive(Clone, Copy, PartialEq)]
enum ListId {
    Area(usize),
    Node(usize),
}

struct Tree {
    nodes: Vec<Node>,
    areas: Vec<Area>,
    reasons: Vec<String>,
}

/// Walks the `layout` and `actions` sections of page object `obj`.
pub fn layout(obj: &ast::Object) -> Layout {
    layout_with(obj, &[])
}

/// The control tree of page `obj` with the page extensions `exts` merged in, in the order given
/// (the order they apply: a later `modify` of a property wins).
pub fn layout_with(obj: &ast::Object, exts: &[(usize, ast::Object)]) -> Layout {
    let mut tree = Tree {
        nodes: Vec::new(),
        areas: Vec::new(),
        reasons: Vec::new(),
    };
    for top in obj.sections() {
        let n = top.name().to_ascii_lowercase();
        if n == "layout" || n == "actions" {
            let in_actions = n == "actions";
            // Controls written straight into `layout`/`actions` (no `area`) hang off an unnamed area.
            let mut loose = Vec::new();
            for s in top.sections() {
                if s.name().eq_ignore_ascii_case("area") {
                    let mut children = Vec::new();
                    tree.add_children(&mut children, &s, in_actions, None);
                    tree.areas.push(Area {
                        name: s.args().first().cloned().unwrap_or_default(),
                        in_actions,
                        children,
                    });
                } else {
                    tree.add_one(&mut loose, &s, in_actions, None);
                }
            }
            if !loose.is_empty() {
                tree.areas.push(Area {
                    name: String::new(),
                    in_actions,
                    children: loose,
                });
            }
        }
    }
    tree.merge(exts);
    tree.flatten()
}

/// One section of an extension that changes the page: `addlast(Target) { ... }` and the like.
struct Op {
    ext: usize,
    in_actions: bool,
    section: ast::Section,
}

impl Tree {
    fn add_children(&mut self, into: &mut Vec<usize>, section: &ast::Section, in_actions: bool, owner: Option<usize>) {
        for s in section.sections() {
            self.add_one(into, &s, in_actions, owner);
        }
    }

    /// The control (or, for `area`, the controls) that section `s` declares, appended to `into`.
    fn add_one(&mut self, into: &mut Vec<usize>, s: &ast::Section, in_actions: bool, owner: Option<usize>) {
        use al_ir::ControlKind as K;
        let name = s.name().to_ascii_lowercase();
        let arg0 = s.args().first().cloned().unwrap_or_default();
        let kind = match name.as_str() {
            "area" => {
                self.add_children(into, s, in_actions, owner);
                return;
            }
            "field" => K::Field,
            "group" if in_actions => K::ActionGroup,
            "group" => K::Group,
            "repeater" => K::Repeater,
            // An actionref is a control that delegates to its action (corpus
            // handlers/TestPageActionRunObject_Tests.al:ActionrefToARunObjectActionOpensTheSameTarget,
            // handlers/TestPagePromotedActionref_Tests.al).
            "action" | "actionref" => K::Action,
            "systemaction" => K::SystemAction,
            "part" => K::Part,
            // A separator carries no behaviour of its own.
            "separator" => return,
            other => {
                self.reasons
                    .push(format!("page control kind `{other}` is not supported yet"));
                return;
            }
        };
        let idx = self.nodes.len();
        self.nodes.push(Node {
            name: arg0,
            kind: kind.clone(),
            section: s.clone(),
            owner,
            modifies: Vec::new(),
            children: Vec::new(),
        });
        into.push(idx);
        if matches!(kind, K::Group | K::Repeater | K::ActionGroup) {
            let mut kids = Vec::new();
            self.add_children(&mut kids, s, in_actions, owner);
            self.nodes[idx].children = kids;
        }
    }

    fn find_node(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name.eq_ignore_ascii_case(name))
    }

    fn list(&self, id: ListId) -> &Vec<usize> {
        match id {
            ListId::Area(a) => &self.areas[a].children,
            ListId::Node(n) => &self.nodes[n].children,
        }
    }

    fn list_mut(&mut self, id: ListId) -> &mut Vec<usize> {
        match id {
            ListId::Area(a) => &mut self.areas[a].children,
            ListId::Node(n) => &mut self.nodes[n].children,
        }
    }

    /// The list that holds node `idx`, and its position there. Only nodes that are still
    /// reachable from an area count (a node whose declaration was refused is in no list).
    fn parent_list(&self, idx: usize) -> Option<(ListId, usize)> {
        let lists = (0..self.areas.len())
            .map(ListId::Area)
            .chain((0..self.nodes.len()).map(ListId::Node));
        for id in lists {
            if let Some(at) = self.list(id).iter().position(|&c| c == idx) {
                return Some((id, at));
            }
        }
        None
    }

    /// Applies one extension section. `Ok(false)` when its target is not in the tree yet.
    fn apply(&mut self, op: &Op) -> Result<bool, String> {
        let kind = op.section.name().to_ascii_lowercase();
        let target = op.section.args().first().cloned().unwrap_or_default();
        match kind.as_str() {
            "modify" => {
                let Some(n) = self.find_node(&target) else {
                    return Ok(false);
                };
                self.nodes[n].modifies.push((op.ext, op.section.clone()));
                Ok(true)
            }
            "addfirst" | "addlast" => {
                // An area of the same section, or a control that holds others.
                let area = self
                    .areas
                    .iter()
                    .position(|a| a.in_actions == op.in_actions && a.name.eq_ignore_ascii_case(&target));
                let holder = match area {
                    Some(a) => ListId::Area(a),
                    None => match self.find_node(&target) {
                        Some(n)
                            if matches!(
                                self.nodes[n].kind,
                                al_ir::ControlKind::Group
                                    | al_ir::ControlKind::Repeater
                                    | al_ir::ControlKind::ActionGroup
                            ) =>
                        {
                            ListId::Node(n)
                        }
                        Some(_) => return Err(format!("{kind}({target}): `{target}` cannot hold controls")),
                        None => return Ok(false),
                    },
                };
                let mut fresh = Vec::new();
                self.add_children(&mut fresh, &op.section, op.in_actions, Some(op.ext));
                let list = self.list_mut(holder);
                if kind == "addfirst" {
                    list.splice(0..0, fresh);
                } else {
                    list.extend(fresh);
                }
                Ok(true)
            }
            "addafter" | "addbefore" => {
                let Some(n) = self.find_node(&target) else {
                    return Ok(false);
                };
                let Some((id, at)) = self.parent_list(n) else {
                    return Ok(false);
                };
                let mut fresh = Vec::new();
                self.add_children(&mut fresh, &op.section, op.in_actions, Some(op.ext));
                let at = if kind == "addafter" { at + 1 } else { at };
                self.list_mut(id).splice(at..at, fresh);
                Ok(true)
            }
            other => Err(format!("`{other}` in a page extension is not supported yet")),
        }
    }

    /// Merges the extensions in order. A section whose target another extension adds is retried
    /// until nothing more can be placed, so the order of the extensions never decides whether a
    /// target is found.
    fn merge(&mut self, exts: &[(usize, ast::Object)]) {
        let mut pending: Vec<Op> = Vec::new();
        for (e, obj) in exts {
            for top in obj.sections() {
                let n = top.name().to_ascii_lowercase();
                if n != "layout" && n != "actions" {
                    continue;
                }
                for s in top.sections() {
                    pending.push(Op {
                        ext: *e,
                        in_actions: n == "actions",
                        section: s,
                    });
                }
            }
        }
        loop {
            let before = pending.len();
            let mut later = Vec::new();
            for op in pending {
                match self.apply(&op) {
                    Ok(true) => {}
                    Ok(false) => later.push(op),
                    Err(why) => self.reasons.push(why),
                }
            }
            pending = later;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        for op in pending {
            let kind = op.section.name().to_ascii_lowercase();
            let target = op.section.args().first().cloned().unwrap_or_default();
            self.reasons.push(format!(
                "page extension: {kind}({target}): `{target}` is not on the page"
            ));
        }
    }

    /// The tree in source order: areas in turn, each control before its children.
    fn flatten(self) -> Layout {
        let Tree { nodes, areas, reasons } = self;
        let mut slots: Vec<Option<Node>> = nodes.into_iter().map(Some).collect();
        let mut controls = Vec::new();
        fn emit(slots: &mut [Option<Node>], idx: usize, parent: Option<usize>, out: &mut Vec<RawControl>) {
            let Some(n) = slots[idx].take() else { return };
            let here = out.len();
            out.push(RawControl {
                name: n.name,
                kind: n.kind,
                parent,
                section: n.section,
                owner: n.owner,
                modifies: n.modifies,
            });
            for c in n.children {
                emit(slots, c, Some(here), out);
            }
        }
        for a in areas {
            for c in a.children {
                emit(&mut slots, c, None, &mut controls);
            }
        }
        Layout { controls, reasons }
    }
}

/// The right-hand side of one `SubPageLink` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkRhs {
    /// `field(Name)`
    Field(String),
    /// `const(x)` / `filter(x)`: the filter text; quotes of a single-quoted literal are removed.
    Filter(String),
    /// `const(Database::"Name")`: the id of the named table.
    TableId(String),
}

/// Splits `text` at top-level `sep`, ignoring separators inside parentheses and quotes.
pub fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let mut quote: Option<char> = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                out.push(text[start..i].trim().to_string());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(text[start..].trim().to_string());
    out.retain(|s| !s.is_empty());
    out
}

fn unquote_name(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(t)
        .to_string()
}

/// `'H1'|'H3'` → `H1|H3`: the AL string literals of a `filter(...)` or `const(...)` value
/// without their quotes (`''` is a quote inside a literal); text outside quotes is kept.
fn unquote_literals(inner: &str) -> String {
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
            continue;
        }
        while let Some(d) = chars.next() {
            if d == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push('\'');
                } else {
                    break;
                }
            } else {
                out.push(d);
            }
        }
    }
    out
}

/// `RunPageLink` has the shape of `SubPageLink`; the errors say which one it was.
pub fn parse_run_page_link(text: &str) -> Result<Vec<(String, LinkRhs)>, String> {
    parse_sub_page_link(text).map_err(|e| e.replace("SubPageLink", "RunPageLink"))
}

/// The value of a `RunObject` property: `Page "My Page"`, `Codeunit 80`, `Report Microsoft.Sales."X"`.
/// The object kind and the (possibly qualified) name or id, as parts.
pub fn parse_run_object(text: &str) -> Result<(al_ir::RunKind, Vec<String>), String> {
    let text = text.trim();
    let (word, rest) = text
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("RunObject `{text}`"))?;
    let kind = match word.to_ascii_lowercase().as_str() {
        "page" => al_ir::RunKind::Page,
        "codeunit" => al_ir::RunKind::Codeunit,
        "report" => al_ir::RunKind::Report,
        "xmlport" => al_ir::RunKind::XmlPort,
        "query" => al_ir::RunKind::Query,
        _ => return Err(format!("RunObject `{text}`")),
    };
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in rest.trim().chars() {
        match (quoted, c) {
            (_, '"') => quoted = !quoted,
            (false, '.') => parts.push(std::mem::take(&mut cur)),
            (_, c) => cur.push(c),
        }
    }
    parts.push(cur);
    if parts.iter().any(|p| p.trim().is_empty()) {
        return Err(format!("RunObject `{text}`"));
    }
    Ok((kind, parts.into_iter().map(|p| p.trim().to_string()).collect()))
}

/// Parses the value of a `SubPageLink` property: `A = field(B), C = const(x), D = filter(y | z)`.
/// `Err` names the part it does not understand.
pub fn parse_sub_page_link(text: &str) -> Result<Vec<(String, LinkRhs)>, String> {
    let mut out = Vec::new();
    for item in split_top(text, ',') {
        let eq = {
            // The first `=` outside quotes separates the part's field from the rhs.
            let mut quote: Option<char> = None;
            item.char_indices().find_map(|(i, c)| match (quote, c) {
                (Some(q), c) if c == q => {
                    quote = None;
                    None
                }
                (Some(_), _) => None,
                (None, '\'' | '"') => {
                    quote = Some(c);
                    None
                }
                (None, '=') => Some(i),
                _ => None,
            })
        };
        let Some(eq) = eq else {
            return Err(format!("SubPageLink entry `{item}`"));
        };
        let left = unquote_name(&item[..eq]);
        let rhs = item[eq + 1..].trim();
        let open = rhs.find('(').ok_or_else(|| format!("SubPageLink entry `{item}`"))?;
        if !rhs.ends_with(')') {
            return Err(format!("SubPageLink entry `{item}`"));
        }
        let kind = rhs[..open].trim().to_ascii_lowercase();
        let inner = rhs[open + 1..rhs.len() - 1].trim();
        let parsed = match kind.as_str() {
            "field" => LinkRhs::Field(unquote_name(inner)),
            "const" | "filter" => {
                if let Some(t) = inner
                    .strip_prefix("Database::")
                    .or_else(|| inner.strip_prefix("database::"))
                {
                    LinkRhs::TableId(unquote_name(t))
                } else {
                    LinkRhs::Filter(unquote_literals(inner))
                }
            }
            other => return Err(format!("SubPageLink with {other}(...)")),
        };
        out.push((left, parsed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routines(src: &str) -> Vec<String> {
        let parse = al_syntax::parse(src, &Default::default());
        let root = ast::SourceFile::cast(parse.syntax()).unwrap();
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        root.objects()
            .flat_map(|o| {
                o.syntax()
                    .descendants()
                    .filter_map(ast::Routine::cast)
                    .collect::<Vec<_>>()
            })
            .map(|r| r.name())
            .collect()
    }

    // Corpus handlers/TestPageControlPropertyLiveness_Tests.al: `Visible = not HideIt`
    // style properties are expressions over page state.
    #[test]
    fn expression_properties_become_synthetic_routines() {
        let src = "page 50100 P { layout { area(Content) { field(A; Rec.X) { Visible = not Hide; Editable = true; } field(B; Hide) { } field(C; Format(Rec.X)) { } } } var Hide: Boolean; }";
        let out = expand(src, &Default::default()).expect("a page with expressions");
        let names = routines(&out);
        let vis = src.find("Visible").unwrap();
        let c = src.find("field(C").unwrap();
        // Names count from the start of the object (here: 0).
        assert_eq!(names, vec![synthetic_name("visible", vis), synthetic_name("source", c)]);
        assert!(out.starts_with(&src[..src.len() - 1]), "original text is only added to");
    }

    #[test]
    fn literal_properties_and_other_objects_are_left_alone() {
        assert_eq!(
            expand(
                "page 50100 P { layout { area(Content) { field(A; Rec.X) { Visible = false; } } } }",
                &Default::default()
            ),
            None
        );
        assert_eq!(
            expand("codeunit 50100 C { procedure P() begin end; }", &Default::default()),
            None
        );
    }

    #[test]
    fn sources_classify_as_record_field_global_or_expression() {
        let globals: BTreeSet<String> = ["glob".to_string()].into();
        assert_eq!(classify_source("Rec.Num", &globals), Source::RecField("Num".into()));
        assert_eq!(
            classify_source("Rec.\"Entry No.\"", &globals),
            Source::RecField("Entry No.".into())
        );
        assert_eq!(classify_source("Glob", &globals), Source::Var("Glob".into()));
        assert_eq!(classify_source("Format(Rec.Num)", &globals), Source::Expr);
    }

    fn merged(base: &str, exts: &[&str]) -> Layout {
        let objects = |src: &str| {
            let parse = al_syntax::parse(src, &Default::default());
            assert!(parse.errors.is_empty(), "{:?}", parse.errors);
            ast::SourceFile::cast(parse.syntax())
                .unwrap()
                .objects()
                .collect::<Vec<_>>()
        };
        let base = objects(base).remove(0);
        let exts: Vec<(usize, ast::Object)> = exts
            .iter()
            .enumerate()
            .map(|(i, e)| (i + 1, objects(e).remove(0)))
            .collect();
        layout_with(&base, &exts)
    }

    fn names(l: &Layout) -> Vec<String> {
        l.controls
            .iter()
            .map(|c| match c.parent {
                Some(p) => format!("{}>{}", l.controls[p].name, c.name),
                None => c.name.clone(),
            })
            .collect()
    }

    // Corpus pageextensioncontrol/TestPageExtensionControlTrapped_Page.al (`addafter("Rec Text")`),
    // handlers/TestPageExtensionActionWithPart_Page.al (`addfirst(FactBoxes)`), handlers/TestPageExtensionActionInvoke_Page.al
    // (`addlast(Processing)`): extension controls land where the section names them.
    #[test]
    fn extension_sections_place_controls_first_last_after_and_before() {
        let base = "page 50100 P { layout { area(Content) { group(G) { field(A; Rec.A) { } field(B; Rec.B) { } } field(Z; Rec.Z) { } } area(FactBoxes) { } } actions { area(Processing) { action(Act) { } } } }";
        let ext = "pageextension 50101 E extends P {
            layout {
                addfirst(G) { field(First; Rec.F) { } }
                addafter(A) { field(AfterA; Rec.F) { } }
                addbefore(B) { field(BeforeB; Rec.F) { } }
                addlast(Content) { field(Last; Rec.F) { } }
                addfirst(FactBoxes) { part(Box; Other) { } }
            }
            actions { addlast(Processing) { action(ExtAct) { } } addafter(Act) { action(AfterAct) { } } }
        }";
        let l = merged(base, &[ext]);
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        assert_eq!(
            names(&l),
            [
                "G",
                "G>First",
                "G>A",
                "G>AfterA",
                "G>BeforeB",
                "G>B",
                "Z",
                "Last",
                "Box",
                "Act",
                "AfterAct",
                "ExtAct"
            ]
        );
        assert_eq!(l.controls.iter().filter(|c| c.owner == Some(1)).count(), 7);
    }

    // The order the extensions are given never decides whether a target is found.
    #[test]
    fn a_target_another_extension_adds_is_found_whatever_the_order() {
        let base = "page 50100 P { layout { area(Content) { field(A; Rec.A) { } } } }";
        let adds = "pageextension 50101 E1 extends P { layout { addlast(Content) { group(Extra) { } } } }";
        let into = "pageextension 50102 E2 extends P { layout { addlast(Extra) { field(Inner; Rec.F) { } } } }";
        for order in [[adds, into], [into, adds]] {
            let l = merged(base, &order);
            assert!(l.reasons.is_empty(), "{:?}", l.reasons);
            assert_eq!(names(&l), ["A", "Extra", "Extra>Inner"]);
        }
    }

    #[test]
    fn modify_blocks_attach_to_their_control_in_order_and_unknown_targets_are_reasons() {
        let base = "page 50100 P { layout { area(Content) { field(A; Rec.A) { } } } }";
        let e1 = "pageextension 50101 E1 extends P { layout { modify(A) { Caption = 'one'; } } }";
        let e2 = "pageextension 50102 E2 extends P { layout { modify(A) { Caption = 'two'; } addlast(Nowhere) { } movefirst(A; A) { } } }";
        let l = merged(base, &[e1, e2]);
        let a = &l.controls[0];
        assert_eq!(a.modifies.iter().map(|(e, _)| *e).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(l.reasons.len(), 2, "{:?}", l.reasons);
        assert!(l.reasons.iter().any(|r| r.contains("Nowhere")));
        assert!(l.reasons.iter().any(|r| r.contains("movefirst")));
    }

    // Corpus handlers/TestPagePromotedActionref_Page.al: an actionref is a control that names its target.
    #[test]
    fn an_actionref_is_a_control() {
        let base = "page 50100 P { actions { area(Processing) { action(Act) { } } area(Promoted) { actionref(Act_Promoted; Act) { } } } }";
        let l = merged(base, &[]);
        assert_eq!(names(&l), ["Act", "Act_Promoted"]);
        assert_eq!(l.controls[1].kind, al_ir::ControlKind::Action);
        assert!(l.controls[1].section.name().eq_ignore_ascii_case("actionref"));
        assert_eq!(l.controls[1].section.args(), ["Act_Promoted", "Act"]);
    }

    // Corpus pageextensioncontrol/TestPageExtensionModifyEditable_Tests.al: `Editable = not PXMSLocked;` in a
    // `modify` block of a page extension is an expression over the extension's variables.
    #[test]
    fn expression_properties_of_page_extensions_and_modify_blocks_become_routines_of_the_extension() {
        let src = "pageextension 50101 E extends P { layout { modify(A) { Editable = not Locked; } addlast(Content) { field(B; Own) { Visible = Show; } } } var Locked: Boolean; Own: Integer; Show: Boolean; }";
        let out = expand(src, &Default::default()).expect("an extension with expressions");
        let names = routines(&out);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| is_synthetic(n)), "{names:?}");
    }

    // Corpus handlers/TestPageSubpagePartConstFilter.al, TestPagePartLinkFilterGroup.al:
    // SubPageLink forms field(...), const(...), filter(...).
    #[test]
    fn sub_page_link_entries_parse() {
        let got = parse_sub_page_link(
            "\"Header No.\" = field(\"No.\"), Kind = const(Attachment), Status = filter(Open | Released), Cat = const('SPECIAL'), \"Table ID\" = const(Database::\"TSPL Header\")",
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                ("Header No.".to_string(), LinkRhs::Field("No.".into())),
                ("Kind".to_string(), LinkRhs::Filter("Attachment".into())),
                ("Status".to_string(), LinkRhs::Filter("Open | Released".into())),
                ("Cat".to_string(), LinkRhs::Filter("SPECIAL".into())),
                ("Table ID".to_string(), LinkRhs::TableId("TSPL Header".into())),
            ]
        );
        assert!(parse_sub_page_link("A = bogus(B)").is_err());
    }

    // Corpus handlers/TestPageActionRunPageLink.al:ActionRunPageLink_Filter_AppliesTheFilterExpression:
    // `filter('H1' | 'H3')` selects both values.
    #[test]
    fn quoted_literals_in_a_filter_lose_their_quotes_one_by_one() {
        let got = parse_run_page_link("\"Head No.\" = filter('H1' | 'H3'), B = const('it''s')").unwrap();
        assert_eq!(
            got,
            vec![
                ("Head No.".to_string(), LinkRhs::Filter("H1 | H3".into())),
                ("B".to_string(), LinkRhs::Filter("it's".into())),
            ]
        );
        assert!(
            parse_run_page_link("A = bogus(B)")
                .unwrap_err()
                .starts_with("RunPageLink")
        );
    }

    // Corpus handlers/TestPageActionRunObject_Objects.al: `RunObject = Page "TPARO Card Target"`.
    #[test]
    fn run_object_names_a_kind_and_an_object() {
        use al_ir::RunKind;
        assert_eq!(
            parse_run_object("Page \"TPARO Card Target\"").unwrap(),
            (RunKind::Page, vec!["TPARO Card Target".to_string()])
        );
        assert_eq!(
            parse_run_object("codeunit 80").unwrap(),
            (RunKind::Codeunit, vec!["80".to_string()])
        );
        assert_eq!(
            parse_run_object("Report Microsoft.Sales.\"Sales. Report\"").unwrap(),
            (
                RunKind::Report,
                vec!["Microsoft".into(), "Sales".into(), "Sales. Report".into()]
            )
        );
        assert!(parse_run_object("Table 18").is_err());
        assert!(parse_run_object("Page").is_err());
    }
}
