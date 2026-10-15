//! Compile errors with the AL compiler's own codes (`AL0xxx`), for the must-not-compile corpus
//! (`tests/must-not-compile` of the corpus repo, branch `al-runtime-rs`; ADR-009 and the owner
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
//! The codes the checker itself reports (`check.rs`, where types are known) are AL0118, AL0122,
//! AL0132 and AL0161; they are listed in [`IMPLEMENTED`] too.

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
        "an enum assigned to an enum of another type whose target does not state AssignmentCompatibility = true",
    ),
    ("AL0124", "app.json key `assemblyProbingPaths` at runtime 16.0"),
    ("AL0132", "ExecutionMode::Install, Uninstall, Upgrade, Report"),
    (
        "AL0161",
        "a page extension names a non-protected variable or a local procedure of its base page",
    ),
    ("AL0162", "OnInit in a page extension"),
    ("AL0197", "two objects of one type and name in one app"),
    ("AL0296", "a Cloud app names a platform table that is Scope = OnPrem"),
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
        "a page extension procedure with the signature of a base page procedure",
    ),
];

/// Codes the must-not-compile corpus expects and this front end cannot decide, with the reason.
pub const NOT_CHECKED: &[(&str, &str)] = &[(
    "AL0175",
    "needs the .NET type behind the alias (an enum is comparable with `=`, a class object is not); the front end has no assembly metadata",
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

/// What the compiler knows of an app besides its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppContext {
    pub target: AppTarget,
    /// `runtime` of app.json as (major, minor).
    pub runtime: Option<(u32, u32)>,
    /// The top-level keys of app.json.
    pub app_json_keys: Vec<String>,
}

/// Platform tables declared `Scope = OnPrem`. Pinned by the must-not-compile corpus: cases 0006
/// (`Published Application`) and 0014 (`API Webhook Entity`), BC 27.5 compiler. Other platform
/// tables may be OnPrem too; none is guessed (docs/probes-needed.md).
pub const ONPREM_PLATFORM_TABLES: &[&str] = &["Published Application", "API Webhook Entity"];

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
    for oi in 0..ws.objects.len() {
        let o = &ws.objects[oi];
        if ws.kinds[o.app] == AppKind::Dependency {
            continue;
        }
        let obj = ws.object_ast(oi);
        match o.kind {
            ObjKind::PageExtension => {
                page_extension_triggers(ws, oi, &obj, &mut out);
                page_extension_procedures(ws, oi, &obj, &mut out);
            }
            ObjKind::Query => query_joins(ws, o.file, obj.sections(), &mut out),
            ObjKind::Report => report_columns(ws, o.file, obj.sections(), &mut out),
            _ => {}
        }
    }
    out
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

/// AL0162. A page has the trigger OnInit; a page extension does not (corpus 0004). Only OnInit is
/// refused: the other trigger names a page extension may not declare are not pinned.
fn page_extension_triggers(ws: &Workspace, oi: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    for t in obj.triggers() {
        if t.name().eq_ignore_ascii_case("OnInit") {
            push(
                out,
                ws,
                ws.objects[oi].file,
                t.keyword_offset().unwrap_or_else(|| offset_of(t.syntax())),
                "AL0162",
                "'OnInit' is not a valid trigger".to_string(),
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

/// AL0440. A page extension procedure with the name and parameter types of a procedure of its
/// base page that it could otherwise reach (not `local`), corpus 0003 and
/// handlers/TestPageExtensionBaseAccessContracts.al:20.
fn page_extension_procedures(ws: &Workspace, oi: usize, obj: &ast::Object, out: &mut Vec<Diagnostic>) {
    let Some(base) = ws.extension_target(oi) else { return };
    if ws.objects[base].kind != ObjKind::Page {
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
                    "The Page '{}' already defines a method called '{name}' with the same parameter types in '{}'.",
                    ws.objects[base].name, ws.apps[ws.objects[base].app].name
                ),
            );
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
            if include && s.arg_name_parts(1).is_none() && s.args().len() >= 2 {
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
    // AL0124 (corpus 0007): app.json at runtime 16.0 does not take `assemblyProbingPaths`.
    // Only runtime 16.0 is pinned.
    if ctx.runtime == Some((16, 0)) && ctx.app_json_keys.iter().any(|k| k == "assemblyProbingPaths") {
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
    // AL0296 (corpus 0006, 0014): a Cloud app names a platform table that is Scope = OnPrem.
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
                let Some(table) = ONPREM_PLATFORM_TABLES.iter().find(|x| x.eq_ignore_ascii_case(&name)) else {
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
            }
        }
    }
    out
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
    fn al0132_execution_mode_members_that_do_not_exist() {
        for m in ["Install", "Uninstall", "Upgrade", "Report"] {
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

    // must-not-compile 0074 (only the source states AssignmentCompatibility) and 0075 (neither does), corpus
    // branch al-runtime-rs-staging: AL0122. A target that states it compiles whatever the source says
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
        assert!(codes(&[PARENT, &r("column(IdText; Format(Id)) { }")]).is_empty());
    }

    fn ctx(target: AppTarget, runtime: Option<(u32, u32)>, keys: &[&str]) -> AppContext {
        AppContext {
            target,
            runtime,
            app_json_keys: keys.iter().map(|k| k.to_string()).collect(),
        }
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
}
