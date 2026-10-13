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
const CONTROL_SECTIONS: &[&str] = &["field", "group", "repeater", "part", "action", "systemaction"];

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
    let parse = al_syntax::parse(src, symbols);
    let root = ast::SourceFile::cast(parse.syntax())?;
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for obj in root.objects() {
        if !obj.kind().eq_ignore_ascii_case("page") {
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
    for (at, text) in inserts.into_iter().rev() {
        out.insert_str(at, &text);
    }
    Some(out)
}

/// A control found in the page's source, before bindings and routines are resolved.
pub struct RawControl {
    pub name: String,
    pub kind: al_ir::ControlKind,
    pub parent: Option<usize>,
    pub section: ast::Section,
}

/// The control tree of a page, from syntax alone, and why the client cannot
/// drive it yet.
pub struct Layout {
    pub controls: Vec<RawControl>,
    pub reasons: Vec<String>,
}

/// Walks the `layout` and `actions` sections of page object `obj`.
pub fn layout(obj: &ast::Object) -> Layout {
    let mut out = Layout {
        controls: Vec::new(),
        reasons: Vec::new(),
    };
    for top in obj.sections() {
        let n = top.name().to_ascii_lowercase();
        if n == "layout" || n == "actions" {
            visit(&top, None, n == "actions", &mut out);
        }
    }
    out
}

fn visit(section: &ast::Section, parent: Option<usize>, in_actions: bool, out: &mut Layout) {
    use al_ir::ControlKind as K;
    for s in section.sections() {
        let name = s.name().to_ascii_lowercase();
        let arg0 = s.args().first().cloned().unwrap_or_default();
        let kind = match name.as_str() {
            "area" => {
                visit(&s, parent, in_actions, out);
                continue;
            }
            "field" => K::Field,
            "group" if in_actions => K::ActionGroup,
            "group" => K::Group,
            "repeater" => K::Repeater,
            "action" | "actionref" => K::Action,
            "systemaction" => K::SystemAction,
            "part" => K::Part,
            // A separator carries no behaviour. An actionref is a control that delegates to its
            // action (corpus handlers/TestPageActionRunObject_Tests.al:ActionrefToARunObjectActionOpensTheSameTarget).
            "separator" => continue,
            other => {
                out.reasons
                    .push(format!("page control kind `{other}` is not supported yet"));
                continue;
            }
        };
        let idx = out.controls.len();
        out.controls.push(RawControl {
            name: arg0,
            kind: kind.clone(),
            parent,
            section: s.clone(),
        });
        if matches!(kind, K::Group | K::Repeater | K::ActionGroup) {
            visit(&s, Some(idx), in_actions, out);
        }
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
