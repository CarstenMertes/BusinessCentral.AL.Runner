//! Report objects: the dataset walk, the properties and triggers the runtime understands, and the
//! IR it is lowered to.
//!
//! The runtime runs only processing: no layout, no request page, no output. Every property,
//! trigger or section it does not understand becomes a reason on the report, so a variable of
//! that report is refused statically (hard rule 4) instead of running with a guessed behaviour.

use crate::members::Members;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_ir as ir;
use al_syntax::ast;
use std::collections::HashMap;

/// Report properties that change nothing a processing run can observe.
const INERT_OBJECT_PROPERTIES: &[&str] = &[
    "caption",
    "captionml",
    "usagecategory",
    "applicationarea",
    "permissions",
    "tooltip",
    "tooltipml",
    "description",
];

/// Data item properties that change nothing a processing run can observe: they steer the request
/// page and the layout, which a run without both never reaches.
const INERT_ITEM_PROPERTIES: &[&str] = &[
    "requestfilterfields",
    "requestfilterheading",
    "requestfilterheadingml",
    "printonlyifdetail",
    "newpagepergroup",
    "newpageperrecord",
];

/// Sections that only matter to a layout or a request page.
const LAYOUT_SECTIONS: &[&str] = &["rendering", "labels", "wordlayout", "rdlclayout", "excellayout"];

/// A data item before its names are resolved.
pub struct RawItem {
    pub name: String,
    pub table: String,
    pub parent: Option<usize>,
    pub section: ast::Section,
}

pub struct Layout {
    pub items: Vec<RawItem>,
    pub processing_only: bool,
    pub use_request_page: Option<bool>,
    pub has_request_page: bool,
    pub transaction_type: Option<String>,
    pub reasons: Vec<String>,
}

fn bool_prop(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if let Some(inner) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        return inner.to_string();
    }
    t.to_string()
}

/// Walks the `dataset` section of report `obj` and checks the object's own properties.
pub fn layout(obj: &ast::Object) -> Layout {
    let mut out = Layout {
        items: Vec::new(),
        processing_only: false,
        use_request_page: None,
        has_request_page: false,
        transaction_type: None,
        reasons: Vec::new(),
    };
    for p in obj.properties() {
        let (name, value) = (p.name().to_ascii_lowercase(), p.value_text());
        match name.as_str() {
            "processingonly" => match bool_prop(&value) {
                Some(b) => out.processing_only = b,
                None => out
                    .reasons
                    .push(format!("ProcessingOnly = {} is not understood", value.trim())),
            },
            "userequestpage" => match bool_prop(&value) {
                Some(b) => out.use_request_page = Some(b),
                None => out
                    .reasons
                    .push(format!("UseRequestPage = {} is not understood", value.trim())),
            },
            "transactiontype" => out.transaction_type = Some(value.trim().to_ascii_lowercase()),
            n if INERT_OBJECT_PROPERTIES.contains(&n) => {}
            "defaultlayout" | "defaultrenderinglayout" | "rdlclayout" | "wordlayout" | "excellayout" => out
                .reasons
                .push(format!("report property {name} (a layout) is not supported yet")),
            other => out
                .reasons
                .push(format!("report property {other} is not supported yet")),
        }
    }
    for t in obj.triggers() {
        let n = t.name().to_ascii_lowercase();
        if !["oninitreport", "onprereport", "onpostreport"].contains(&n.as_str()) {
            out.reasons.push(format!("report trigger {n} is not supported yet"));
        }
    }
    for top in obj.sections() {
        match top.name().to_ascii_lowercase().as_str() {
            "dataset" => visit(&top, None, &mut out),
            "requestpage" => out.has_request_page = true,
            n if LAYOUT_SECTIONS.contains(&n) => out
                .reasons
                .push(format!("report section {n} (a layout) is not supported yet")),
            other => out.reasons.push(format!("report section {other} is not supported yet")),
        }
    }
    if !out.processing_only {
        out.reasons
            .push("the report is not ProcessingOnly, so running it renders output".into());
    }
    out
}

fn visit(section: &ast::Section, parent: Option<usize>, out: &mut Layout) {
    for s in section.sections() {
        let section_name = s.name().to_ascii_lowercase();
        match section_name.as_str() {
            "dataitem" => {}
            // Columns only feed the layout and the dataset a SaveAs writes.
            "column" if parent.is_some() => continue,
            other => {
                out.reasons
                    .push(format!("dataset element {other} is not supported yet"));
                continue;
            }
        }
        let args = s.args();
        let name = args.first().cloned().unwrap_or_default();
        let table = args.get(1).cloned().unwrap_or_default();
        for p in s.properties() {
            let pn = p.name().to_ascii_lowercase();
            if !["dataitemtableview", "dataitemlink"].contains(&pn.as_str())
                && !INERT_ITEM_PROPERTIES.contains(&pn.as_str())
            {
                out.reasons
                    .push(format!("property {pn} of dataitem {name} is not supported yet"));
            }
        }
        for t in s.triggers() {
            let tn = t.name().to_ascii_lowercase();
            if !["onpredataitem", "onaftergetrecord", "onpostdataitem"].contains(&tn.as_str()) {
                out.reasons
                    .push(format!("trigger {tn} of dataitem {name} is not supported yet"));
            }
        }
        if out.items.iter().any(|i| i.name.eq_ignore_ascii_case(&name)) {
            out.reasons.push(format!("dataitem {name} is declared twice"));
        }
        let idx = out.items.len();
        out.items.push(RawItem {
            name,
            table,
            parent,
            section: s.clone(),
        });
        visit(&s, Some(idx), out);
    }
}

fn split_top(text: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut depth) = (Vec::new(), String::new(), false, 0);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            c if c == sep && !quoted && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// Everything `build` needs to know about the report being lowered.
pub struct Context<'a> {
    pub ws: &'a Workspace,
    pub members: &'a [Members],
    pub file: usize,
    pub oi: usize,
    pub layout: &'a Layout,
    /// The global slot of each data item's record, by lower-case name.
    pub slots: &'a HashMap<String, usize>,
    pub request_flag_slot: usize,
    /// (object, source offset) to routine index.
    pub routine_at: &'a HashMap<(usize, usize), usize>,
    /// Reportextensions that change what a run does: why the report cannot run yet.
    pub extension_reasons: Vec<String>,
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

/// The IR of report `cx.oi`. `reasons` are what the lowering of the item variables found.
pub fn build(cx: &Context, obj: &ast::Object, mut reasons: Vec<String>) -> ir::Report {
    let layout = cx.layout;
    let trigger = |sec: &ast::Section, name: &str| {
        sec.triggers()
            .find(|t| t.name().eq_ignore_ascii_case(name))
            .and_then(|t| {
                cx.routine_at
                    .get(&(cx.oi, usize::from(t.syntax().text_range().start())))
            })
            .copied()
    };
    let mut items: Vec<ir::ReportItem> = Vec::new();
    for raw in &layout.items {
        let sec = &raw.section;
        let table = table_object(cx, &raw.table);
        let slot = cx.slots.get(&raw.name.to_lowercase()).copied();
        if slot.is_none() || table.is_none() {
            reasons.push(format!(
                "dataitem {}: table {} is outside the workspace",
                raw.name, raw.table
            ));
        }
        let prop = |n: &str| {
            sec.properties()
                .find(|p| p.name().eq_ignore_ascii_case(n))
                .map(|p| p.value_text())
        };
        let mut link = Vec::new();
        if let Some(text) = prop("DataItemLink") {
            match raw.parent.and_then(|p| items.get(p).and_then(|i| i.table)).zip(table) {
                None => reasons.push(format!(
                    "dataitem {}: DataItemLink needs an enclosing data item",
                    raw.name
                )),
                Some((parent_table, own_table)) => {
                    for pair in split_top(&text, ',') {
                        let parts = split_top(&pair, '=');
                        let own = parts.first().map(|p| unquote(p));
                        let other = parts.get(1).map(|p| p.trim()).and_then(|p| {
                            let low = p.to_ascii_lowercase();
                            (low.starts_with("field(") && p.ends_with(')')).then(|| unquote(&p[6..p.len() - 1]))
                        });
                        let own_pos = own.as_ref().and_then(|n| {
                            cx.members[own_table]
                                .fields
                                .iter()
                                .position(|x| x.name.eq_ignore_ascii_case(n))
                        });
                        let other_pos = other.as_ref().and_then(|n| {
                            cx.members[parent_table]
                                .fields
                                .iter()
                                .position(|x| x.name.eq_ignore_ascii_case(n))
                        });
                        match (own_pos, other_pos) {
                            (Some(a), Some(b)) => link.push((a, b)),
                            _ => reasons.push(format!(
                                "dataitem {}: DataItemLink entry `{}` is not understood",
                                raw.name,
                                pair.trim()
                            )),
                        }
                    }
                }
            }
        }
        items.push(ir::ReportItem {
            name: raw.name.clone(),
            parent: raw.parent,
            slot,
            table,
            view: prop("DataItemTableView"),
            link,
            on_pre: trigger(sec, "OnPreDataItem"),
            on_after_get: trigger(sec, "OnAfterGetRecord"),
            on_post: trigger(sec, "OnPostDataItem"),
        });
    }
    reasons.extend(layout.reasons.iter().cloned());
    reasons.extend(cx.extension_reasons.iter().cloned());
    let report_trigger = |name: &str| {
        obj.triggers()
            .find(|t| t.name().eq_ignore_ascii_case(name))
            .and_then(|t| {
                cx.routine_at
                    .get(&(cx.oi, usize::from(t.syntax().text_range().start())))
            })
            .copied()
    };
    ir::Report {
        processing_only: layout.processing_only,
        use_request_page: layout.use_request_page,
        has_request_page: layout.has_request_page,
        transaction_type: layout.transaction_type.clone(),
        items,
        on_init: report_trigger("OnInitReport"),
        on_pre: report_trigger("OnPreReport"),
        on_post: report_trigger("OnPostReport"),
        request_flag_slot: cx.request_flag_slot,
        unsupported: reasons,
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
    fn the_dataset_is_walked_in_source_order_with_parents() {
        let l = layout_of(
            "report 1 R { ProcessingOnly = true; UseRequestPage = false; dataset { dataitem(A; Cust) { column(X; No) { } dataitem(B; Ledger) { DataItemLink = No = field(No); } } dataitem(C; Cust) { } } }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        let names: Vec<_> = l.items.iter().map(|i| (i.name.as_str(), i.parent)).collect();
        assert_eq!(names, vec![("A", None), ("B", Some(0)), ("C", None)]);
        assert!(l.processing_only);
        assert_eq!(l.use_request_page, Some(false));
    }

    #[test]
    fn what_the_corpus_does_not_pin_becomes_a_reason() {
        let l = layout_of(
            "report 1 R { ProcessingOnly = true; DefaultLayout = Word; dataset { dataitem(A; Cust) { CalcFields = Amount; trigger OnFoo() begin end; } } rendering { } }",
        );
        let all = l.reasons.join("; ");
        assert!(all.contains("defaultlayout"), "{all}");
        assert!(all.contains("property calcfields"), "{all}");
        assert!(all.contains("trigger onfoo"), "{all}");
        assert!(all.contains("section rendering"), "{all}");
    }

    #[test]
    fn a_report_that_is_not_processing_only_renders_output() {
        let l = layout_of("report 1 R { dataset { dataitem(A; Cust) { } } }");
        assert!(l.reasons.join(";").contains("ProcessingOnly"), "{:?}", l.reasons);
    }

    #[test]
    fn the_request_page_is_noted_not_walked() {
        let l = layout_of("report 1 R { ProcessingOnly = true; requestpage { layout { } } }");
        assert!(l.has_request_page);
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }
}
