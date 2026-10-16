//! Report objects: the dataset walk, the properties and triggers the runtime understands, and the
//! IR it is lowered to.
//!
//! The runtime runs only processing: no layout, no request page, no output. Every property,
//! trigger or section it does not understand becomes a reason on the report, so a variable of
//! that report is refused statically (hard rule 4) instead of running with a guessed behaviour.

use crate::check::posmap::FxMap as HashMap;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_ir as ir;
use al_syntax::ast;

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
    // Feeds the client's Tell Me search only (docs/owner-review.md, 2026-10-14).
    "additionalsearchterms",
    "additionalsearchtermsml",
    // Where the platform reads from (a replica); nothing a run or its dataset shows.
    "dataaccessintent",
    // Whether the client shows the report, from a permission on a table. Permissions are not modelled
    // and no AL code reads it (docs/owner-review.md, issue 28).
    "accessbypermission",
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

/// Column properties that steer how a layout shows a number. The Base Application states
/// `AutoFormatType` on most decimal columns, so a dataset that refused them would refuse nearly every
/// report; the dataset is taken to carry the same `decimalformatter` for them
/// (docs/probes-needed.md, "Report.SaveAs dataset").
const FORMAT_ONLY_COLUMN_PROPERTIES: &[&str] = &[
    "autoformattype",
    "autoformatexpression",
    "decimalplaces",
    "includecaption",
];

/// Whether the data item `sec` states `UseTemporary = true`: its record is a temporary one, which
/// holds only what the report's code inserts.
pub fn uses_temporary(sec: &ast::Section) -> bool {
    sec.properties()
        .any(|p| p.name().eq_ignore_ascii_case("UseTemporary") && p.value_text().trim().eq_ignore_ascii_case("true"))
}

/// A data item before its names are resolved.
pub struct RawItem {
    pub name: String,
    pub table: String,
    pub parent: Option<usize>,
    /// For a top-level item of a reportextension: the name of the item it is added under.
    pub target: Option<String>,
    /// For a top-level item of a reportextension: `addfirst`, `addlast`, `addbefore` or `addafter`
    /// (`target` then names the sibling for the last two).
    pub place: Place,
    pub section: ast::Section,
}

/// Where a reportextension puts the data items one `add...` block declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Place {
    #[default]
    Last,
    First,
    Before,
    After,
}

pub struct Layout {
    pub items: Vec<RawItem>,
    pub processing_only: bool,
    pub use_request_page: Option<bool>,
    pub has_request_page: bool,
    pub transaction_type: Option<String>,
    pub reasons: Vec<String>,
    /// Why running the report renders output: the layouts it declares, and `ProcessingOnly` not
    /// being true. A run refuses these after its request page, not before.
    pub output: Vec<String>,
    /// The `Caption` property when it is a text literal.
    pub caption: Option<String>,
    /// The request page's `AboutTitle` and `AboutText` when they are text literals.
    pub about_title: Option<String>,
    pub about_text: Option<String>,
    /// The kind of the default layout.
    pub default_layout: ir::ReportLayout,
    pub layout_file_missing: bool,
    /// Why `Report.SaveAs(.., Xml, ..)` cannot write this report's dataset.
    pub dataset_reasons: Vec<String>,
}

/// The value of a property written as one text literal (`'it''s'`), unquoted.
fn text_literal(value: &str) -> Option<String> {
    let v = value.trim();
    let inner = v.strip_prefix('\'')?.strip_suffix('\'')?;
    let unescaped = inner.replace("''", "\u{0}");
    (!unescaped.contains('\'')).then(|| unescaped.replace('\u{0}', "'"))
}

/// The kind of the layout a report renders with by default. Only the two forms the corpus
/// uses are told apart (`DefaultLayout = RDLC`, and `DefaultRenderingLayout` naming a layout of
/// `Type = RDLC`); any other layout is `Other`.
fn default_layout(obj: &ast::Object) -> ir::ReportLayout {
    let prop = |n: &str| {
        obj.properties()
            .find(|p| p.name().eq_ignore_ascii_case(n))
            .map(|p| p.value_text().trim().to_string())
    };
    if let Some(name) = prop("DefaultRenderingLayout") {
        let name = crate::scan::unquote_double(&name);
        let layout = obj
            .sections()
            .filter(|s| s.name().eq_ignore_ascii_case("rendering"))
            .flat_map(|r| r.sections().collect::<Vec<_>>())
            .find(|l| {
                l.name().eq_ignore_ascii_case("layout")
                    && l.args().first().is_some_and(|a| a.eq_ignore_ascii_case(&name))
            });
        let rdlc = layout.is_some_and(|l| {
            l.properties()
                .any(|p| p.name().eq_ignore_ascii_case("Type") && p.value_text().trim().eq_ignore_ascii_case("RDLC"))
        });
        return if rdlc {
            ir::ReportLayout::Rdlc
        } else {
            ir::ReportLayout::Other
        };
    }
    if let Some(kind) = prop("DefaultLayout") {
        return if kind.eq_ignore_ascii_case("RDLC") {
            ir::ReportLayout::Rdlc
        } else {
            ir::ReportLayout::Other
        };
    }
    let declares_any = ["RDLCLayout", "WordLayout", "ExcelLayout"]
        .iter()
        .any(|n| prop(n).is_some())
        || obj.sections().any(|s| {
            ["rendering", "rdlclayout", "wordlayout", "excellayout"]
                .iter()
                .any(|n| s.name().eq_ignore_ascii_case(n))
        });
    if declares_any {
        ir::ReportLayout::Other
    } else {
        ir::ReportLayout::None
    }
}

/// A report with `DefaultLayout = ...` and no layout file at all (report/PRSaveAsFormats.al: `DefaultLayout = RDLC`
/// and nothing else).
fn layout_file_missing(obj: &ast::Object) -> bool {
    let has = |n: &str| obj.properties().any(|p| p.name().eq_ignore_ascii_case(n));
    has("DefaultLayout")
        && !has("DefaultRenderingLayout")
        && !["RDLCLayout", "WordLayout", "ExcelLayout"].iter().any(|n| has(n))
        && !obj.sections().any(|s| {
            ["rendering", "rdlclayout", "wordlayout", "excellayout"]
                .iter()
                .any(|n| s.name().eq_ignore_ascii_case(n))
        })
}

fn bool_prop(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
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
        output: Vec::new(),
        caption: None,
        about_title: None,
        about_text: None,
        default_layout: default_layout(obj),
        layout_file_missing: layout_file_missing(obj),
        dataset_reasons: Vec::new(),
    };
    for p in obj.properties() {
        let (name, value) = (p.name().to_ascii_lowercase(), p.value_text());
        match name.as_str() {
            "caption" => {
                out.caption = text_literal(&value);
                if out.caption.is_none() {
                    out.dataset_reasons
                        .push(format!("Caption = {} is not a text literal", value.trim()));
                }
            }
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
            // Rendering-only: how the report prints. `PreviewMode` belongs to the Base Application's
            // G/L Register (handlers/TestReportDatasetHelpLink.al runs its SaveAs).
            "defaultlayout"
            | "defaultrenderinglayout"
            | "rdlclayout"
            | "wordlayout"
            | "excellayout"
            | "previewmode" => out
                .output
                .push(format!("report property {name} (a layout) is not supported yet")),
            // The Schedule button of the request page: a run through `Report.Run` ignores it, true or false
            // (report/TestProbeReportContracts.al:AllowScheduling_TrueAndFalse_Run).
            "allowscheduling" => {}
            // The data item a Word layout is merged per record of: rendering only, by the
            // documentation (docs/probes-needed.md). Base Application's "Reference No Label" is named
            // by an action of the page Item Reference List that tests open without invoking it.
            "wordmergedataitem" => out
                .output
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
            "dataset" => visit(&top, None, None, &Place::Last, &mut out.items, &mut out.reasons),
            "requestpage" => {
                out.has_request_page = true;
                for p in top.properties() {
                    let pn = p.name().to_ascii_lowercase();
                    let value = p.value_text();
                    let slot = match pn.as_str() {
                        "abouttitle" => &mut out.about_title,
                        "abouttext" => &mut out.about_text,
                        _ => continue,
                    };
                    *slot = text_literal(&value);
                    if slot.is_none() {
                        out.dataset_reasons
                            .push(format!("{pn} = {} is not a text literal", value.trim()));
                    }
                }
            }
            "labels" if top.sections().next().is_some() => {
                out.dataset_reasons
                    .push("the report declares labels (whether the dataset carries them is not pinned)".into());
                out.output
                    .push("report section labels (a layout) is not supported yet".into());
            }
            n if LAYOUT_SECTIONS.contains(&n) => out
                .output
                .push(format!("report section {n} (a layout) is not supported yet")),
            other => out.reasons.push(format!("report section {other} is not supported yet")),
        }
    }
    if !out.processing_only {
        out.output
            .push("the report is not ProcessingOnly, so running it renders output".into());
    }
    out
}

/// Request page properties that change nothing a test client can observe.
const INERT_REQUEST_PAGE_PROPERTIES: &[&str] = &[
    "captionml",
    "abouttitle",
    "abouttitleml",
    "abouttext",
    "abouttextml",
    "contextsensitivehelppage",
];

/// Request page triggers the test client runs: it opens the page (`OnInit`, `OnOpenPage`) and
/// closes it (`OnQueryClosePage`, `OnClosePage`).
const REQUEST_PAGE_TRIGGERS: &[&str] = &["oninit", "onopenpage", "onqueryclosepage", "onclosepage"];

/// What a reportextension can add to a request page that the client runs: the void page triggers
/// after the report's own, and the triggers of a `modify` block that a page extension has
/// (docs/probes-needed.md, "Reportextension request pages").
fn request_extension_reasons(exts: &[(usize, ast::Object)]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, e) in exts {
        let ename = e.name();
        for sec in e.sections().filter(|s| s.name().eq_ignore_ascii_case("requestpage")) {
            for t in sec.triggers() {
                let n = t.name().to_ascii_lowercase();
                if !["onopenpage", "onclosepage"].contains(&n.as_str()) {
                    out.push(format!(
                        "reportextension `{ename}`: request page trigger {} is not supported yet",
                        t.name()
                    ));
                }
            }
            for p in sec.properties() {
                out.push(format!(
                    "reportextension `{ename}`: request page property {} is not supported yet",
                    p.name().to_ascii_lowercase()
                ));
            }
            for m in ast::sections_in(sec.syntax()) {
                if !m.name().eq_ignore_ascii_case("modify") {
                    continue;
                }
                for t in m.triggers() {
                    if !crate::lower::MODIFY_TRIGGERS.contains(&t.name().to_ascii_lowercase().as_str()) {
                        out.push(format!(
                            "reportextension `{ename}`: trigger {} in modify({}) is not supported yet",
                            t.name(),
                            m.args().first().cloned().unwrap_or_default()
                        ));
                    }
                }
            }
        }
    }
    out
}

/// The request page of report `obj`: the controls its `requestpage` section declares, then one
/// `DataItem` control per data item of `items` (the filters a handler sets on the page), and the
/// `requestpage` section itself when the report declares one. A report without one still has a
/// request page showing its data items' filters.
pub fn request_layout(
    obj: &ast::Object,
    items: &[RawItem],
    exts: &[(usize, ast::Object)],
) -> (crate::pages::Layout, Option<ast::Section>) {
    let section = obj.sections().find(|s| s.name().eq_ignore_ascii_case("requestpage"));
    // The `layout` and `actions` sections of each extension's `requestpage`.
    let ext_sections: Vec<(usize, Vec<ast::Section>)> = exts
        .iter()
        .map(|(e, o)| {
            let sections = o
                .sections()
                .filter(|s| s.name().eq_ignore_ascii_case("requestpage"))
                .flat_map(|s| s.sections().collect::<Vec<_>>())
                .collect();
            (*e, sections)
        })
        .collect();
    let mut layout = match &section {
        Some(s) => crate::pages::layout_from(s.sections(), &ext_sections),
        None => crate::pages::layout_from(std::iter::empty(), &ext_sections),
    };
    layout.reasons.extend(request_extension_reasons(exts));
    if let Some(s) = &section {
        for p in s.properties() {
            let name = p.name().to_ascii_lowercase();
            // `ShowFilter` hides the data item filters of the request page. Whether a TestRequestPage then still
            // has them is not pinned, so a literal value passes here and the test client refuses to touch a
            // data item of a page that says `false` (docs/probes-needed.md).
            let show_filter = name == "showfilter" && bool_prop(&p.value_text()).is_some();
            // `Editable = false` makes the page and its fields read Editable No (handlers/PSRequestPageEditable.al).
            let editable = name == "editable" && bool_prop(&p.value_text()).is_some();
            if !["caption", "savevalues", "sourcetable"].contains(&name.as_str())
                && !INERT_REQUEST_PAGE_PROPERTIES.contains(&name.as_str())
                && !show_filter
                && !editable
            {
                layout
                    .reasons
                    .push(format!("request page property {name} is not supported yet"));
            }
        }
        for t in s.triggers() {
            let name = t.name().to_ascii_lowercase();
            if !REQUEST_PAGE_TRIGGERS.contains(&name.as_str()) {
                layout
                    .reasons
                    .push(format!("request page trigger {name} is not supported yet"));
            }
        }
        for sub in s.sections() {
            let name = sub.name().to_ascii_lowercase();
            if name != "layout" && name != "actions" {
                layout
                    .reasons
                    .push(format!("request page section {name} is not supported yet"));
            }
        }
    }
    let mut kinds = Vec::new();
    for c in &layout.controls {
        match c.kind {
            ir::ControlKind::Part | ir::ControlKind::Repeater => kinds.push(format!(
                "request page control {} is a {} (not supported yet)",
                c.name,
                if c.kind == ir::ControlKind::Part {
                    "part"
                } else {
                    "repeater"
                }
            )),
            _ => {}
        }
    }
    layout.reasons.extend(kinds);
    for i in items {
        layout.controls.push(crate::pages::RawControl {
            name: i.name.clone(),
            kind: ir::ControlKind::DataItem,
            parent: None,
            section: i.section.clone(),
            owner: None,
            modifies: Vec::new(),
        });
    }
    (layout, section)
}

/// The data items under `section` (a `dataset`, a `dataitem` or an extension's `addlast`), nested
/// ones with `parent` set. `target` names the item a reportextension adds the top-level ones under.
fn visit(
    section: &ast::Section,
    parent: Option<usize>,
    target: Option<&str>,
    place: &Place,
    items: &mut Vec<RawItem>,
    reasons: &mut Vec<String>,
) {
    for s in section.sections() {
        let section_name = s.name().to_ascii_lowercase();
        match section_name.as_str() {
            "dataitem" => {}
            // Columns only feed the layout and the dataset a SaveAs writes.
            "column" if parent.is_some() || target.is_some() => continue,
            other => {
                reasons.push(format!("dataset element {other} is not supported yet"));
                continue;
            }
        }
        let args = s.args();
        let name = args.first().cloned().unwrap_or_default();
        let table = crate::types::section_object_name(&s, 1).unwrap_or_default();
        for p in s.properties() {
            let pn = p.name().to_ascii_lowercase();
            let temporary_ok =
                pn == "usetemporary" && matches!(p.value_text().trim().to_ascii_lowercase().as_str(), "true" | "false");
            if pn == "dataitemlinkreference" {
                // Read by `item_ir` (corpus handlers/TestReportDataItemLinkReferenceContracts.al).
            } else if !["dataitemtableview", "dataitemlink"].contains(&pn.as_str())
                && !INERT_ITEM_PROPERTIES.contains(&pn.as_str())
                && !temporary_ok
            {
                reasons.push(format!("property {pn} of dataitem {name} is not supported yet"));
            }
        }
        for t in s.triggers() {
            let tn = t.name().to_ascii_lowercase();
            if crate::pages::is_synthetic(&tn) {
                continue;
            }
            if !["onpredataitem", "onaftergetrecord", "onpostdataitem"].contains(&tn.as_str()) {
                reasons.push(format!("trigger {tn} of dataitem {name} is not supported yet"));
            }
        }
        if items.iter().any(|i| i.name.eq_ignore_ascii_case(&name)) {
            reasons.push(format!("dataitem {name} is declared twice"));
        }
        let idx = items.len();
        items.push(RawItem {
            name,
            table,
            parent,
            target: if parent.is_none() {
                target.map(String::from)
            } else {
                None
            },
            place: if parent.is_none() { place.clone() } else { Place::Last },
            section: s.clone(),
        });
        visit(&s, Some(idx), None, &Place::Last, items, reasons);
    }
}

/// The dataset changes and report triggers of a reportextension.
pub struct ExtLayout {
    /// `modify(Item)` blocks: the item's name and the block.
    pub modifies: Vec<(String, ast::Section)>,
    /// The data items added with `addlast`, with `target` naming the item above them.
    pub items: Vec<RawItem>,
    /// `add(Item)` blocks (columns for a base item): the item's name and the block.
    pub adds: Vec<(String, ast::Section)>,
    pub reasons: Vec<String>,
}

/// Walks reportextension `obj`. Only what the corpus pins is understood
/// (reportextension/TestReportExtensionTriggers.al): `OnPreReport` and `OnPostReport`,
/// `modify(Item)` with its two triggers, `add(Item)` of columns, and `addfirst`, `addlast`, `addbefore` and
/// `addafter` of data items.
pub fn ext_layout(obj: &ast::Object) -> ExtLayout {
    let mut out = ExtLayout {
        modifies: Vec::new(),
        items: Vec::new(),
        adds: Vec::new(),
        reasons: Vec::new(),
    };
    for p in obj.properties() {
        let name = p.name().to_ascii_lowercase();
        if !INERT_OBJECT_PROPERTIES.contains(&name.as_str()) {
            out.reasons
                .push(format!("reportextension property {name} is not supported yet"));
        }
    }
    for t in obj.triggers() {
        let n = t.name().to_ascii_lowercase();
        if !["onprereport", "onpostreport"].contains(&n.as_str()) {
            out.reasons
                .push(format!("reportextension trigger {n} is not supported yet"));
        }
    }
    for top in obj.sections() {
        match top.name().to_ascii_lowercase().as_str() {
            "dataset" => {
                for s in top.sections() {
                    let kind = s.name().to_ascii_lowercase();
                    let target = s.args().first().cloned().unwrap_or_default();
                    match kind.as_str() {
                        "modify" => {
                            for p in s.properties() {
                                out.reasons.push(format!(
                                    "property {} of modify({target}) is not supported yet",
                                    p.name().to_ascii_lowercase()
                                ));
                            }
                            for t in s.triggers() {
                                let tn = t.name().to_ascii_lowercase();
                                if ![
                                    "onbeforepredataitem",
                                    "onafterpredataitem",
                                    "onbeforepostdataitem",
                                    "onafterpostdataitem",
                                    "onbeforeaftergetrecord",
                                    "onafteraftergetrecord",
                                ]
                                .contains(&tn.as_str())
                                {
                                    out.reasons
                                        .push(format!("trigger {tn} of modify({target}) is not supported yet"));
                                }
                            }
                            out.modifies.push((target, s.clone()));
                        }
                        // Columns only feed the layout and the dataset a SaveAs writes.
                        "add" if s.sections().all(|c| c.name().eq_ignore_ascii_case("column")) => {
                            out.adds.push((target, s.clone()));
                        }
                        // The compiler allows addbefore and addafter on a nested data item only; addfirst and
                        // addlast name the parent (reportextensiontrigger/TestReportExtensionDataItemPlacementContracts.al).
                        "addlast" | "addfirst" | "addbefore" | "addafter" => {
                            if s.properties().next().is_some() || s.triggers().next().is_some() {
                                out.reasons.push(format!(
                                    "{kind}({target}) with properties or triggers is not supported yet"
                                ));
                            }
                            let place = match kind.as_str() {
                                "addlast" => Place::Last,
                                "addfirst" => Place::First,
                                "addbefore" => Place::Before,
                                _ => Place::After,
                            };
                            // Columns added next to a column, or first or last among the columns of an item,
                            // land at a place in the dataset no test pins; only `addlast(Item)` has been
                            // silently ignoring them so far.
                            if kind != "addlast" && s.sections().any(|c| c.name().eq_ignore_ascii_case("column")) {
                                out.reasons.push(format!(
                                    "{kind}({target}) adds columns, whose place in the dataset is not pinned"
                                ));
                            }
                            visit(&s, None, Some(&target), &place, &mut out.items, &mut out.reasons);
                        }
                        other => out.reasons.push(format!(
                            "reportextension dataset element {other}({target}) is not supported yet"
                        )),
                    }
                }
            }
            "requestpage" => {}
            other => out
                .reasons
                .push(format!("reportextension section {other} is not supported yet")),
        }
    }
    out
}

/// Everything `build` needs to know about the report being lowered.
pub struct Context<'a> {
    pub ws: &'a Workspace,
    pub members: &'a crate::members::MemberTable<'a>,
    pub file: usize,
    pub oi: usize,
    pub layout: &'a Layout,
    /// The global slot of each data item's record, by lower-case name.
    pub slots: &'a HashMap<String, usize>,
    pub request_flag_slot: usize,
    /// (object, source offset) to routine index.
    pub routine_at: &'a crate::lower::RoutineAt,
    /// Reportextensions that change what a run does: why the report cannot run yet.
    pub extension_reasons: Vec<String>,
    /// The reportextensions (object indices) that change what a run does, in object id order.
    pub extensions: Vec<usize>,
}

fn table_object(ws: &Workspace, file: usize, name: &str) -> Option<usize> {
    match ws.resolve_ref(file, ObjKind::Table, &crate::types::qualified_parts(name)) {
        Resolution::Found(t) => Some(t),
        _ => None,
    }
}

/// What lowering needs to know about the object that declares the data items.
struct Owner<'a> {
    ws: &'a Workspace,
    members: &'a crate::members::MemberTable<'a>,
    file: usize,
    oi: usize,
    routine_at: &'a crate::lower::RoutineAt,
    /// Source offsets of the object's `column` sections, in source order: the index of an offset
    /// is the ordinal `pages::column_name` names the column's routine by.
    column_starts: Vec<usize>,
    /// (ordinal, routine index) of the synthetic routine of each column that has one.
    column_routines: HashMap<usize, usize>,
}

impl<'a> Owner<'a> {
    fn new(
        ws: &'a Workspace,
        members: &'a crate::members::MemberTable<'a>,
        file: usize,
        oi: usize,
        routine_at: &'a crate::lower::RoutineAt,
        obj: &ast::Object,
    ) -> Owner<'a> {
        let column_starts: Vec<usize> = crate::pages::column_sections(obj)
            .iter()
            .map(|s| usize::from(s.syntax().text_range().start()))
            .collect();
        let names: HashMap<String, usize> = column_starts
            .iter()
            .enumerate()
            .map(|(i, _)| (crate::pages::column_name(i), i))
            .collect();
        let mut column_routines = HashMap::default();
        for r in ast::routines_in(obj.syntax()) {
            if let Some(&ordinal) = names.get(&r.name()) {
                if let Some(idx) = routine_at.get(&(oi, usize::from(r.syntax().text_range().start()))) {
                    column_routines.insert(ordinal, idx);
                }
            }
        }
        Owner {
            ws,
            members,
            file,
            oi,
            routine_at,
            column_starts,
            column_routines,
        }
    }

    /// The columns declared directly in `sec` (a `dataitem` or an extension's `add`).
    fn columns(&self, sec: &ast::Section, table: Option<usize>) -> Vec<ir::ReportColumn> {
        sec.sections()
            .filter(|c| c.name().eq_ignore_ascii_case("column"))
            .map(|c| {
                let args = c.args();
                let name = args.first().cloned().unwrap_or_default();
                let start = usize::from(c.syntax().text_range().start());
                let ordinal = self.column_starts.iter().position(|&s| s == start);
                let routine = ordinal.and_then(|o| self.column_routines.get(&o).copied());
                let mut unsupported = None;
                let mut include_caption = false;
                let mut decimal_places = None;
                for p in c.properties() {
                    let value = p.value_text();
                    match p.name().to_ascii_lowercase().as_str() {
                        // handlers/TestReportSaveAsColumnTypesContracts.al:222-248: `true` adds the label.
                        "includecaption" => match value.trim().to_ascii_lowercase().as_str() {
                            "true" => include_caption = true,
                            "false" => {}
                            other => {
                                unsupported.get_or_insert(format!("column {name}: IncludeCaption = {other}"));
                            }
                        },
                        "decimalplaces" => decimal_places = Some(value.trim().to_string()),
                        _ => {}
                    }
                }
                if unsupported.is_some() {
                } else if args.len() != 2 {
                    unsupported = Some(format!("column {name} does not state exactly a name and a source"));
                } else if routine.is_none() {
                    unsupported = Some(format!("column {name} has no routine"));
                } else if let Some(p) = c
                    .properties()
                    .find(|p| !FORMAT_ONLY_COLUMN_PROPERTIES.contains(&p.name().to_ascii_lowercase().as_str()))
                {
                    // Every other column property (IncludeCaption, BlankZero, ...) changes what the
                    // dataset carries for the column; no test pins any of them.
                    unsupported = Some(format!(
                        "column {name} states the property {} (its effect on the dataset is not pinned)",
                        p.name()
                    ));
                }
                // A source that is one field of the item's table: its properties decide how a
                // Decimal is formatted.
                let field = table.and_then(|t| {
                    let source = crate::scan::unquote_double(args.get(1)?);
                    self.members[t]
                        .fields
                        .iter()
                        .position(|f| f.name.eq_ignore_ascii_case(&source))
                });
                ir::ReportColumn {
                    name,
                    routine,
                    unsupported,
                    field,
                    include_caption,
                    decimal_places,
                }
            })
            .collect()
    }
}

/// An enclosing data item a `DataItemLinkReference` names: its index and its table.
type EnclosingItem = (usize, Option<usize>);

/// The IR of one data item. `parent_table` is the table of the item it is nested under.
fn item_ir(
    o: &Owner,
    raw: &RawItem,
    slot: Option<usize>,
    parent: Option<usize>,
    parent_table: Option<usize>,
    // The enclosing data item a `DataItemLinkReference` names: its index and its table.
    enclosing: &dyn Fn(&str) -> Option<EnclosingItem>,
    reasons: &mut Vec<String>,
) -> ir::ReportItem {
    let sec = &raw.section;
    let trigger = |name: &str| {
        sec.triggers()
            .find(|t| t.name().eq_ignore_ascii_case(name))
            .and_then(|t| o.routine_at.get(&(o.oi, usize::from(t.syntax().text_range().start()))))
    };
    let table = table_object(o.ws, o.file, &raw.table);
    if slot.is_none() || table.is_none() {
        reasons.push(format!(
            "dataitem {}: {}",
            raw.name,
            o.ws.outside_workspace(ObjKind::Table, &raw.table)
        ));
    }
    let prop = |n: &str| {
        sec.properties()
            .find(|p| p.name().eq_ignore_ascii_case(n))
            .map(|p| p.value_text())
    };
    let mut link = Vec::new();
    // `DataItemLinkReference = Name` links the fields to the named enclosing data item's row, not the
    // parent's (corpus handlers/TestReportDataItemLinkReferenceContracts.al).
    let mut link_item = None;
    let mut parent_table = parent_table;
    if let Some(name) = prop("DataItemLinkReference") {
        let name = crate::scan::unquote_double(name.trim());
        match enclosing(&name) {
            Some((idx, table)) => {
                link_item = Some(idx);
                parent_table = table;
            }
            None => reasons.push(format!(
                "dataitem {}: DataItemLinkReference `{name}` names no data item this one is nested under (an item a reportextension adds cannot name one yet)",
                raw.name
            )),
        }
    }
    let referenced_unknown = prop("DataItemLinkReference").is_some() && link_item.is_none();
    if let Some(text) = prop("DataItemLink").filter(|_| !referenced_unknown) {
        match parent_table.zip(table) {
            None => reasons.push(format!(
                "dataitem {}: DataItemLink needs an enclosing data item",
                raw.name
            )),
            Some((parent_table, own_table)) => {
                for pair in crate::scan::split_double(&text, ',') {
                    let parts = crate::scan::split_double(&pair, '=');
                    let own = parts.first().map(|p| crate::scan::unquote_double(p));
                    let other = parts.get(1).map(|p| p.trim()).and_then(|p| {
                        let low = p.to_ascii_lowercase();
                        (low.starts_with("field(") && p.ends_with(')'))
                            .then(|| crate::scan::unquote_double(&p[6..p.len() - 1]))
                    });
                    let own_pos = own.as_ref().and_then(|n| {
                        o.members[own_table]
                            .fields
                            .iter()
                            .position(|x| x.name.eq_ignore_ascii_case(n))
                    });
                    let other_pos = other.as_ref().and_then(|n| {
                        o.members[parent_table]
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
    ir::ReportItem {
        name: raw.name.clone(),
        parent,
        slot,
        table,
        view: prop("DataItemTableView"),
        request_filter_fields: prop("RequestFilterFields"),
        link,
        link_item,
        on_pre: trigger("OnPreDataItem"),
        on_after_get: trigger("OnAfterGetRecord"),
        on_post: trigger("OnPostDataItem"),
        columns: o.columns(sec, table),
    }
}

fn object_trigger(o: &Owner, obj: &ast::Object, name: &str) -> Option<usize> {
    obj.triggers()
        .find(|t| t.name().eq_ignore_ascii_case(name))
        .and_then(|t| o.routine_at.get(&(o.oi, usize::from(t.syntax().text_range().start()))))
}

/// The IR of report `cx.oi`. `reasons` are what the lowering of the item variables found.
pub fn build(cx: &Context, obj: &ast::Object, mut reasons: Vec<String>) -> ir::Report {
    let layout = cx.layout;
    let o = Owner::new(cx.ws, cx.members, cx.file, cx.oi, cx.routine_at, obj);
    let mut items: Vec<ir::ReportItem> = Vec::new();
    for raw in &layout.items {
        let slot = cx.slots.get(&raw.name.to_lowercase()).copied();
        let parent_table = raw.parent.and_then(|p| items.get(p).and_then(|i| i.table));
        let enclosing = |name: &str| {
            let mut at = raw.parent;
            while let Some(i) = at {
                if layout.items[i].name.eq_ignore_ascii_case(name) {
                    return Some((i, items.get(i).and_then(|x| x.table)));
                }
                at = layout.items[i].parent;
            }
            None
        };
        let item = item_ir(&o, raw, slot, raw.parent, parent_table, &enclosing, &mut reasons);
        items.push(item);
    }
    reasons.extend(layout.reasons.iter().cloned());
    reasons.extend(cx.extension_reasons.iter().cloned());
    ir::Report {
        processing_only: layout.processing_only,
        use_request_page: layout.use_request_page,
        has_request_page: layout.has_request_page,
        transaction_type: layout.transaction_type.clone(),
        items,
        on_init: object_trigger(&o, obj, "OnInitReport"),
        on_pre: object_trigger(&o, obj, "OnPreReport"),
        on_post: object_trigger(&o, obj, "OnPostReport"),
        extensions: cx.extensions.clone(),
        request_flag_slot: cx.request_flag_slot,
        output: layout.output.clone(),
        unsupported: reasons,
        caption: layout.caption.clone(),
        about_title: layout.about_title.clone(),
        about_text: layout.about_text.clone(),
        layout: layout.default_layout,
        dataset_unsupported: layout.dataset_reasons.clone(),
        layout_file_missing: layout.layout_file_missing,
    }
}

/// Everything `build_ext` needs to know about the reportextension being lowered.
pub struct ExtContext<'a> {
    pub ws: &'a Workspace,
    pub members: &'a crate::members::MemberTable<'a>,
    pub file: usize,
    pub oi: usize,
    pub base: usize,
    pub base_layout: &'a Layout,
    /// The extended report's IR (its items' tables).
    pub base_items: &'a [ir::ReportItem],
    pub layout: &'a ExtLayout,
    pub routine_at: &'a crate::lower::RoutineAt,
}

/// The slot of base item `i` in the extension's globals: the extension names the base items'
/// records first, then its own items (`ExtContext::base_items.len() + j`).
pub fn alias_slot(i: usize) -> usize {
    i
}

/// The IR of the reportextension `cx.oi`, and why it cannot run if it cannot.
pub fn build_ext(cx: &ExtContext, obj: &ast::Object) -> (ir::ReportExt, Vec<String>) {
    let o = Owner::new(cx.ws, cx.members, cx.file, cx.oi, cx.routine_at, obj);
    let mut reasons = cx.layout.reasons.clone();
    let base_index = |name: &str| {
        cx.base_layout
            .items
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(name))
    };
    let n = cx.base_items.len();
    let mut modifies = Vec::new();
    for (name, sec) in &cx.layout.modifies {
        let trigger = |t: &str| {
            sec.triggers().find(|x| x.name().eq_ignore_ascii_case(t)).and_then(|x| {
                cx.routine_at
                    .get(&(cx.oi, usize::from(x.syntax().text_range().start())))
            })
        };
        match base_index(name) {
            Some(item) => modifies.push(ir::ReportExtModify {
                item,
                before: trigger("OnBeforeAfterGetRecord"),
                after: trigger("OnAfterAfterGetRecord"),
                before_pre: trigger("OnBeforePreDataItem"),
                after_pre: trigger("OnAfterPreDataItem"),
                before_post: trigger("OnBeforePostDataItem"),
                after_post: trigger("OnAfterPostDataItem"),
            }),
            None => reasons.push(format!("modify({name}): the report has no such data item")),
        }
    }
    if modifies
        .iter()
        .enumerate()
        .any(|(i, m)| modifies[..i].iter().any(|x| x.item == m.item))
    {
        reasons.push("two modify blocks for one data item (their order is not pinned)".into());
    }
    let mut added: Vec<ir::AddedItem> = Vec::new();
    for (j, raw) in cx.layout.items.iter().enumerate() {
        let mut base_parent = None;
        let mut parent = raw.parent;
        let mut place = ir::AddedPlace::Last;
        let parent_table = if let Some(target) = &raw.target {
            let anchor = if let Some(i) = base_index(target) {
                Some(ir::ItemAnchor::Base(i))
            } else {
                cx.layout.items[..j]
                    .iter()
                    .position(|x| x.name.eq_ignore_ascii_case(target))
                    .map(ir::ItemAnchor::Added)
            };
            match (&raw.place, anchor) {
                // `addfirst(Parent)` and `addlast(Parent)` name the parent.
                (Place::First | Place::Last, Some(ir::ItemAnchor::Base(i))) => {
                    base_parent = Some(i);
                    if raw.place == Place::First {
                        place = ir::AddedPlace::First;
                    }
                    cx.base_items[i].table
                }
                (Place::First | Place::Last, Some(ir::ItemAnchor::Added(k))) => {
                    parent = Some(k);
                    if raw.place == Place::First {
                        place = ir::AddedPlace::First;
                    }
                    added[k].item.table
                }
                // `addbefore(Sibling)` and `addafter(Sibling)` name a sibling and sit under its parent.
                (Place::Before | Place::After, Some(a)) => {
                    place = if raw.place == Place::Before {
                        ir::AddedPlace::Before(a)
                    } else {
                        ir::AddedPlace::After(a)
                    };
                    match a {
                        ir::ItemAnchor::Base(i) => match cx.base_items[i].parent {
                            Some(p) => {
                                base_parent = Some(p);
                                cx.base_items[p].table
                            }
                            None => {
                                reasons.push(format!(
                                    "{target} is a top-level data item: addbefore and addafter cannot name it"
                                ));
                                None
                            }
                        },
                        ir::ItemAnchor::Added(k) => {
                            base_parent = added[k].base_parent;
                            parent = added[k].item.parent;
                            match (base_parent, parent) {
                                (Some(p), _) => cx.base_items[p].table,
                                (None, Some(q)) => added[q].item.table,
                                (None, None) => None,
                            }
                        }
                    }
                }
                (_, None) => {
                    reasons.push(format!(
                        "add...({target}): the report has no such data item (an item added by another extension is not supported)"
                    ));
                    None
                }
            }
        } else {
            raw.parent.and_then(|p| added.get(p).and_then(|a| a.item.table))
        };
        let item = item_ir(&o, raw, Some(n + j), parent, parent_table, &|_| None, &mut reasons);
        added.push(ir::AddedItem {
            item,
            base_parent,
            place,
        });
    }
    let aliases = (0..n)
        .filter(|&i| cx.base_items[i].slot.is_some())
        .map(|i| (i, alias_slot(i)))
        .collect();
    let mut adds = Vec::new();
    for (name, sec) in &cx.layout.adds {
        match base_index(name) {
            Some(item) => adds.push(ir::ReportExtAdd {
                item,
                columns: o.columns(sec, cx.base_items[item].table),
            }),
            None => reasons.push(format!("add({name}): the report has no such data item")),
        }
    }
    (
        ir::ReportExt {
            base: cx.base,
            on_pre: object_trigger(&o, obj, "OnPreReport"),
            on_post: object_trigger(&o, obj, "OnPostReport"),
            aliases,
            modifies,
            added,
            adds,
        },
        reasons,
    )
}

/// The layouts the `rendering` section of a report declares.
pub fn layouts_of(obj: &ast::Object) -> Vec<ir::LayoutDecl> {
    obj.sections()
        .filter(|s| s.name().eq_ignore_ascii_case("rendering"))
        .flat_map(|r| r.sections().collect::<Vec<_>>())
        .filter(|l| l.name().eq_ignore_ascii_case("layout"))
        .map(|l| ir::LayoutDecl {
            name: l.args().first().cloned().unwrap_or_default(),
            properties: l.properties().map(|p| (p.name(), p.value_text())).collect(),
        })
        .collect()
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
        assert!(all.contains("property calcfields"), "{all}");
        assert!(all.contains("trigger onfoo"), "{all}");
        // A layout means the run renders: it is refused when it gets that far, not at declaration.
        let output = l.output.join("; ");
        assert!(output.contains("defaultlayout"), "{output}");
        assert!(output.contains("section rendering"), "{output}");
    }

    #[test]
    fn word_merge_data_item_refuses_the_rendering_run_not_the_declaration() {
        // Base Application "Reference No Label" (`WordMergeDataItem = "Item Reference"`) is named by
        // an action of the page Item Reference List that tests open without invoking it.
        let l = layout_of(
            "report 1 R { WordMergeDataItem = A; DefaultRenderingLayout = W; dataset { dataitem(A; Cust) { } } rendering { } }",
        );
        assert!(!l.reasons.join("; ").contains("wordmergedataitem"), "{:?}", l.reasons);
        assert!(l.output.join("; ").contains("wordmergedataitem"), "{:?}", l.output);
    }

    #[test]
    fn search_terms_are_inert_on_a_report() {
        // AdditionalSearchTerms only feeds the client's Tell Me search; no AL code or test client
        // reads it, like Caption and UsageCategory next to it (docs/owner-review.md).
        let l = layout_of(
            "report 1 R { ProcessingOnly = true; AdditionalSearchTerms = 'x'; AdditionalSearchTermsML = 'x'; dataset { dataitem(A; Cust) { } } }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        let l = ext_layout_of("reportextension 1 E extends R { AdditionalSearchTerms = 'x'; }");
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }

    #[test]
    fn access_by_permission_is_inert_on_a_report() {
        // AccessByPermission decides whether the client shows the report to a user, from a permission on a
        // table. Permissions are not modelled (ReadPermission is always true) and no AL code reads it
        // (docs/owner-review.md, issue 28).
        let l = layout_of(
            "report 1 R { ProcessingOnly = true; AccessByPermission = TableData Customer = R; dataset { dataitem(A; Cust) { } } }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        let l = ext_layout_of("reportextension 1 E extends R { AccessByPermission = TableData Customer = R; }");
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }

    #[test]
    fn procedures_next_to_added_data_items_and_variables_are_accepted() {
        // Pinned by reportext/W3RpExtProcedures.al: a procedure called through a report variable runs on the
        // extension's instance of that variable.
        let l = ext_layout_of(
            "reportextension 1 E extends R { dataset { addlast(A) { dataitem(B; Ledger) { } } } var G: Integer; procedure P() begin end; }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }

    #[test]
    fn columns_added_next_to_a_column_stay_refused_but_data_items_are_placed() {
        // Where `addafter` puts a column is pinned by no corpus test.
        let l = ext_layout_of("reportextension 1 E extends R { dataset { addafter(A) { column(X; No) { } } } }");
        assert!(l.reasons.join(";").contains("addafter(A)"), "{:?}", l.reasons);
        // A data item is placed (reportextensiontrigger/TestReportExtensionDataItemPlacementContracts.al).
        for kind in ["addfirst", "addlast", "addbefore", "addafter"] {
            let l = ext_layout_of(&format!(
                "reportextension 1 E extends R {{ dataset {{ {kind}(A) {{ dataitem(B; Ledger) {{ }} }} }} }}"
            ));
            assert!(l.reasons.is_empty(), "{kind}: {:?}", l.reasons);
            assert_eq!(l.items.len(), 1);
        }
        let l = ext_layout_of("reportextension 1 E extends R { dataset { addbefore(A) { dataitem(B; Ledger) { } } } }");
        assert_eq!(l.items[0].place, Place::Before);
        assert_eq!(l.items[0].target.as_deref(), Some("A"));
    }

    #[test]
    fn a_report_that_is_not_processing_only_renders_output() {
        let l = layout_of("report 1 R { dataset { dataitem(A; Cust) { } } }");
        assert!(l.output.join(";").contains("ProcessingOnly"), "{:?}", l.output);
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }

    #[test]
    fn the_request_page_is_noted_not_walked() {
        let l = layout_of("report 1 R { ProcessingOnly = true; requestpage { layout { } } }");
        assert!(l.has_request_page);
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
    }

    fn ext_layout_of(src: &str) -> ExtLayout {
        let parse = al_syntax::parse(src, &Default::default());
        let root = ast::SourceFile::cast(parse.syntax()).unwrap();
        ext_layout(&root.objects().next().unwrap())
    }

    #[test]
    fn an_extension_walk_finds_modify_blocks_and_added_items_with_their_targets() {
        let l = ext_layout_of(
            "reportextension 1 E extends R { dataset { modify(A) { trigger OnBeforeAfterGetRecord() begin end; } add(A) { column(X; No) { } } addlast(A) { dataitem(B; Ledger) { dataitem(C; Ledger) { } } } } trigger OnPreReport() begin end; }",
        );
        assert!(l.reasons.is_empty(), "{:?}", l.reasons);
        assert_eq!(l.modifies.len(), 1);
        let items: Vec<_> = l
            .items
            .iter()
            .map(|i| (i.name.as_str(), i.parent, i.target.as_deref()))
            .collect();
        assert_eq!(items, vec![("B", None, Some("A")), ("C", Some(0), None)]);
    }

    #[test]
    fn what_an_extension_may_not_do_becomes_a_reason() {
        let l = ext_layout_of(
            "reportextension 1 E extends R { dataset { addfirst(A) { column(X; No) { } } modify(A) { Caption = 'x'; trigger OnFoo() begin end; } } procedure P() begin end; var G: Integer; trigger OnInitReport() begin end; }",
        );
        let all = l.reasons.join("; ");
        assert!(all.contains("addfirst(A)"), "{all}");
        assert!(all.contains("property caption of modify(A)"), "{all}");
        assert!(all.contains("trigger onfoo"), "{all}");
        assert!(all.contains("trigger oninitreport"), "{all}");
    }
}
