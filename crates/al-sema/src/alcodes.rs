//! Compile errors with the AL compiler's own codes (`AL0xxx`), for the must-not-compile corpus
//! (`tests/must-not-compile` of the corpus repo, branch `master`; ADR-009 and the owner
//! decision of 2026-10-14: our diagnostics for code BC rejects use BC's codes, and tests pin them).
//!
//! Rule 6 of `CLAUDE.md` applies in the other direction too: a check here rejects only a shape the
//! corpus case pins, never a guess at a wider rule. Where a check needs what the front end does not
//! have (the members of a .NET type, a dependency's source), the code is listed in [`NOT_CHECKED`]
//! and no diagnostic is produced.
//!
//! Two kinds of checks:
//! - [`check_workspace`] needs the source only. `check::check_with` runs it, so every compile has
//!   its diagnostics. It covers the apps of kind `Workspace`, never a dependency (the AL compiler
//!   has already vetted those).
//! - [`check_context`] needs facts about the app that `AppSource` does not carry (its target and
//!   runtime, the keys of its app.json). The caller supplies them; a compile without them skips
//!   these checks.
//!
//! The codes the checker itself reports (`check/`, where types are known) are AL0118, AL0122,
//! AL0132 and AL0161 (and AL0175 for Char - Text); they are listed in [`IMPLEMENTED`] too.

use crate::workspace::{AppKind, Diagnostic, ObjKind, Workspace};
use al_syntax::{SyntaxKind, SyntaxNode, ast};

/// The AL codes this front end reports, with what each one covers.
pub const IMPLEMENTED: &[(&str, &str)] = &[
    (
        "AL0118",
        "a bare call of a name that is not a procedure or system function, in a codeunit",
    ),
    (
        "AL0122",
        "an enum assigned to an enum of another type whose target does not state AssignmentCompatibility = true; an Integer variable assigned to a Text; a text literal of two or more characters assigned to an Integer",
    ),
    (
        "AL0124",
        "app.json key `assemblyProbingPaths` of a Cloud app at runtime 15.0, 16.0, 16.1, 17.0, 17.1 and 18.0; the property OrderBy on a query data item; the property AutoCalcField on an xmlport table element",
    ),
    (
        "AL0132",
        "ExecutionMode::Install, Uninstall, Upgrade, Report, Profile; a field or method of a record that the table and the catalog lack; Length on a Text or Code",
    ),
    (
        "AL0161",
        "a page extension names a non-protected variable or a local procedure of its base page, or a report extension does; a Cloud app names an internal agent table of the platform; a subscriber to the internal event GetRoleCenterBannerPartID of System Action Triggers; a page customization names a variable of its page in Editable of a modify",
    ),
    ("AL0162", "OnInit, OnFindRecord and OnNextRecord in a page extension"),
    ("AL0197", "two objects of one type and name in one app"),
    (
        "AL0296",
        "a Cloud app names a platform table that is Scope = OnPrem (the 123 of System.app that the corpus lists), calls GetLastErrorObject, SecretText.Unwrap, or File.Open, or sets the report property EnableExternalAssemblies to true",
    ),
    (
        "AL0433",
        "a Cloud app names a removed platform table (the 14 of System.app that the corpus lists)",
    ),
    (
        "AL0365",
        "DataItemLink on a query data item with SqlJoinType = CrossJoin",
    ),
    (
        "AL0385",
        "IncludeCaption = true on a report column whose source is an expression",
    ),
    (
        "AL0440",
        "a page or report extension procedure, or a second procedure of a codeunit, table, page or report, with the signature of a procedure it could reach",
    ),
];

/// Codes the must-not-compile corpus expects and this front end cannot decide, with the reason. AL0175 is reported
/// for `Char - Text` and `Text - Char` on two variables only (case 0500); a case that finds its code matches.
pub const NOT_CHECKED: &[(&str, &str)] = &[(
    "AL0175",
    "needs the .NET type behind the alias (an enum is comparable with `=`, a class object is not); the front end has no assembly metadata.",
)];

/// Why `code` is not checked, if it is listed in [`NOT_CHECKED`]; `Some("not implemented")` for a
/// code in neither list.
pub fn not_checked_reason(code: &str) -> Option<&'static str> {
    if IMPLEMENTED.iter().any(|(c, _)| *c == code) {
        return None;
    }
    Some(
        NOT_CHECKED
            .iter()
            .find(|(c, _)| *c == code)
            .map_or("not implemented", |(_, why)| why),
    )
}

/// The target of an app (`app.json` `target`; the compiler's default is Cloud).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTarget {
    Cloud,
    OnPrem,
    Extension,
    Internal,
}

/// The app runtimes (major, minor) at which the compiler refuses `assemblyProbingPaths` (corpus 0007, 0071 to 0073).
///
/// 17.1 and 18.0 are the runtimes of the corpus apps on BC 28 and BC 29 (`oracle.toml` `app_runtime`); corpus
/// case 0007 (default runtime of its leg) gets AL0124 on them when the corpus checker runs with AL 30.0.42.60748-beta.
const ASSEMBLY_PROBING_REFUSED: &[(u32, u32)] = &[(15, 0), (16, 0), (16, 1), (17, 0), (17, 1), (18, 0)];

/// What the compiler knows of an app besides its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppContext {
    pub target: AppTarget,
    /// `runtime` of app.json as (major, minor).
    pub runtime: Option<(u32, u32)>,
    /// The top-level keys of app.json.
    pub app_json_keys: Vec<String>,
    /// The BC major the app is compiled for (the platform symbols it sees); `None` when not known.
    pub bc_major: Option<u32>,
}

/// Platform tables declared `Scope = OnPrem`. Pinned by the must-not-compile corpus: cases 0006
/// (`Published Application`) and 0014 (`API Webhook Entity`), BC 27.5 compiler. Other platform
/// tables may be OnPrem too; none is guessed (docs/probes-needed.md).
pub const ONPREM_PLATFORM_TABLES: &[&str] = &["Published Application", "API Webhook Entity"];

/// The platform tables of System.app (BC 27.0) with `Scope = OnPrem`: naming one from a Cloud app is AL0296. Corpus
/// must-not-compile 0020 to 0024 name all 123, 30 per case, with the symbols of BC 27 only, so the verdict is not
/// known for another major. Sorted by name.
pub const ONPREM_PLATFORM_TABLES_27: &[&str] = &[
    "API Webhook Entity",
    "API Webhook Notification",
    "API Webhook Notification Aggr",
    "API Webhook Subscription",
    "Agent",
    "Agent Access Control Data",
    "Agent Data",
    "Agent Task Data",
    "Agent Task File Data",
    "Agent Task Log Entry",
    "Agent Task Log Entry Data",
    "Agent Task Memory Entry",
    "Agent Task Memory Entry Data",
    "Agent Task Message Data",
    "Agent Task Msg Attach Data",
    "Agent Task Page Script",
    "Agent Task Timeline",
    "Agent Task Timeline Step",
    "Agent Task Timeline Step Data",
    "Agent Task Timeline Step Det.",
    "Agent Task User Int. Data",
    "Agent Troubleshoot Info Data",
    "Application Dependency",
    "Application Object Metadata",
    "Application Resource",
    "Configuration Package File",
    "Data Sensitivity",
    "Database",
    "Debugger Breakpoint",
    "Debugger Call Stack",
    "Debugger Variable",
    "Debugger Watch",
    "Debugger Watch Value",
    "Designed Query",
    "Designed Query Caption",
    "Designed Query Category",
    "Designed Query Column",
    "Designed Query Column Filter",
    "Designed Query Data Item",
    "Designed Query Filter",
    "Designed Query Group",
    "Designed Query Join",
    "Designed Query Management",
    "Designed Query Obj",
    "Designed Query Order By",
    "Device",
    "Document Service",
    "Document Service Scenario",
    "Drive",
    "Entitlement",
    "Entitlement Set",
    "Ext. Business Event Definition",
    "Extension Database Snapshot",
    "Extension Execution Info",
    "External Event Activity Log",
    "External Event Log Entry",
    "External Event Notification",
    "External Event Subscription",
    "File",
    "Inplace Installed Application",
    "Installed Application",
    "Intelligent Cloud",
    "Intelligent Cloud Status",
    "Isolated Storage",
    "MCP Configuration",
    "MCP Configuration Tool",
    "Media",
    "Media Set",
    "Membership Entitlement",
    "NAV App",
    "NAV App Capabilities",
    "NAV App Data Archive",
    "NAV App Dependencies",
    "NAV App Extra",
    "NAV App Object Metadata",
    "NAV App Object Prerequisites",
    "NAV App Publish Reference",
    "NAV App Published App",
    "NAV App Resource",
    "NAV App Tenant App",
    "NAV App Tenant Operation",
    "Object",
    "Object Metadata",
    "Page Documentation",
    "Page Info And Fields",
    "Page Table Field",
    "Page Usage State",
    "Performance Profile Scheduler",
    "Performance Profiles",
    "Permission Range",
    "Power BI Blob",
    "Power BI Default Selection",
    "Profile",
    "Profile Configuration Symbols",
    "Profile Metadata",
    "Profile Page Metadata",
    "Published Application",
    "Query Navigation",
    "Report Printer",
    "Report Settings Override",
    "SID - Account ID",
    "Send-To Program",
    "Server Instance",
    "Style Sheet",
    "System Object",
    "Table Field Types",
    "Table Information",
    "Table Synch. Setup",
    "Tenant Application Storage",
    "Tenant Feature Key",
    "Tenant Profile",
    "Tenant Profile Extension",
    "Tenant Profile Page Metadata",
    "Tenant Profile Setting",
    "Token Cache",
    "Upgrade Blob Storage",
    "User AI Consumption Data",
    "User Consumption Log Entry",
    "User Page Metadata",
    "User Property",
    "Web Service",
    "Webhook Notification",
    "Webhook Subscription",
];

/// The tables among them that are `Access = Internal` in the namespace `System.Agents.Internal`: naming one is also AL0161
/// (corpus 0023, 0024; BC 27).
pub const INTERNAL_AGENT_TABLES_27: &[&str] = &[
    "Agent Access Control Data",
    "Agent Data",
    "Agent Task Data",
    "Agent Task File Data",
    "Agent Task Log Entry Data",
    "Agent Task Memory Entry Data",
    "Agent Task Message Data",
    "Agent Task Msg Attach Data",
    "Agent Task Timeline Step Data",
    "Agent Task User Int. Data",
    "Agent Troubleshoot Info Data",
];

/// The tables among them that are removed, with the compiler's reason: naming one is also AL0433 (corpus 0020, 0021; BC 27).
pub const REMOVED_PLATFORM_TABLES_27: &[(&str, &str)] = &[
    (
        "Debugger Breakpoint",
        "Support for the classic debugger engine has been removed..",
    ),
    (
        "Debugger Call Stack",
        "Support for the classic debugger engine has been removed..",
    ),
    (
        "Debugger Variable",
        "Support for the classic debugger engine has been removed..",
    ),
    (
        "Debugger Watch",
        "Support for the classic debugger engine has been removed..",
    ),
    (
        "NAV App",
        "This table is deprecated in favour of the \"Published Application\" table..",
    ),
    (
        "NAV App Dependencies",
        "This table is deprecated in favour of the \"Application Dependency\" table..",
    ),
    (
        "NAV App Object Metadata",
        "This table is deprecated in favour of the \"Application Object Metadata\" table..",
    ),
    (
        "NAV App Publish Reference",
        "This table has been deprecated with the support for per tenant extensions..",
    ),
    (
        "NAV App Published App",
        "This table has been deprecated with the support for per tenant extensions..",
    ),
    (
        "NAV App Resource",
        "This table is deprecated in favour of the \"Application Resource\" table..",
    ),
    (
        "NAV App Tenant App",
        "This table is deprecated in favour of the \"Installed Application\" table..",
    ),
    (
        "Profile",
        "Support for System Profiles has been removed. Profiles should now be defined in AL extensions and should be accessed through the All Profile virtual table.. Tag: 22.0.",
    ),
    (
        "Profile Metadata",
        "Support for System Profiles has been removed. Profiles should now be defined in AL extensions and should be accessed through the All Profile virtual table.. Tag: 22.0.",
    ),
    (
        "Profile Page Metadata",
        "Support for System Profiles has been removed. Profiles should now be defined in AL extensions and should be accessed through the All Profile virtual table.. Tag: 22.0.",
    ),
];

fn push(out: &mut Vec<Diagnostic>, ws: &Workspace, file: usize, offset: usize, code: &'static str, message: String) {
    out.push(Diagnostic {
        file: ws.files[file].path.clone(),
        offset,
        code,
        message,
    });
}

fn offset_of(n: &SyntaxNode) -> usize {
    n.text_range().start().into()
}

fn display_kind(k: ObjKind) -> &'static str {
    match k {
        ObjKind::Table => "Table",
        ObjKind::TableExtension => "TableExtension",
        ObjKind::Page => "Page",
        ObjKind::PageExtension => "PageExtension",
        ObjKind::Codeunit => "Codeunit",
        ObjKind::Report => "Report",
        ObjKind::ReportExtension => "ReportExtension",
        ObjKind::Query => "Query",
        ObjKind::XmlPort => "XmlPort",
        ObjKind::Enum => "Enum",
        ObjKind::EnumExtension => "EnumExtension",
        ObjKind::Interface => "Interface",
        ObjKind::PermissionSet => "PermissionSet",
        ObjKind::PermissionSetExtension => "PermissionSetExtension",
        ObjKind::Profile => "Profile",
        ObjKind::Other => "Object",
    }
}

/// The checks that need the source only, over the apps of kind `Workspace`.
pub fn check_workspace(ws: &Workspace) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    duplicate_names(ws, &mut out);
    event_subscriptions(ws, &mut out);
    for oi in 0..ws.objects.len() {
        let o = &ws.objects[oi];
        if ws.kinds[o.app] == AppKind::Dependency {
            continue;
        }
        let obj = ws.object_ast(oi);
        if obj.kind() == "pagecustomization" {
            page_customization_variables(ws, oi, &obj, &mut out);
        }
        match o.kind {
            ObjKind::XmlPort => xmlport_elements(ws, o.file, obj.sections(), &mut out),
            ObjKind::PageExtension => {
                page_extension_triggers(ws, oi, &obj, &mut out);
                extension_procedures(ws, oi, &obj, ObjKind::Page, &mut out);
            }
            ObjKind::ReportExtension => extension_procedures(ws, oi, &obj, ObjKind::Report, &mut out),
            ObjKind::Codeunit => duplicate_procedures(ws, oi, &obj, &mut out),
            ObjKind::Query => query_joins(ws, o.file, obj.sections(), &mut out),
            ObjKind::Report => report_columns(ws, o.file, obj.sections(), &mut out),
            _ => {}
        }
    }
    out
}

/// AL0161. A subscription to an event of a platform codeunit that is an `InternalEvent` (corpus 0740:
/// `GetRoleCenterBannerPartID` of "System Action Triggers"). Other internal events are not listed, none is guessed.
const INTERNAL_PLATFORM_EVENTS: &[(&str, &str)] = &[("System Action Triggers", "GetRoleCenterBannerPartID")];

fn event_subscriptions(ws: &Workspace, out: &mut Vec<Diagnostic>) {
    for (fi, f) in ws.files.iter().enumerate() {
        if ws.kinds[f.app] == AppKind::Dependency {
            continue;
        }
        let text = ws.text(fi);
        // The objects of a parsed file carry no scanned subscribers; read the attributes with the scanner, for the files that name
        // one of the events at all.
        if !INTERNAL_PLATFORM_EVENTS
            .iter()
            .any(|(_, e)| text.to_ascii_lowercase().contains(&e.to_ascii_lowercase()))
        {
            continue;
        }
        let scan = al_syntax::scan::scan(text, &Default::default());
        for s in scan.objects.iter().flat_map(|o| o.subscribers.iter()) {
            if s.publisher_kind != "codeunit" {
                continue;
            }
            let name = s.publisher.join(".");
            let Some((publisher, event)) = INTERNAL_PLATFORM_EVENTS
                .iter()
                .find(|(p, e)| p.eq_ignore_ascii_case(&name) && e.eq_ignore_ascii_case(&s.event))
            else {
                continue;
            };
            // A codeunit of that name in a workspace app is the one named.
            let own = ws.objects.iter().any(|x| {
                x.kind == ObjKind::Codeunit
                    && x.name.eq_ignore_ascii_case(publisher)
                    && ws.kinds[x.app] != AppKind::Dependency
            });
            if !own {
                push(
                    out,
                    ws,
                    fi,
                    s.offset,
                    "AL0161",
                    format!("'{event}' is inaccessible due to its protection level"),
                );
            }
        }
    }
}

/// AL0124. The property `AutoCalcField` on a table element of an xmlport (corpus 0826; it belongs to report and query columns).
fn xmlport_elements(
    ws: &Workspace,
    file: usize,
    sections: impl Iterator<Item = ast::Section>,
    out: &mut Vec<Diagnostic>,
) {
    for s in sections {
        if s.name().eq_ignore_ascii_case("tableelement") {
            if let Some(p) = s.properties().find(|p| p.name().eq_ignore_ascii_case("AutoCalcField")) {
                push(
                    out,
                    ws,
                    file,
                    offset_of(p.syntax()),
                    "AL0124",
                    "The property 'AutoCalcField' cannot be used in this context".to_string(),
                );
            }
        }
        xmlport_elements(ws, file, s.sections(), out);
    }
}

/// AL0161. A page customization names a variable of the page it customizes in the `Editable` expression of a
/// `modify` (corpus 0865); it has no access to the page's variables. Other properties are not judged.
fn page_customization_variables(ws: &Workspace, oi: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    let Some(parts) = obj.customizes_parts() else { return };
    let Some(page) = ws.lookup(Some(ws.objects[oi].app), ObjKind::Page, &parts.join(".")) else {
        return;
    };
    let page_ast = ws.object_ast(page);
    fn walk(
        ws: &Workspace,
        file: usize,
        page: &ast::Object,
        sections: impl Iterator<Item = ast::Section>,
        in_modify: bool,
        out: &mut Vec<Diagnostic>,
    ) {
        for s in sections {
            let modify = in_modify || s.name().eq_ignore_ascii_case("modify");
            if s.name().eq_ignore_ascii_case("modify") {
                for p in s.properties() {
                    let value = p.value_text();
                    let name = value.trim();
                    if p.name().eq_ignore_ascii_case("Editable")
                        && !name.is_empty()
                        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        && crate::base_access::declares_var(page, name)
                    {
                        push(
                            out,
                            ws,
                            file,
                            offset_of(p.syntax()),
                            "AL0161",
                            format!("'{name}' is inaccessible due to its protection level"),
                        );
                    }
                }
            }
            walk(ws, file, page, s.sections(), modify, out);
        }
    }
    walk(ws, ws.objects[oi].file, &page_ast, obj.sections(), false, out);
}

/// AL0197: one diagnostic for every declaration of a name that an app declares more than once for
/// the same object type (corpus 0016: two codeunits, two diagnostics).
fn duplicate_names(ws: &Workspace, out: &mut Vec<Diagnostic>) {
    let mut seen: std::collections::HashMap<(usize, ObjKind, String, Option<String>), Vec<usize>> = Default::default();
    for (oi, o) in ws.objects.iter().enumerate() {
        if ws.kinds[o.app] == AppKind::Dependency || o.name.is_empty() {
            continue;
        }
        let ns = o.namespace.as_ref().map(|n| n.to_lowercase());
        seen.entry((o.app, o.kind, o.name.to_lowercase(), ns))
            .or_default()
            .push(oi);
    }
    let mut dups: Vec<usize> = seen.into_values().filter(|v| v.len() > 1).flatten().collect();
    dups.sort_unstable();
    for oi in dups {
        let o = &ws.objects[oi];
        push(
            out,
            ws,
            o.file,
            o.offset,
            "AL0197",
            format!(
                "An application object of type '{}' with name '{}' is already declared by the extension",
                display_kind(o.kind),
                o.name
            ),
        );
    }
}

/// The triggers of a page that a page extension may not declare (corpus 0004, 0017, 0018). The other
/// twelve page triggers compile in an extension (corpus al-language/pageext/POPageExtTriggers.al);
/// any other trigger name is not judged here.
const PAGE_ONLY_TRIGGERS: &[&str] = &["OnInit", "OnFindRecord", "OnNextRecord"];

/// AL0162. A page has the triggers OnInit, OnFindRecord and OnNextRecord; a page extension does not
/// (corpus 0004, 0017, 0018).
fn page_extension_triggers(ws: &Workspace, oi: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    for t in obj.triggers() {
        let name = t.name();
        if PAGE_ONLY_TRIGGERS.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            push(
                out,
                ws,
                ws.objects[oi].file,
                t.keyword_offset().unwrap_or_else(|| offset_of(t.syntax())),
                "AL0162",
                format!("'{name}' is not a valid trigger"),
            );
        }
    }
}

pub(crate) fn signature(r: &ast::Routine) -> Vec<(bool, String)> {
    r.params()
        .map(|p| {
            let ty = p.type_ref().map(|t| t.text()).unwrap_or_default();
            (p.is_var(), ty.split_whitespace().collect::<String>().to_lowercase())
        })
        .collect()
}

/// AL0440. A page or report extension procedure with the name and parameter types of a procedure of
/// its base object that it could otherwise reach (not `local`), corpus 0003 (page), 0042 (report) and
/// handlers/TestPageExtensionBaseAccessContracts.al:20.
fn extension_procedures(ws: &Workspace, oi: usize, obj: &ast::Object, base_kind: ObjKind, out: &mut Vec<Diagnostic>) {
    let Some(base) = ws.extension_target(oi) else { return };
    if ws.objects[base].kind != base_kind {
        return;
    }
    let base_ast = ws.object_ast(base);
    for r in obj.routines() {
        let name = r.name();
        let sig = signature(&r);
        if crate::base_access::visible_procs(&base_ast, &name).any(|b| signature(&b) == sig) {
            push(
                out,
                ws,
                ws.objects[oi].file,
                r.keyword_offset().unwrap_or_else(|| offset_of(r.syntax())),
                "AL0440",
                format!(
                    "The {} '{}' already defines a method called '{name}' with the same parameter types in '{}'.",
                    display_kind(base_kind),
                    ws.objects[base].name,
                    ws.apps[ws.objects[base].app].name
                ),
            );
        }
    }
}

/// AL0440. A procedure of a codeunit that repeats the name and parameter types of an earlier one of the same
/// codeunit (corpus 0043; the names compare without case, the parameter types as written without blanks).
/// Other object types are not judged: no case pins them.
fn duplicate_procedures(ws: &Workspace, oi: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<(String, Vec<(bool, String)>)> = Vec::new();
    for r in obj.routines() {
        if r.is_trigger() {
            continue;
        }
        let key = (r.name().to_lowercase(), signature(&r));
        if seen.contains(&key) {
            push(
                out,
                ws,
                ws.objects[oi].file,
                r.keyword_offset().unwrap_or_else(|| offset_of(r.syntax())),
                "AL0440",
                format!(
                    "The Codeunit '{}' already defines a method called '{}' with the same parameter types in '{}'.",
                    ws.objects[oi].name,
                    r.name(),
                    ws.apps[ws.objects[oi].app].name
                ),
            );
        } else {
            seen.push(key);
        }
    }
}

fn property<'a>(props: &'a [(String, String)], name: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn properties_of(s: &ast::Section) -> Vec<(String, String)> {
    s.properties().map(|p| (p.name(), p.value_text())).collect()
}

/// AL0365. `DataItemLink` on a query data item whose `SqlJoinType` is `CrossJoin` (corpus 0009).
fn query_joins(ws: &Workspace, file: usize, sections: impl Iterator<Item = ast::Section>, out: &mut Vec<Diagnostic>) {
    for s in sections {
        let name = s.name();
        if name.eq_ignore_ascii_case("dataitem") {
            let props = properties_of(&s);
            // AL0124 (corpus 0103): OrderBy belongs to the query object, not to a data item.
            if let Some(p) = s.properties().find(|p| p.name().eq_ignore_ascii_case("OrderBy")) {
                push(
                    out,
                    ws,
                    file,
                    offset_of(p.syntax()),
                    "AL0124",
                    "The property 'OrderBy' cannot be used in this context".to_string(),
                );
            }
            let cross = property(&props, "SqlJoinType").is_some_and(|v| v.eq_ignore_ascii_case("CrossJoin"));
            if cross {
                if let Some(link) = s.properties().find(|p| p.name().eq_ignore_ascii_case("DataItemLink")) {
                    push(
                        out,
                        ws,
                        file,
                        offset_of(link.syntax()),
                        "AL0365",
                        "The property 'DataItemLink' cannot be set if the property 'SqlJoinType' is set to 'CrossJoin'"
                            .to_string(),
                    );
                }
            }
        }
        // `elements { dataitem(...) { dataitem(...) } }`: descend through every section.
        query_joins(ws, file, s.sections(), out);
    }
}

/// AL0385. `IncludeCaption = true` on a report column whose source is an expression, not a named
/// field (corpus 0015). A source that is a name (a field, or a variable) is not judged here.
fn report_columns(
    ws: &Workspace,
    file: usize,
    sections: impl Iterator<Item = ast::Section>,
    out: &mut Vec<Diagnostic>,
) {
    for s in sections {
        if s.name().eq_ignore_ascii_case("column") {
            let props = properties_of(&s);
            let include = property(&props, "IncludeCaption").is_some_and(|v| v.eq_ignore_ascii_case("true"));
            // The source is a call (`Format(Id)`, corpus 0015); an indexed variable's field
            // (`BomComponent[Level]."No."`, Base Application's "Quantity Explosion of BOM") is a named field.
            let is_call = s.args().get(1).is_some_and(|a| {
                let mut quoted = false;
                a.chars().any(|c| {
                    if c == '"' {
                        quoted = !quoted;
                    }
                    c == '(' && !quoted
                })
            });
            if include && s.arg_name_parts(1).is_none() && is_call {
                if let Some(p) = s.properties().find(|p| p.name().eq_ignore_ascii_case("IncludeCaption")) {
                    push(
                        out,
                        ws,
                        file,
                        offset_of(p.syntax()),
                        "AL0385",
                        "IncludeCaption can be set to true only if the source of the column is a named field."
                            .to_string(),
                    );
                }
            }
        }
        report_columns(ws, file, s.sections(), out);
    }
}

/// The checks that need the facts of app `app` (`AppContext`).
pub fn check_context(ws: &Workspace, app: usize, ctx: &AppContext) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // AL0124 (corpus 0007, 0071 to 0073): a Cloud app's app.json at runtime 15.0, 16.0, 16.1, 17.0, 17.1 or 18.0 does not
    // take `assemblyProbingPaths`. Those six runtimes are pinned, and only for the default target (Cloud).
    if ctx.target == AppTarget::Cloud
        && ctx.runtime.is_some_and(|r| ASSEMBLY_PROBING_REFUSED.contains(&r))
        && ctx.app_json_keys.iter().any(|k| k == "assemblyProbingPaths")
    {
        if let Some(file) = ws.files.iter().position(|f| f.app == app) {
            push(
                &mut out,
                ws,
                file,
                0,
                "AL0124",
                "The property 'assemblyProbingPaths' cannot be used in this context".to_string(),
            );
        }
    }
    // AL0296 (corpus 0806): a Cloud app sets the report property EnableExternalAssemblies, which has scope OnPrem. Only the value
    // `true` is pinned.
    if ctx.target == AppTarget::Cloud {
        for (oi, o) in ws.objects.iter().enumerate() {
            if o.app != app || o.kind != ObjKind::Report {
                continue;
            }
            let obj = ws.object_ast(oi);
            if let Some(p) = obj.properties().find(|p| {
                p.name().eq_ignore_ascii_case("EnableExternalAssemblies")
                    && p.value_text().trim().eq_ignore_ascii_case("true")
            }) {
                push(
                    &mut out,
                    ws,
                    o.file,
                    offset_of(p.syntax()),
                    "AL0296",
                    "The application object or method 'EnableExternalAssemblies' has scope 'OnPrem' and cannot be used for 'Cloud' development."
                        .to_string(),
                );
            }
        }
    }
    // AL0296 (corpus 0006, 0014; and for BC 27 0020 to 0024): a Cloud app names a platform table that is
    // Scope = OnPrem. The same tables can add AL0161 (internal agent tables) and AL0433 (removed tables).
    if ctx.target == AppTarget::Cloud {
        for (fi, f) in ws.files.iter().enumerate() {
            if f.app != app {
                continue;
            }
            let root = SyntaxNode::new_root(ws.green(fi));
            for n in root.descendants().filter(|n| n.kind() == SyntaxKind::TypeRef) {
                let Some(t) = ast::TypeRef::cast(n.clone()) else {
                    continue;
                };
                if !t.type_name().eq_ignore_ascii_case("record") {
                    continue;
                }
                let Some(name) = t.object_name() else { continue };
                let any_major = ONPREM_PLATFORM_TABLES.iter().find(|x| x.eq_ignore_ascii_case(&name));
                let on_27 = al_version::platform_table_scopes_known(ctx.bc_major);
                let in_27 = on_27
                    .then(|| ONPREM_PLATFORM_TABLES_27.iter().find(|x| x.eq_ignore_ascii_case(&name)))
                    .flatten();
                let Some(table) = any_major.or(in_27) else {
                    continue;
                };
                // A table of that name in the app (or what it depends on) is the one named.
                if ws.find(ObjKind::Table, &name).is_some() {
                    continue;
                }
                push(
                    &mut out,
                    ws,
                    fi,
                    offset_of(&n),
                    "AL0296",
                    format!(
                        "The application object or method '{table}' has scope 'OnPrem' and cannot be used for 'Cloud' development."
                    ),
                );
                if !on_27 {
                    continue;
                }
                if let Some(agent) = INTERNAL_AGENT_TABLES_27.iter().find(|x| x.eq_ignore_ascii_case(&name)) {
                    push(
                        &mut out,
                        ws,
                        fi,
                        offset_of(&n),
                        "AL0161",
                        format!(
                            "'Table System.Agents.Internal.\"{agent}\"' is inaccessible due to its protection level"
                        ),
                    );
                }
                if let Some((removed, reason)) = REMOVED_PLATFORM_TABLES_27
                    .iter()
                    .find(|(x, _)| x.eq_ignore_ascii_case(&name))
                {
                    push(
                        &mut out,
                        ws,
                        fi,
                        offset_of(&n),
                        "AL0433",
                        format!("Table '{removed}' is removed. Reason: {reason}"),
                    );
                }
            }
        }
    }
    // AL0296 (corpus 0110, 0112, 0113): a Cloud app calls a method that has scope OnPrem: GetLastErrorObject, the
    // Unwrap of a SecretText, the Open of a File. Only these three shapes are pinned.
    if ctx.target == AppTarget::Cloud {
        for (fi, f) in ws.files.iter().enumerate() {
            if f.app != app {
                continue;
            }
            let root = SyntaxNode::new_root(ws.green(fi));
            let Some(source) = ast::SourceFile::cast(root) else {
                continue;
            };
            for obj in source.objects() {
                onprem_method_calls(ws, fi, &obj, &mut out);
            }
        }
    }
    out
}

/// The OnPrem-only calls of one object (see [`check_context`]).
fn onprem_method_calls(ws: &Workspace, file: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    let globals: Vec<(String, String)> = obj.var_decls().flat_map(|d| typed_names(&d)).collect();
    let own_procs: Vec<String> = obj.routines().map(|r| r.name().to_ascii_lowercase()).collect();
    for r in obj.routines() {
        let Some(body) = r.body() else { continue };
        let mut vars: Vec<(String, String)> = globals.clone();
        for p in r.params() {
            if let Some(t) = p.type_ref() {
                vars.push((p.name().to_ascii_lowercase(), t.type_name().to_ascii_lowercase()));
            }
        }
        vars.extend(r.local_var_decls().flat_map(|d| typed_names(&d)));
        for call in body.descendants().filter(|n| n.kind() == SyntaxKind::CallExpr) {
            let Some(callee) = call.children().next() else { continue };
            let method = match callee.kind() {
                SyntaxKind::NameRef => {
                    let name = callee.text().to_string().trim().to_string();
                    (name.eq_ignore_ascii_case("GetLastErrorObject") && !own_procs.contains(&name.to_ascii_lowercase()))
                        .then_some(name)
                }
                SyntaxKind::MemberExpr => {
                    let mut kids = callee.children();
                    let (Some(recv), Some(m)) = (kids.next(), kids.next()) else {
                        continue;
                    };
                    if recv.kind() != SyntaxKind::NameRef {
                        continue;
                    }
                    let recv_name = recv.text().to_string().trim().trim_matches('"').to_ascii_lowercase();
                    let method = m.text().to_string().trim().to_string();
                    let ty = vars
                        .iter()
                        .rev()
                        .find(|(n, _)| *n == recv_name)
                        .map(|(_, t)| t.as_str());
                    match (ty, method.to_ascii_lowercase().as_str()) {
                        (Some("secrettext"), "unwrap") | (Some("file"), "open") => Some(method),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(method) = method {
                push(
                    out,
                    ws,
                    file,
                    offset_of(&call),
                    "AL0296",
                    format!(
                        "The application object or method '{method}' has scope 'OnPrem' and cannot be used for 'Cloud' development."
                    ),
                );
            }
        }
    }
}

/// The (lower-case name, lower-case type name) pairs a variable declaration introduces.
fn typed_names(d: &ast::VarDecl) -> Vec<(String, String)> {
    let Some(t) = d.type_ref() else { return Vec::new() };
    let ty = t.type_name().to_ascii_lowercase();
    d.names()
        .into_iter()
        .map(|n| (n.to_ascii_lowercase(), ty.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;
    use al_builtins::catalog::Catalog;

    fn load(files: &[&str]) -> Workspace {
        Workspace::load(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: files
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (format!("f{i}.al"), s.to_string()))
                    .collect(),
            }],
            &Default::default(),
        )
    }

    /// The AL codes of a compile of `files`, sorted.
    fn codes(files: &[&str]) -> Vec<String> {
        let ws = load(files);
        let m = crate::members::build(&ws);
        let r = crate::check::check(&ws, &m, &Catalog::load());
        let mut v: Vec<String> = r
            .diagnostics
            .iter()
            .filter(|d| d.code.starts_with("AL"))
            .map(|d| d.code.to_string())
            .collect();
        v.sort();
        v
    }

    fn ctx_codes(src: &str, ctx: &AppContext) -> Vec<String> {
        let ws = load(&[src]);
        check_context(&ws, 0, ctx).iter().map(|d| d.code.to_string()).collect()
    }

    const BASE: &str = "table 50000 T { fields { field(1; Id; Integer) { } } }
        page 50001 BP { PageType = Card; SourceTable = T; var Hidden: Integer; protected var Shown: Integer;
          local procedure Secret(): Integer begin exit(1); end;
          protected procedure Bump(): Integer begin exit(1); end;
          procedure Plus(a: Integer): Integer begin exit(a); end; }";

    fn ext(body: &str) -> String {
        format!("pageextension 50002 BPE extends BP {{ {body} }}")
    }

    #[test]
    fn al0162_oninit_in_a_page_extension() {
        let e = ext("trigger OnInit() begin end;");
        assert_eq!(codes(&[BASE, &e]), vec!["AL0162"]);
        // OnOpenPage is allowed.
        let ok = ext("trigger OnOpenPage() begin end;");
        assert!(codes(&[BASE, &ok]).is_empty());
        // Corpus 0017, 0018.
        let find = ext("trigger OnFindRecord(Which: Text): Boolean begin exit(true); end;");
        assert_eq!(codes(&[BASE, &find]), vec!["AL0162"]);
        let next = ext("trigger OnNextRecord(Steps: Integer): Integer begin exit(0); end;");
        assert_eq!(codes(&[BASE, &next]), vec!["AL0162"]);
    }

    #[test]
    fn al0440_extension_procedure_with_the_signature_of_a_base_procedure() {
        assert_eq!(
            codes(&[BASE, &ext("procedure Bump(): Integer begin exit(2); end;")]),
            vec!["AL0440"]
        );
        // Another parameter list is an overload, not a clash.
        assert!(codes(&[BASE, &ext("procedure Plus(a: Text): Integer begin exit(2); end;")]).is_empty());
        // A local base procedure is not reachable, so it is not a clash.
        assert!(codes(&[BASE, &ext("procedure Secret(): Integer begin exit(2); end;")]).is_empty());
    }

    #[test]
    fn al0440_second_procedure_with_the_signature_of_an_earlier_one_in_a_codeunit() {
        let dup = "codeunit 50000 C { local procedure Foo(A: Integer) begin end; local procedure foo(B: Integer) begin end; }";
        assert_eq!(codes(&[dup]), vec!["AL0440"]);
        // Another parameter type or var-ness is an overload; a table is not judged.
        let overload = "codeunit 50000 C { procedure Foo(A: Integer) begin end; procedure Foo(A: Text) begin end; procedure Foo(var A: Integer) begin end; }";
        assert!(codes(&[overload]).is_empty());
        // Corpus 0042: a report extension procedure with the signature of a protected base procedure.
        let table = "table 50001 T { fields { field(1; Id; Integer) { } } }";
        let base = "report 50002 BR { dataset { dataitem(Row; T) { column(Id; Id) { } } } protected procedure Bump(): Integer begin exit(1); end; }";
        let ext = "reportextension 50003 BRE extends BR { procedure Bump(): Integer begin exit(2); end; }";
        assert_eq!(codes(&[table, base, ext]), vec!["AL0440"]);
    }

    #[test]
    fn al0161_non_protected_variable_and_local_procedure_of_the_base() {
        let hidden = ext("trigger OnOpenPage() begin Hidden := 1; end;");
        assert_eq!(codes(&[BASE, &hidden]), vec!["AL0161"]);
        let secret = ext("trigger OnOpenPage() var V: Integer; begin V := Secret(); end;");
        assert_eq!(codes(&[BASE, &secret]), vec!["AL0161"]);
        // Protected variable and protected procedure are fine; so is the extension's own variable
        // named like a base variable.
        let ok = ext(
            "var Hidden: Integer; trigger OnOpenPage() var V: Integer; begin Shown := 1; Hidden := 1; V := Bump(); end;",
        );
        assert!(codes(&[BASE, &ok]).is_empty());
    }

    #[test]
    fn al0118_bare_call_of_an_unknown_name_in_a_codeunit() {
        let cu = "codeunit 50000 C { procedure Make(): Guid begin exit(CreateSequentialGuid()); end; }";
        assert_eq!(codes(&[cu]), vec!["AL0118"]);
        let ok = "codeunit 50000 C { procedure Make(): Guid begin exit(CreateGuid()); end; procedure Other() begin Make(); end; }";
        assert!(codes(&[ok]).is_empty());
    }

    #[test]
    fn al0118_bare_call_of_an_unknown_name_in_a_table_page_report_and_page_extension() {
        let t = "table 50000 T { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } }
            procedure Run() begin NoSuchProcedure(); end; }";
        assert_eq!(codes(&[t]), vec!["AL0118"]);
        let t = "table 50000 T { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }";
        let page =
            "page 50001 P { PageType = Card; SourceTable = T; trigger OnOpenPage() begin NoSuchProcedure(); end; }";
        assert_eq!(codes(&[t, page]), vec!["AL0118"]);
        let report = "report 50002 R { dataset { dataitem(Row; T) { column(Id; Id) { } } } trigger OnPreReport() begin NoSuchProcedure(); end; }";
        assert_eq!(codes(&[t, report]), vec!["AL0118"]);
        let ext = ext("trigger OnOpenPage() begin NoSuchProcedure(); end;");
        assert_eq!(codes(&[BASE, &ext]), vec!["AL0118"]);
        // What a table, page or report can call is not judged: an own procedure, an implicit record method,
        // a page method, a system function.
        let ok = "table 50000 T { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } }
            procedure Own() begin end;
            procedure Run() begin Own(); Init(); SetRange(Id, 1); Message('x'); end; }";
        assert!(codes(&[ok]).is_empty());
        let ok_page = "page 50001 P { PageType = Card; SourceTable = T; trigger OnOpenPage() begin SetRange(Id, 1); Update(false); end; }";
        assert!(codes(&[t, ok_page]).is_empty());
    }

    #[test]
    fn al0118_is_not_given_for_the_codeunits_own_run() {
        // A bare `Run()` in a codeunit runs the codeunit itself (a method of the Codeunit type, called on the
        // running instance); real BC compiles it (NP Retail's scanner import codeunits do).
        let cu = "codeunit 50000 C { trigger OnRun() begin end;
            procedure Go() begin if not Run() then Message('failed'); end; }";
        assert!(codes(&[cu]).is_empty(), "{:?}", codes(&[cu]));
        // A name that is a method of no type is still refused.
        let bad = "codeunit 50000 C { procedure Go() begin NoSuchProcedure(); end; }";
        assert_eq!(codes(&[bad]), vec!["AL0118"]);
    }

    #[test]
    fn al0118_is_not_given_for_a_procedure_of_a_sibling_page_extension() {
        // Microsoft's test library calls `RefreshFactboxes`, an `internal procedure` of another page extension
        // of the same base page (Tests-TestLibraries UsersPBTTest.PageExt.al, Base Application
        // UsersPerfFactboxes.PageExt.al); real BC compiles it.
        let sibling = "pageextension 50003 BPE2 extends BP { internal procedure Refresh(a: Integer) begin end; }";
        let caller = "pageextension 50002 BPE extends BP { trigger OnOpenPage() begin Refresh(1); end; }";
        assert!(
            codes(&[BASE, sibling, caller]).is_empty(),
            "{:?}",
            codes(&[BASE, sibling, caller])
        );
        // Without the sibling the name is unknown.
        assert_eq!(codes(&[BASE, caller]), vec!["AL0118"]);
    }

    #[test]
    fn al0118_rec_in_a_codeunit_without_table_no() {
        let cu = "table 50000 T { fields { field(1; Id; Integer) { } } }
            codeunit 50001 C { procedure P() begin Rec.Id := 1; end; }";
        assert_eq!(codes(&[cu]), vec!["AL0118"]);
        let ok = "table 50000 T { fields { field(1; Id; Integer) { } } }
            codeunit 50001 C { TableNo = T; trigger OnRun() begin Rec.Id := 1; end; }";
        assert!(codes(&[ok]).is_empty());
    }

    #[test]
    fn al0132_record_field_or_method_that_does_not_exist() {
        let t = "table 50000 T { fields { field(1; Id; Integer) { } } }";
        let body = |stmt: &str| format!("codeunit 50001 C {{ procedure P() var R: Record T; begin {stmt} end; }}");
        assert_eq!(codes(&[t, &body("R.NoSuchField := 1;")]), vec!["AL0132"]);
        assert_eq!(codes(&[t, &body("R.NoSuchMethod();")]), vec!["AL0132"]);
        assert!(codes(&[t, &body("R.Id := 1; R.Init(); R.SetRange(Id, 1);")]).is_empty());
        // A table that an extension adds to is not judged.
        let ext = "tableextension 50002 TE extends T { fields { field(50000; Extra; Integer) { } } }";
        assert!(codes(&[t, ext, &body("R.Extra := 1;")]).is_empty());
    }

    #[test]
    fn al0161_in_a_report_extension() {
        let t = "table 50000 T { fields { field(1; Id; Integer) { } } }";
        let base = "report 50001 BR { dataset { dataitem(Row; T) { column(Id; Id) { } } } var Hidden: Integer;
            local procedure SecretLocal(): Integer begin exit(1); end; protected var Shown: Integer; }";
        let var = "reportextension 50002 E extends BR { trigger OnPreReport() begin Hidden := 1; end; }";
        assert_eq!(codes(&[t, base, var]), vec!["AL0161"]);
        let proc_ = "reportextension 50002 E extends BR { trigger OnPreReport() var V: Integer; begin V := SecretLocal(); end; }";
        assert_eq!(codes(&[t, base, proc_]), vec!["AL0161"]);
        let ok = "reportextension 50002 E extends BR { trigger OnPreReport() begin Shown := 1; end; }";
        assert!(codes(&[t, base, ok]).is_empty());
    }

    #[test]
    fn al0132_execution_mode_members_that_do_not_exist() {
        for m in ["Install", "Uninstall", "Upgrade", "Report", "Profile"] {
            let cu =
                format!("codeunit 50000 C {{ procedure M(): ExecutionMode begin exit(ExecutionMode::{m}); end; }}");
            assert_eq!(codes(&[&cu]), vec!["AL0132"], "{m}");
        }
        let ok = "codeunit 50000 C { procedure M(): ExecutionMode begin exit(ExecutionMode::Standard); end; }";
        assert!(codes(&[ok]).is_empty());
    }

    fn enum_pair(src_compat: bool, dst_compat: bool) -> String {
        format!(
            "enum 50000 \"MNC Src\" {{ Extensible = true; AssignmentCompatibility = {src_compat}; value(0; None) {{ }} value(1; One) {{ }} value(7; Seven) {{ }} }}
             enum 50001 \"MNC Dst\" {{ Extensible = true; AssignmentCompatibility = {dst_compat}; value(0; None) {{ }} value(1; One) {{ }} value(2; Two) {{ }} }}
             codeunit 50002 \"MNC Assign\" {{ procedure Do(): Integer var S: Enum \"MNC Src\"; D: Enum \"MNC Dst\"; begin D := S; exit(D.AsInteger()); end; }}"
        )
    }

    /// The AL diagnostics of a compile of `src`, as `code message`.
    fn messages(src: &str) -> Vec<String> {
        let ws = load(&[src]);
        let m = crate::members::build(&ws);
        crate::check::check(&ws, &m, &Catalog::load())
            .diagnostics
            .iter()
            .filter(|d| d.code.starts_with("AL"))
            .map(|d| format!("{} {}", d.code, d.message))
            .collect()
    }

    // must-not-compile 0074 (only the source states AssignmentCompatibility) and 0075 (neither does):
    // AL0122. A target that states it compiles whatever the source says
    // (types/PXEnumRecordRef.al:7-10, :74-108); so does an Integer into any enum (:11-12, :137-196).
    #[test]
    fn al0122_an_enum_of_another_type_into_a_target_without_assignment_compatibility() {
        let want = vec!["AL0122 Cannot implicitly convert type 'Enum \"MNC Src\"' to 'Enum \"MNC Dst\"'".to_string()];
        assert_eq!(messages(&enum_pair(true, false)), want, "0074");
        assert_eq!(messages(&enum_pair(false, false)), want, "0075");
        assert!(messages(&enum_pair(false, true)).is_empty());
        assert!(messages(&enum_pair(true, true)).is_empty());
        // The property left out (false by default), into a table field.
        let field = "enum 50000 \"MNC Src\" { value(0; None) { } value(1; One) { } }
             enum 50001 \"MNC Dst\" { value(0; None) { } value(1; One) { } }
             table 50003 \"MNC Row\" { fields { field(1; Id; Integer) { } field(2; Kind; Enum \"MNC Dst\") { } } }
             codeunit 50002 \"MNC Assign\" { procedure Do() var S: Enum \"MNC Src\"; R: Record \"MNC Row\"; begin R.Kind := S; end; }";
        assert_eq!(messages(field), want, "field");
        let same = "enum 50001 \"MNC Dst\" { value(0; None) { } value(1; One) { } }
             codeunit 50002 \"MNC Assign\" { procedure Do() var D: Enum \"MNC Dst\"; E: Enum \"MNC Dst\"; I: Integer; begin D := E; I := 1; D := I; end; }";
        assert!(messages(same).is_empty());
    }

    // must-not-compile 0069 (`T := I`, an Integer variable into a Text variable) and 0070 (`I := 'text'`, a
    // text literal into an Integer variable): AL0122. A one-character literal is a Char, which an Integer takes;
    // the other shapes (a Text variable into an Integer, an Integer into a Code) are not judged.
    #[test]
    fn al0122_integer_into_text_and_text_literal_into_integer() {
        let cu = |body: &str| {
            format!(
                "codeunit 50001 \"MNC Codeunit\" {{ procedure DoIt() var I: Integer; T: Text; L: Text[30]; C: Code[20]; begin {body} end; }}"
            )
        };
        assert_eq!(
            messages(&cu("T := I;")),
            vec!["AL0122 Cannot implicitly convert type 'Integer' to 'Text'".to_string()]
        );
        assert_eq!(
            messages(&cu("I := 'text';")),
            vec!["AL0122 Cannot implicitly convert type 'Text' to 'Integer'".to_string()]
        );
        assert_eq!(messages(&cu("L := I;")).len(), 1);
        for ok in [
            "I := 'A';",
            "T := Format(I);",
            "I := 5; T := 'x';",
            "C := 'x';",
            "I := T.IndexOf('x');",
        ] {
            assert!(messages(&cu(ok)).is_empty(), "{ok}");
        }
    }

    // must-not-compile 0500 (corpus tests/must-not-compile/cases/0500-char-minus-text.al:1-2): `C - T` and `T - C`
    // with a Char variable and an unbounded Text variable. AL0175. A Text literal with a Char compiles (an
    // Integer); Text[30], Code and other shapes are not pinned and not judged.
    #[test]
    fn al0175_char_minus_text_variable() {
        let cu = |body: &str| {
            format!(
                "codeunit 50001 \"MNC Codeunit\" {{ procedure DoIt() var C: Char; T: Text; L: Text[30]; I: Integer; begin {body} end; }}"
            )
        };
        assert_eq!(
            messages(&cu("I := C - T;")),
            vec!["AL0175 Operator '-' cannot be applied to operands of type 'Char' and 'Text'".to_string()]
        );
        assert_eq!(
            messages(&cu("I := T - C;")),
            vec!["AL0175 Operator '-' cannot be applied to operands of type 'Text' and 'Char'".to_string()]
        );
        for ok in [
            "I := C - 'a';",
            "I := 'a' - C;",
            "I := C - I;",
            "I := C - C;",
            "I := C - L;",
            "T := T + T;",
        ] {
            assert!(messages(&cu(ok)).iter().all(|m| !m.starts_with("AL0175")), "{ok}");
        }
    }

    // must-not-compile 0501: a Text literal and a Text variable assigned to a Decimal. AL0122.
    #[test]
    fn al0122_text_into_decimal() {
        let cu = |body: &str| {
            format!(
                "codeunit 50001 \"MNC Codeunit\" {{ procedure DoIt() var D: Decimal; T: Text; V: Variant; begin {body} end; }}"
            )
        };
        let want = "AL0122 Cannot implicitly convert type 'Text' to 'Decimal'".to_string();
        assert_eq!(messages(&cu("D := '100';")), vec![want.clone()]);
        assert_eq!(messages(&cu("D := T;")), vec![want]);
        for ok in ["D := V;", "Evaluate(D, T);", "D := 5;"] {
            assert!(messages(&cu(ok)).is_empty(), "{ok}");
        }
    }

    #[test]
    fn al0197_every_declaration_of_a_repeated_name_is_reported() {
        let cu = "codeunit 50000 \"Same\" { } codeunit 50001 \"same\" { }";
        assert_eq!(codes(&[cu]), vec!["AL0197", "AL0197"]);
        // Another object type with the name is fine.
        let ok = "codeunit 50000 \"Same\" { } table 50001 \"Same\" { fields { field(1; Id; Integer) { } } }";
        assert!(codes(&[ok]).is_empty());
    }

    const PARENT: &str = "table 50000 P { fields { field(1; Id; Integer) { } field(2; Pid; Integer) { } } }";

    #[test]
    fn al0365_crossjoin_with_a_data_item_link() {
        let q = |props: &str| {
            format!(
                "query 50002 Q {{ elements {{ dataitem(Parent; P) {{ column(Id; Id) {{ }}
                   dataitem(Child; P) {{ {props} column(CId; Id) {{ }} }} }} }} }}"
            )
        };
        assert_eq!(
            codes(&[PARENT, &q("SqlJoinType = CrossJoin; DataItemLink = Pid = Parent.Id;")]),
            vec!["AL0365"]
        );
        assert!(codes(&[PARENT, &q("SqlJoinType = InnerJoin; DataItemLink = Pid = Parent.Id;")]).is_empty());
        assert!(codes(&[PARENT, &q("SqlJoinType = CrossJoin;")]).is_empty());
    }

    #[test]
    fn al0385_include_caption_on_an_expression_column() {
        let r = |col: &str| format!("report 50001 R {{ dataset {{ dataitem(Row; P) {{ {col} }} }} }}");
        assert_eq!(
            codes(&[PARENT, &r("column(IdText; Format(Id)) { IncludeCaption = true; }")]),
            vec!["AL0385"]
        );
        assert!(codes(&[PARENT, &r("column(IdText; Id) { IncludeCaption = true; }")]).is_empty());
        // An indexed variable's field is a named field (Base Application's "Quantity Explosion of BOM").
        let indexed =
            "report 50001 R { dataset { dataitem(Row; P) { column(IdText; Arr[Level].Id) { IncludeCaption = true; } } }
            var Arr: array[3] of Record P; Level: Integer; }";
        assert!(codes(&[PARENT, indexed]).is_empty(), "{:?}", codes(&[PARENT, indexed]));
        assert!(codes(&[PARENT, &r("column(IdText; Format(Id)) { }")]).is_empty());
    }

    fn ctx(target: AppTarget, runtime: Option<(u32, u32)>, keys: &[&str]) -> AppContext {
        AppContext {
            target,
            runtime,
            app_json_keys: keys.iter().map(|k| k.to_string()).collect(),
            bc_major: None,
        }
    }

    #[test]
    fn al0296_al0161_and_al0433_for_the_platform_tables_of_bc_27_only() {
        let cu = |t: &str| {
            format!("codeunit 50000 C {{ procedure P(): Integer var R: Record \"{t}\"; begin exit(R.Count()); end; }}")
        };
        let at = |major, t: &str| {
            let mut c = ctx(AppTarget::Cloud, None, &[]);
            c.bc_major = Some(major);
            let mut v = ctx_codes(&cu(t), &c);
            v.sort();
            v
        };
        assert_eq!(at(27, "Designed Query"), vec!["AL0296"]);
        assert_eq!(at(27, "Agent Data"), vec!["AL0161", "AL0296"]);
        assert_eq!(at(27, "Profile"), vec!["AL0296", "AL0433"]);
        // Not pinned for another major, and a table that is not on the list is not judged.
        assert!(at(28, "Designed Query").is_empty());
        assert!(at(27, "Company").is_empty());
        // Cases 0006 and 0014 hold for every major.
        assert_eq!(at(28, "Published Application"), vec!["AL0296"]);
    }

    #[test]
    fn al0296_cloud_app_names_an_onprem_platform_table() {
        let cu = "codeunit 50000 C { procedure P(): Integer var R: Record \"Published Application\"; begin exit(R.Count()); end; }";
        assert_eq!(ctx_codes(cu, &ctx(AppTarget::Cloud, None, &[])), vec!["AL0296"]);
        assert!(ctx_codes(cu, &ctx(AppTarget::OnPrem, None, &[])).is_empty());
        let other =
            "codeunit 50000 C { procedure P(): Integer var R: Record \"Company\"; begin exit(R.Count()); end; }";
        assert!(ctx_codes(other, &ctx(AppTarget::Cloud, None, &[])).is_empty());
    }

    #[test]
    fn al0124_assembly_probing_paths_at_runtime_16() {
        let cu = "codeunit 50000 C { }";
        let keys = ["assemblyProbingPaths"];
        assert_eq!(
            ctx_codes(cu, &ctx(AppTarget::Cloud, Some((16, 0)), &keys)),
            vec!["AL0124"]
        );
        for r in [(15, 0), (16, 1), (17, 0), (17, 1), (18, 0)] {
            assert_eq!(
                ctx_codes(cu, &ctx(AppTarget::Cloud, Some(r), &keys)),
                vec!["AL0124"],
                "{r:?}"
            );
        }
        // Another runtime, another target and no key are not judged.
        assert!(ctx_codes(cu, &ctx(AppTarget::Cloud, Some((19, 0)), &keys)).is_empty());
        assert!(ctx_codes(cu, &ctx(AppTarget::OnPrem, Some((16, 0)), &keys)).is_empty());
        assert!(ctx_codes(cu, &ctx(AppTarget::Cloud, Some((16, 0)), &[])).is_empty());
        assert!(ctx_codes(cu, &ctx(AppTarget::Cloud, None, &keys)).is_empty());
    }

    #[test]
    fn every_implemented_code_is_listed_once() {
        let mut v: Vec<_> = IMPLEMENTED.iter().map(|(c, _)| *c).collect();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), IMPLEMENTED.len());
        assert_eq!(not_checked_reason("AL0161"), None);
        assert_eq!(not_checked_reason("AL9999"), Some("not implemented"));
    }

    fn cloud() -> AppContext {
        AppContext {
            target: AppTarget::Cloud,
            runtime: Some((16, 0)),
            app_json_keys: Vec::new(),
            bc_major: Some(27),
        }
    }

    #[test]
    fn al0296_a_cloud_app_calls_a_method_with_scope_onprem() {
        let last = "codeunit 50000 C { procedure P() var I: ErrorInfo; begin I := GetLastErrorObject(); end; }";
        assert_eq!(ctx_codes(last, &cloud()), vec!["AL0296"]);
        // A procedure of the app with that name is not the platform's.
        let own = "codeunit 50000 C { procedure GetLastErrorObject(): Integer begin end; procedure P() begin GetLastErrorObject(); end; }";
        assert!(ctx_codes(own, &cloud()).is_empty());
        let unwrap = "codeunit 50000 C { procedure P(S: SecretText): Text begin exit(S.Unwrap()); end; }";
        assert_eq!(ctx_codes(unwrap, &cloud()), vec!["AL0296"]);
        let file = "codeunit 50000 C { procedure P(): Boolean var F: File; begin exit(F.Open('x')); end; }";
        assert_eq!(ctx_codes(file, &cloud()), vec!["AL0296"]);
        // Another type's Unwrap, and an OnPrem app, are fine.
        let other = "codeunit 50000 C { procedure P(S: Text): Text begin exit(S.Unwrap()); end; }";
        assert!(ctx_codes(other, &cloud()).is_empty());
        let onprem = AppContext {
            target: AppTarget::OnPrem,
            ..cloud()
        };
        assert!(ctx_codes(file, &onprem).is_empty());
    }

    #[test]
    fn al0124_orderby_on_a_query_data_item_and_al0132_length_on_a_text() {
        let q = "table 50000 T { fields { field(1; Id; Integer) { } } }
            query 50001 Q { elements { dataitem(Row; T) { OrderBy = ascending(Id); column(Id; Id) { } } } }";
        assert_eq!(codes(&[q]), vec!["AL0124"]);
        let len = "codeunit 50000 C { procedure P(): Integer var V: Text; begin exit(V.Length()); end; }";
        assert_eq!(codes(&[len]), vec!["AL0132"]);
        let strlen = "codeunit 50000 C { procedure P(): Integer var V: Text; begin exit(StrLen(V)); end; }";
        assert!(codes(&[strlen]).is_empty());
    }

    #[test]
    fn al0161_this_names_a_base_variable_that_is_not_protected() {
        let base = "table 50000 T { fields { field(1; Id; Integer) { } } }
            report 50001 R { dataset { dataitem(Row; T) { } } var BaseVar: Text; protected var Shown: Text; }";
        let bad = "reportextension 50002 E extends R { trigger OnPreReport() begin this.BaseVar := 'x'; end; }";
        assert_eq!(codes(&[base, bad]), vec!["AL0161"]);
        let ok = "reportextension 50002 E extends R { trigger OnPreReport() begin Message(this.Shown); end; }";
        assert!(codes(&[base, ok]).is_empty());
    }
}
