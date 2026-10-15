//! XmlPort objects (Format = Xml): the schema walk, the properties and triggers the runtime
//! understands, and the IR it is lowered to.
//!
//! The runtime runs only what the corpus pins. Every property, trigger or element kind it does
//! not understand becomes a reason on the xmlport, so a variable of that xmlport is refused
//! statically (hard rule 4) instead of running with a guessed behaviour.

use crate::members::FieldClass;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_ir as ir;
use al_syntax::ast;
use std::collections::HashMap;

/// Object properties that change nothing the runtime can observe.
const INERT_OBJECT_PROPERTIES: &[&str] = &[
    "caption",
    "captionml",
    "permissions",
    "transactiontype",
    "usedefaultnamespace",
];

/// A schema element before its names are resolved.
pub struct RawNode {
    pub name: String,
    pub xml_name: String,
    pub kind: ir::XmlNodeKind,
    pub parent: Option<usize>,
    pub section: ast::Section,
    pub args: Vec<String>,
}

pub struct Layout {
    pub nodes: Vec<RawNode>,
    pub direction: ir::XmlDirection,
    pub format: ir::XmlFormat,
    pub field_separator: Option<String>,
    pub field_delimiter: Option<String>,
    pub record_separator: Option<String>,
    pub utf8: bool,
    pub format_evaluate_xml: bool,
    pub reasons: Vec<String>,
    /// Reasons only running the port refuses (see `ir::XmlPort::run_unsupported`).
    pub run_reasons: Vec<String>,
}

fn bool_prop(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

/// The triggers each element kind may declare. Anything else is refused: only these are pinned
/// by the corpus (xmlport/TestXmlPortAdvanced.al).
fn allowed_triggers(kind: ir::XmlNodeKind) -> &'static [&'static str] {
    use ir::XmlNodeKind as K;
    match kind {
        K::Text | K::TextAttr => &["onbeforepassvariable", "onafterassignvariable"],
        K::Field | K::FieldAttr => &["onafterassignfield", "onbeforepassfield"],
        K::Table => &[
            "onafterinitrecord",
            "onbeforeinsertrecord",
            "onafterinsertrecord",
            "onbeforemodifyrecord",
            "onaftermodifyrecord",
        ],
    }
}

fn allowed_properties(kind: ir::XmlNodeKind) -> &'static [&'static str] {
    match kind {
        ir::XmlNodeKind::Table => &[
            "xmlname",
            "linktable",
            "linkfields",
            "autosave",
            "autoupdate",
            "autoreplace",
            "usetemporary",
        ],
        ir::XmlNodeKind::Field => &["xmlname", "minoccurs", "width"],
        _ => &["xmlname"],
    }
}

/// Walks the `schema` section of xmlport `obj` and checks the object's own properties.
pub fn layout(obj: &ast::Object) -> Layout {
    let mut out = Layout {
        nodes: Vec::new(),
        direction: ir::XmlDirection::Both,
        format: ir::XmlFormat::Xml,
        field_separator: None,
        field_delimiter: None,
        record_separator: None,
        utf8: false,
        format_evaluate_xml: false,
        reasons: Vec::new(),
        run_reasons: Vec::new(),
    };
    for p in obj.properties() {
        let (name, value) = (p.name().to_ascii_lowercase(), p.value_text());
        match name.as_str() {
            "direction" => match value.trim().to_ascii_lowercase().as_str() {
                "import" => out.direction = ir::XmlDirection::Import,
                "export" => out.direction = ir::XmlDirection::Export,
                "both" => out.direction = ir::XmlDirection::Both,
                other => out.reasons.push(format!("Direction = {other} is not supported")),
            },
            "format" => match value.trim().to_ascii_lowercase().as_str() {
                "xml" => {}
                "variabletext" => out.format = ir::XmlFormat::VariableText,
                "fixedtext" => out.format = ir::XmlFormat::FixedText,
                other => out.reasons.push(format!("Format = {other} is not supported")),
            },
            // UseRequestPage only matters for Run, which is not implemented: Export and Import
            // never open the request page.
            "userequestpage" => {}
            // xmlport/TestXmlPortTextFormatsContracts.al: VariableText and FixedText pin these; an Xml port's encoding is not pinned.
            "textencoding" if value.trim().eq_ignore_ascii_case("utf8") => out.utf8 = true,
            "fieldseparator" => out.field_separator = Some(crate::scan::unquote_xmlport(&value)),
            "fielddelimiter" => out.field_delimiter = Some(crate::scan::unquote_xmlport(&value)),
            "recordseparator" => out.record_separator = Some(crate::scan::unquote_xmlport(&value)),
            // xmlport/TestXmlPortFormatEvaluateContracts.al: Legacy is the display text, Xml the XML forms.
            "formatevaluate" => match value.trim().to_ascii_lowercase().as_str() {
                "legacy" => {}
                "xml" => out.format_evaluate_xml = true,
                other => out
                    .run_reasons
                    .push(format!("xmlport property formatevaluate = {other} is not pinned")),
            },
            n if INERT_OBJECT_PROPERTIES.contains(&n) => {}
            other => out
                .run_reasons
                .push(format!("xmlport property {other} is not supported yet")),
        }
    }
    for top in obj.sections() {
        match top.name().to_ascii_lowercase().as_str() {
            "schema" => visit(&top, None, &mut out),
            "requestpage" => out.run_reasons.push("request pages are not supported yet".into()),
            other => out
                .reasons
                .push(format!("xmlport section {other} is not supported yet")),
        }
    }
    out
}

fn visit(section: &ast::Section, parent: Option<usize>, out: &mut Layout) {
    use ir::XmlNodeKind as K;
    for s in section.sections() {
        let section_name = s.name().to_ascii_lowercase();
        let kind = match section_name.as_str() {
            "textelement" => K::Text,
            "textattribute" => K::TextAttr,
            "tableelement" => K::Table,
            "fieldelement" => K::Field,
            "fieldattribute" => K::FieldAttr,
            other => {
                out.reasons
                    .push(format!("xmlport schema element {other} is not supported yet"));
                continue;
            }
        };
        let args = s.args();
        let name = args.first().cloned().unwrap_or_default();
        let mut xml_name = name.clone();
        for p in s.properties() {
            let pn = p.name().to_ascii_lowercase();
            if !allowed_properties(kind).contains(&pn.as_str()) {
                // Refused when the port runs, not when it is declared: Base App's xmlport 1 sets
                // MinOccurs, MaxOccurs, Occurrence and SourceTableView, and tests reach it only
                // through a request page they cancel (handlers/TestReqPageBoundVisibility.al:172).
                out.run_reasons
                    .push(format!("property {pn} of {section_name} {name} is not supported yet"));
            } else if pn == "xmlname" {
                xml_name = crate::scan::unquote_xmlport(&p.value_text());
            }
        }
        for t in s.triggers() {
            let tn = t.name().to_ascii_lowercase();
            if !allowed_triggers(kind).contains(&tn.as_str()) {
                out.run_reasons
                    .push(format!("trigger {tn} of {section_name} {name} is not supported yet"));
            }
        }
        let mut args = args;
        if kind == K::Table {
            if let Some(table) = crate::types::section_object_name(&s, 1) {
                args[1] = table;
            }
        }
        let idx = out.nodes.len();
        out.nodes.push(RawNode {
            name,
            xml_name,
            kind,
            parent,
            section: s.clone(),
            args,
        });
        if matches!(kind, K::Text | K::Table) {
            visit(&s, Some(idx), out);
        }
    }
}

impl Layout {
    /// Whether the text element at `i` has any child node (a container carries no value).
    pub fn has_children(&self, i: usize) -> bool {
        self.nodes.iter().any(|n| n.parent == Some(i))
    }

    pub fn property(&self, i: usize, name: &str) -> Option<String> {
        self.nodes[i]
            .section
            .properties()
            .find(|p| p.name().eq_ignore_ascii_case(name))
            .map(|p| p.value_text())
    }

    /// The nearest table node named `name` at or above node `from`.
    fn table_named(&self, from: Option<usize>, name: &str) -> Option<usize> {
        let mut cur = from;
        while let Some(i) = cur {
            if self.nodes[i].kind == ir::XmlNodeKind::Table && self.nodes[i].name.eq_ignore_ascii_case(name) {
                return Some(i);
            }
            cur = self.nodes[i].parent;
        }
        None
    }
}

/// Everything `build` needs to know about the object being lowered.
pub struct Context<'a> {
    pub ws: &'a Workspace,
    pub members: &'a crate::members::MemberTable<'a>,
    pub oi: usize,
    pub file: usize,
    pub layout: &'a Layout,
    /// The object's global slots, by lower-case name.
    pub slots: &'a HashMap<String, usize>,
    /// (object, source offset) to routine index.
    pub routine_at: &'a crate::lower::RoutineAt,
    pub source_slot: usize,
    pub destination_slot: usize,
}

fn table_object(cx: &Context, name: &str) -> Option<usize> {
    match cx
        .ws
        .resolve_ref(cx.file, ObjKind::Table, &crate::types::qualified_parts(name))
    {
        Resolution::Found(t) => Some(t),
        _ => None,
    }
}

/// The IR of xmlport `cx.oi`. `reasons` are what the schema walk and the lowering of the
/// element variables already found.
pub fn build(cx: &Context, mut reasons: Vec<String>) -> ir::XmlPort {
    let layout = cx.layout;
    let mut run_reasons = layout.run_reasons.clone();
    let mut nodes: Vec<ir::XmlNode> = Vec::new();
    for (i, raw) in layout.nodes.iter().enumerate() {
        let sec = &raw.section;
        let kind = raw.kind;
        let flag = |name: &str, default: bool| {
            layout
                .property(i, name)
                .map_or(default, |v| bool_prop(&v).unwrap_or(default))
        };
        let triggers = sec
            .triggers()
            .filter_map(|t| {
                cx.routine_at
                    .get(&(cx.oi, usize::from(t.syntax().text_range().start())))
                    .map(|r| (t.name().to_ascii_lowercase(), r))
            })
            .collect();
        let mut node = ir::XmlNode {
            name: raw.name.clone(),
            xml_name: raw.xml_name.clone(),
            kind,
            parent: raw.parent,
            slot: None,
            table: None,
            table_node: None,
            field: None,
            link_node: None,
            link_fields: Vec::new(),
            auto_save: flag("AutoSave", true),
            auto_update: flag("AutoUpdate", false),
            auto_replace: flag("AutoReplace", false),
            use_temporary: flag("UseTemporary", false),
            min_zero: layout
                .property(i, "MinOccurs")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("zero")),
            width: layout.property(i, "Width").and_then(|v| v.trim().parse().ok()),
            triggers,
        };
        match kind {
            ir::XmlNodeKind::Text | ir::XmlNodeKind::TextAttr => {
                node.slot = cx.slots.get(&raw.name.to_lowercase()).copied();
                if node.slot.is_none() {
                    reasons.push(format!("variable of {} has no slot", raw.name));
                }
                // No corpus test pins a trigger on a container text element (one with child
                // elements; Base App's xmlport 1 has OnAfterAssignVariable on one), so running
                // the port is refused, not declaring it.
                if kind == ir::XmlNodeKind::Text && layout.has_children(i) && !node.triggers.is_empty() {
                    run_reasons.push(format!(
                        "triggers of the container text element {} are not supported (not pinned)",
                        raw.name
                    ));
                }
            }
            ir::XmlNodeKind::Table => {
                node.slot = cx.slots.get(&raw.name.to_lowercase()).copied();
                node.table = raw.args.get(1).and_then(|t| table_object(cx, t));
                if node.slot.is_none() || node.table.is_none() {
                    reasons.push(format!(
                        "table element {}: table {} is outside the workspace",
                        raw.name,
                        raw.args.get(1).cloned().unwrap_or_default()
                    ));
                }
                if !node.auto_save && node.use_temporary {
                    // A temporary buffer that is not saved never holds a row the triggers can read.
                    reasons.push(format!(
                        "table element {}: UseTemporary with AutoSave = false",
                        raw.name
                    ));
                }
                let link_table = layout
                    .property(i, "LinkTable")
                    .map(|v| crate::scan::unquote_xmlport(&v));
                let link_fields = layout.property(i, "LinkFields");
                match (link_table, link_fields) {
                    (None, None) => {}
                    (Some(t), Some(f)) => match layout.table_named(raw.parent, &t) {
                        Some(ln) => {
                            node.link_node = Some(ln);
                            for pair in crate::scan::split_double(&f, ',') {
                                let parts = crate::scan::split_double(&pair, '=');
                                let own = parts.first().map(|p| crate::scan::unquote_xmlport(p));
                                let other = parts.get(1).map(|p| p.trim()).and_then(|p| {
                                    let low = p.to_ascii_lowercase();
                                    (low.starts_with("field(") && p.ends_with(')'))
                                        .then(|| crate::scan::unquote_xmlport(&p[6..p.len() - 1]))
                                });
                                let own_pos = own.as_ref().and_then(|n| {
                                    node.table.and_then(|t| {
                                        cx.members[t].fields.iter().position(|x| x.name.eq_ignore_ascii_case(n))
                                    })
                                });
                                let other_pos = other.as_ref().and_then(|n| {
                                    nodes.get(ln).and_then(|l| l.table).and_then(|t| {
                                        cx.members[t].fields.iter().position(|x| x.name.eq_ignore_ascii_case(n))
                                    })
                                });
                                match (own_pos, other_pos) {
                                    (Some(a), Some(b)) => node.link_fields.push((a, b)),
                                    _ => reasons.push(format!(
                                        "table element {}: LinkFields entry `{}` is not understood",
                                        raw.name,
                                        pair.trim()
                                    )),
                                }
                            }
                        }
                        None => reasons.push(format!(
                            "table element {}: LinkTable {t} is not a table element above it",
                            raw.name
                        )),
                    },
                    _ => reasons.push(format!(
                        "table element {}: LinkTable and LinkFields must be given together",
                        raw.name
                    )),
                }
            }
            ir::XmlNodeKind::Field | ir::XmlNodeKind::FieldAttr => {
                let source = raw.args.get(1).cloned().unwrap_or_default();
                let (table_name, field_name) = match source.split_once('.') {
                    Some((t, f)) => (crate::scan::unquote_xmlport(t), crate::scan::unquote_xmlport(f)),
                    None => (String::new(), crate::scan::unquote_xmlport(&source)),
                };
                match layout.table_named(raw.parent, &table_name) {
                    Some(tn) => {
                        node.table_node = Some(tn);
                        let pos = nodes[tn].table.and_then(|t| {
                            cx.members[t]
                                .fields
                                .iter()
                                .position(|x| x.name.eq_ignore_ascii_case(&field_name))
                        });
                        match pos {
                            Some(p) => {
                                let t = nodes[tn].table.expect("a position implies a table");
                                if cx.members[t].fields[p].class != FieldClass::Normal {
                                    reasons.push(format!("{} is bound to a FlowField or FlowFilter", raw.name));
                                }
                                node.field = Some(p);
                            }
                            None => reasons.push(format!(
                                "{} is bound to unknown field {field_name} of {table_name}",
                                raw.name
                            )),
                        }
                    }
                    None => reasons.push(format!("{} is not inside a table element named {table_name}", raw.name)),
                }
            }
        }
        nodes.push(node);
    }
    reasons.extend(layout.reasons.iter().cloned());
    ir::XmlPort {
        direction: layout.direction,
        format: layout.format,
        field_separator: layout.field_separator.clone(),
        field_delimiter: layout.field_delimiter.clone(),
        record_separator: layout.record_separator.clone(),
        utf8: layout.utf8,
        format_evaluate_xml: layout.format_evaluate_xml,
        nodes,
        source_slot: cx.source_slot,
        destination_slot: cx.destination_slot,
        unsupported: reasons,
        run_unsupported: run_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_of(src: &str) -> Layout {
        let parse = al_syntax::parse(src, &Default::default());
        let root = ast::SourceFile::cast(parse.syntax()).unwrap();
        layout(&root.objects().next().unwrap())
    }

    #[test]
    fn the_schema_is_walked_in_source_order_with_parents() {
        let l = layout_of(
            "xmlport 1 X { Direction = Export; schema { textelement(Root) { XmlName = 'Docs'; tableelement(T; Cust) { fieldelement(A; T.X) { } fieldattribute(B; T.Y) { } } } } }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        assert_eq!(l.direction, ir::XmlDirection::Export);
        let names: Vec<_> = l.nodes.iter().map(|n| (n.xml_name.as_str(), n.parent)).collect();
        assert_eq!(
            names,
            vec![("Docs", None), ("T", Some(0)), ("A", Some(1)), ("B", Some(1))]
        );
    }

    #[test]
    fn what_the_corpus_does_not_pin_becomes_a_reason() {
        let l = layout_of(
            "xmlport 1 X { Format = Binary; schema { textelement(Root) { MinOccurs = Zero; trigger OnPreXmlItem() begin end; } } requestpage { } }",
        );
        let all = [l.reasons.clone(), l.run_reasons.clone()].concat().join("; ");
        assert!(all.contains("Format = binary"), "{all}");
        assert!(all.contains("property minoccurs"), "{all}");
        assert!(all.contains("trigger onprexmlitem"), "{all}");
        assert!(all.contains("request pages"), "{all}");
        assert!(
            !l.reasons.join(";").contains("request pages"),
            "a request page is a run reason"
        );
    }

    // Mutation-testing additions (docs/status.md debt).

    #[test]
    fn only_utf8_is_an_accepted_text_encoding() {
        for ok in ["UTF8", "utf8", "UTF8 "] {
            let l = layout_of(&format!(
                "xmlport 1 X {{ TextEncoding = {ok}; schema {{ textelement(Root) {{ }} }} }}"
            ));
            assert!(l.reasons.is_empty(), "{ok}: {:?}", l.reasons);
        }
        let l = layout_of("xmlport 1 X { TextEncoding = UTF16; schema { textelement(Root) { } } }");
        assert!(
            l.run_reasons.join(";").contains("property textencoding"),
            "{:?}",
            l.run_reasons
        );
    }

    #[test]
    fn inert_object_properties_are_accepted_and_others_are_reasons() {
        for name in INERT_OBJECT_PROPERTIES {
            let l = layout_of(&format!(
                "xmlport 1 X {{ {name} = 'x'; schema {{ textelement(Root) {{ }} }} }}"
            ));
            assert!(l.reasons.is_empty(), "{name}: {:?}", l.reasons);
        }
        let l = layout_of("xmlport 1 X { Encoding = Windows; schema { textelement(Root) { } } }");
        assert!(
            l.run_reasons.join(";").contains("property encoding"),
            "{:?}",
            l.run_reasons
        );
    }

    #[test]
    fn a_quote_hides_parentheses_from_the_nesting_count() {
        assert_eq!(
            crate::scan::split_double("\"A(B\" = FIELD(X), C = FIELD(Y)", ','),
            vec!["\"A(B\" = FIELD(X)", " C = FIELD(Y)"]
        );
        assert_eq!(
            crate::scan::split_double("\"A)B\" = FIELD(X), C = FIELD(Y)", ','),
            vec!["\"A)B\" = FIELD(X)", " C = FIELD(Y)"]
        );
        assert_eq!(
            crate::scan::split_double("A = FIELD(X, Y), B", ','),
            vec!["A = FIELD(X, Y)", " B"]
        );
    }

    #[test]
    fn link_fields_split_at_top_level_commas() {
        assert_eq!(
            crate::scan::split_double("\"A,B\" = FIELD(X), C = FIELD(Y)", ','),
            vec!["\"A,B\" = FIELD(X)", " C = FIELD(Y)"]
        );
    }
}

#[cfg(test)]
mod lowering_tests {
    use crate::lower::compile;
    use crate::workspace::AppSource;
    use al_ir as ir;

    const SRC: &str = r#"
table 50001 Parent { fields { field(1; No; Integer) { } field(2; Name; Text[30]) { } } keys { key(PK; No) { } } }
table 50002 Child { fields { field(1; No; Integer) { } field(2; ParentNo; Integer) { } } keys { key(PK; No) { } } }
xmlport 50010 X
{
    Direction = Export; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Root)
        {
            XmlName = 'Docs';
            tableelement(P; Parent)
            {
                XmlName = 'Doc';
                textattribute(Note) { trigger OnBeforePassVariable() begin Note := 'n'; end; }
                fieldelement(Number; P.No) { }
                tableelement(C; Child)
                {
                    LinkTable = P;
                    LinkFields = ParentNo = FIELD(No);
                    fieldattribute(Id; C.No) { }
                }
            }
        }
    }
    var Counter: Integer;
    procedure Bump() begin Counter += 1; end;
}
"#;

    fn lowered() -> ir::Object {
        let (ws, l) = compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), SRC.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        l.program.objects.iter().find(|o| o.kind == "xmlport").cloned().unwrap()
    }

    // The element variables take the first global slots, then the two hidden streams, then the
    // declared variables; the nodes point at those slots, at fields by position and at links.
    #[test]
    fn schema_nodes_point_at_slots_fields_and_links() {
        let o = lowered();
        let names: Vec<_> = o.globals.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["Root", "P", "Note", "C", "__source", "__destination", "Counter"]
        );
        let xp = o.xmlport.as_ref().unwrap();
        assert!(xp.unsupported.is_empty(), "{:?}", xp.unsupported);
        assert_eq!((xp.source_slot, xp.destination_slot), (4, 5));
        let kinds: Vec<_> = xp
            .nodes
            .iter()
            .map(|n| (n.xml_name.as_str(), n.kind, n.parent))
            .collect();
        use ir::XmlNodeKind::*;
        assert_eq!(
            kinds,
            vec![
                ("Docs", Text, None),
                ("Doc", Table, Some(0)),
                ("Note", TextAttr, Some(1)),
                ("Number", Field, Some(1)),
                ("C", Table, Some(1)),
                ("Id", FieldAttr, Some(4)),
            ]
        );
        let (p, field, child) = (&xp.nodes[1], &xp.nodes[3], &xp.nodes[4]);
        assert_eq!((p.slot, field.table_node, field.field), (Some(1), Some(1), Some(0)));
        assert_eq!((child.link_node, child.link_fields.clone()), (Some(1), vec![(1, 0)]));
        assert!(child.auto_save && !child.auto_update && !child.auto_replace && !child.use_temporary);
        assert!(xp.nodes[2].trigger("onbeforepassvariable").is_some());
        assert_eq!(xp.direction, ir::XmlDirection::Export);
    }

    #[test]
    fn an_unknown_field_or_table_is_a_reason() {
        let bad = SRC.replace("fieldelement(Number; P.No)", "fieldelement(Number; P.Nope)");
        let (_, l) = compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), bad)],
            }],
            &Default::default(),
        );
        let o = l.program.objects.iter().find(|o| o.kind == "xmlport").unwrap();
        let why = o.xmlport.as_ref().unwrap().unsupported.join(";");
        assert!(why.contains("unknown field Nope"), "{why}");
    }
    // Mutation-testing additions (docs/status.md debt).

    fn reasons_of(src: String) -> String {
        let (_, l) = compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src)],
            }],
            &Default::default(),
        );
        let o = l.program.objects.iter().find(|o| o.kind == "xmlport").unwrap();
        o.xmlport.as_ref().unwrap().unsupported.join(";")
    }

    fn run_reasons_of(src: String) -> String {
        let (_, l) = compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src)],
            }],
            &Default::default(),
        );
        let o = l.program.objects.iter().find(|o| o.kind == "xmlport").unwrap();
        o.xmlport.as_ref().unwrap().run_unsupported.join(";")
    }

    #[test]
    fn a_field_must_sit_inside_a_table_element_of_the_name_it_binds() {
        // Root is a text element, Q is no element at all, and P is a table element of another name.
        for name in ["Root", "Q"] {
            let why = reasons_of(SRC.replace(
                "fieldelement(Number; P.No)",
                &format!("fieldelement(Number; {name}.No)"),
            ));
            assert!(
                why.contains(&format!("is not inside a table element named {name}")),
                "{name}: {why}"
            );
        }
        assert_eq!(reasons_of(SRC.to_string()), "");
    }

    #[test]
    fn link_table_must_name_a_table_element_above() {
        for name in ["Root", "Q", "C"] {
            let why = reasons_of(SRC.replace("LinkTable = P;", &format!("LinkTable = {name};")));
            assert!(
                why.contains(&format!("LinkTable {name} is not a table element above it")),
                "{name}: {why}"
            );
        }
        let why = reasons_of(SRC.replace("LinkFields = ParentNo = FIELD(No);", ""));
        assert!(why.contains("LinkTable and LinkFields must be given together"), "{why}");
    }

    #[test]
    fn a_container_text_element_with_a_trigger_is_not_a_reason_the_run_refuses_it() {
        // The runtime refuses it when the port runs (al-vm/tests/xmlports.rs), so code that only
        // holds the port stays runnable.
        let why = reasons_of(SRC.replace(
            "XmlName = 'Docs';",
            "XmlName = 'Docs'; trigger OnBeforePassVariable() begin end;",
        ));
        assert!(!why.contains("container text element"), "{why}");
        let run = run_reasons_of(SRC.replace(
            "XmlName = 'Docs';",
            "XmlName = 'Docs'; trigger OnBeforePassVariable() begin end;",
        ));
        assert!(run.contains("triggers of the container text element Root"), "{run}");
    }

    #[test]
    fn format_evaluate_xml_and_legacy_are_understood_and_others_are_run_reasons() {
        // xmlport/TestXmlPortFormatEvaluateContracts.al
        for v in ["Xml", "Legacy"] {
            let src = SRC.replace(
                "Direction = Export;",
                &format!("FormatEvaluate = {v}; Direction = Export;"),
            );
            assert!(!reasons_of(src.clone()).contains("formatevaluate"));
            assert!(!run_reasons_of(src).contains("formatevaluate"), "{v}");
        }
        let src = SRC.replace("Direction = Export;", "FormatEvaluate = Other; Direction = Export;");
        assert!(run_reasons_of(src).contains("formatevaluate = other"));
    }

    #[test]
    fn a_table_outside_the_workspace_is_a_reason_even_with_its_variable_in_place() {
        let why = reasons_of(SRC.replace("tableelement(P; Parent)", "tableelement(P; Nowhere)"));
        assert!(
            why.contains("table element P: table Nowhere is outside the workspace"),
            "{why}"
        );
    }

    #[test]
    fn a_link_field_that_is_not_field_of_something_is_not_understood() {
        let why = reasons_of(SRC.replace("LinkFields = ParentNo = FIELD(No);", "LinkFields = ParentNo = No;"));
        assert!(
            why.contains("LinkFields entry `ParentNo = No` is not understood"),
            "{why}"
        );
        // Only a FIELD( prefix makes the right-hand side a field reference; a name that merely
        // ends in a parenthesis and has a real field after six characters is not one.
        let why = reasons_of(SRC.replace(
            "LinkFields = ParentNo = FIELD(No);",
            "LinkFields = ParentNo = ABCDEFNo);",
        ));
        assert!(why.contains("is not understood"), "{why}");
        // The same for a FIELD( that is never closed: its sixth character on is not a field name.
        let why = reasons_of(SRC.replace(
            "LinkFields = ParentNo = FIELD(No);",
            "LinkFields = ParentNo = FIELD(NoX;",
        ));
        assert!(why.contains("is not understood"), "{why}");
    }
}
