//! The tables that list the compiled objects: AllObj, AllObjWithCaption, CodeUnit Metadata and
//! Table Metadata. Their rows come from the program, which holds every object of every loaded
//! app: the ones the checker reached are compiled, the others are shells that keep their id,
//! name, namespace, owning app and the properties written in the object body (ADR-013).
//!
//! The corpus pins these in record/TestAllObjVirtualTable.al, TestAllObjOwningAppPerKind.al,
//! TestCodeunitMetadataVirtualTable.al, TestCodeunitInventoryOrder.al and
//! TestTableMetadataVirtualTable.al. AL Runner measured the rest by decompiling the platform's
//! providers (`docs/virtual-tables-allobj.md` there); where a column is not pinned by the corpus
//! the rule used is stated at the column.

use super::mat::Entry;
use super::{Blank, Providers, ids, option, pos};
use crate::Engine;
use al_builtins::value::{RtType, Value};
use al_data::{Row, TableDef};
use al_ir::Object;
use std::collections::HashMap;

/// The option ordinal of the AllObj "Object Type" member for an object kind, if the kind is listed.
/// Members: TableData=0, Table=1, Report=3, Codeunit=5, XMLport=6, MenuSuite=7, Page=8, Query=9,
/// System=10, FieldNumber=11, PageExtension=14, TableExtension=15, Enum=16, EnumExtension=17,
/// Profile=18, ProfileExtension=19, PermissionSet=20, PermissionSetExtension=21,
/// ReportExtension=22 (the declaration of the AllObj table in the platform app).
pub(super) fn object_type(kind: al_ir::ObjKind) -> Option<i32> {
    Some(match kind {
        al_ir::ObjKind::Table => 1,
        al_ir::ObjKind::Report => 3,
        al_ir::ObjKind::Codeunit => 5,
        al_ir::ObjKind::XmlPort => 6,
        al_ir::ObjKind::Page => 8,
        al_ir::ObjKind::Query => 9,
        al_ir::ObjKind::PageExtension => 14,
        al_ir::ObjKind::TableExtension => 15,
        al_ir::ObjKind::Enum => 16,
        al_ir::ObjKind::EnumExtension => 17,
        al_ir::ObjKind::Profile => 18,
        al_ir::ObjKind::PermissionSet => 20,
        al_ir::ObjKind::PermissionSetExtension => 21,
        al_ir::ObjKind::ReportExtension => 22,
        _ => return None,
    })
}

/// Whether AllObjWithCaption fills "App ID" for the kind: tables, reports, xmlports, pages, queries,
/// codeunits and the extension kinds, not enums, permission sets or profiles (corpus
/// record/TestAllObjVirtualTable.al `AllObjWithCaption_FindEnum_BaseApplicationEnum_AppIdIsEmpty`
/// pins the enum; the rest is AL Runner's reading of the platform provider).
fn carries_app_id(kind: al_ir::ObjKind) -> bool {
    matches!(
        kind,
        al_ir::ObjKind::Table
            | al_ir::ObjKind::Report
            | al_ir::ObjKind::XmlPort
            | al_ir::ObjKind::Page
            | al_ir::ObjKind::Query
            | al_ir::ObjKind::Codeunit
            | al_ir::ObjKind::PageExtension
            | al_ir::ObjKind::TableExtension
            | al_ir::ObjKind::EnumExtension
            | al_ir::ObjKind::PermissionSetExtension
            | al_ir::ObjKind::ReportExtension
    )
}

/// The value of property `name` as written in the object body.
pub(super) fn prop<'a>(o: &'a Object, name: &str) -> Option<&'a str> {
    o.properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// `"Quoted Name"` or `Name` as the name it spells.
pub(super) fn ident(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v)
        .to_string()
}

/// The first string literal of a property value: `'It''s'` is `It's`. A value that does not start
/// with a literal has none.
pub(super) fn string_lit(v: &str) -> Option<String> {
    let rest = v.trim().strip_prefix('\'')?;
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                out.push('\'');
                chars.next();
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

pub(super) fn truthy(v: &str) -> bool {
    v.trim().eq_ignore_ascii_case("true")
}

/// The member of `members` that `text` spells, case-insensitively, as the member is written.
pub(super) fn member<'a>(members: &[&'a str], text: &str) -> Option<&'a str> {
    let t = ident(text);
    members.iter().find(|m| m.eq_ignore_ascii_case(&t)).copied()
}

const PAGE_TYPES: &[&str] = &[
    "Card",
    "List",
    "RoleCenter",
    "CardPart",
    "ListPart",
    "Document",
    "Worksheet",
    "ListPlus",
    "ConfirmationDialog",
    "NavigatePage",
    "StandardDialog",
    "API",
    "HeadlinePart",
    "XmlPort",
    "PromptDialog",
    "ReportPreview",
    "ReportProcessingOnly",
    "UserControlHost",
];
const TABLE_TYPES: &[&str] = &[
    "Normal",
    "CRM",
    "ExternalSQL",
    "Exchange",
    "MicrosoftGraph",
    "Query",
    "Temporary",
];
const QUERY_TYPES: &[&str] = &["Normal", "API"];
const CODEUNIT_SUBTYPES: &[&str] = &["Normal", "Test", "TestRunner", "Upgrade", "Install"];

/// What the program says about each object, by name and by owner.
pub(super) struct Catalog<'a> {
    pub objects: &'a [Object],
    pub apps: &'a [al_ir::AppInfo],
    /// (base kind, lower-case name) to the ids of the objects of that name.
    names: HashMap<(al_ir::ObjKind, String), Vec<u32>>,
    /// Table id to the first table object of that id.
    tables: HashMap<u32, &'a Object>,
    /// Page id to the page extensions that extend it, in program order.
    page_extensions: HashMap<u32, Vec<&'a Object>>,
}

impl<'a> Catalog<'a> {
    pub(super) fn new(engine: &'a Engine) -> Catalog<'a> {
        let program = &engine.program;
        let mut names: HashMap<(al_ir::ObjKind, String), Vec<u32>> = HashMap::new();
        let mut tables: HashMap<u32, &Object> = HashMap::new();
        let mut page_extensions: HashMap<u32, Vec<&Object>> = HashMap::new();
        for o in &program.objects {
            if let (al_ir::ObjKind::PageExtension, Some(base)) = (o.kind, o.extends_id) {
                page_extensions.entry(base).or_default().push(o);
            }
            if let Some(id) = o.id {
                names.entry((o.kind, o.name.to_lowercase())).or_default().push(id);
                if o.kind == al_ir::ObjKind::Table {
                    tables.entry(id).or_insert(o);
                }
            }
        }
        Catalog {
            objects: &program.objects,
            apps: &program.apps,
            names,
            tables,
            page_extensions,
        }
    }

    /// The page extensions of page `id`, in program order (none for `None`).
    pub(super) fn page_extensions_of(&self, id: Option<u32>) -> &[&'a Object] {
        id.and_then(|id| self.page_extensions.get(&id))
            .map_or(&[], Vec::as_slice)
    }

    /// The first table object with this id.
    pub(super) fn table_by_id(&self, id: u32) -> Option<&'a Object> {
        self.tables.get(&id).copied()
    }

    /// The manifest id of the app that owns `o`; empty for the platform's own objects, which no
    /// app owns (AL Runner leaves them without an owner).
    pub(super) fn owner(&self, o: &Object) -> [u8; 16] {
        let Some(app) = self.apps.get(o.app as usize) else {
            return [0; 16];
        };
        if app.id.eq_ignore_ascii_case(al_sema_platform_app_id()) {
            return [0; 16];
        }
        al_builtins::ops::parse_guid(&app.id).unwrap_or([0; 16])
    }

    /// The id of the object of `kind` that a property value names: a number, `Page::"X"` or a name.
    pub(super) fn id_of(&self, kind: al_ir::ObjKind, value: &str) -> Result<u32, String> {
        let v = value.trim();
        let v = v.split_once("::").map_or(v, |(_, rest)| rest);
        if let Ok(n) = v.parse::<u32>() {
            return Ok(n);
        }
        let name = ident(v);
        match self.names.get(&(kind, name.to_lowercase())).map(Vec::as_slice) {
            Some([id]) => Ok(*id),
            Some([]) | None => Err(format!("no {kind} named `{name}`")),
            Some(_) => Err(format!("more than one {kind} is named `{name}`")),
        }
    }
}

pub(super) fn al_sema_platform_app_id() -> &'static str {
    "8874ed3a-0643-4247-9ced-7a7002f7135d"
}

/// The caption a declaration states: the `Caption` literal, else the `ENU` entry of `CaptionML`, else the
/// name (AL's default caption). A caption that is not a literal is not known.
pub(crate) fn declared_caption(caption: Option<&str>, caption_ml: Option<&str>, name: &str) -> Result<String, String> {
    if let Some(c) = caption {
        return string_lit(c).ok_or_else(|| format!("Caption `{c}` is not a literal"));
    }
    if let Some(ml) = caption_ml {
        let enu = split_outside_quotes(ml)
            .iter()
            .find_map(|part| {
                let part = part.trim();
                let (lang, text) = part.split_once('=')?;
                lang.trim().eq_ignore_ascii_case("ENU").then(|| text.trim())
            })
            .and_then(string_lit);
        return enu.ok_or_else(|| format!("CaptionML `{ml}` has no ENU literal"));
    }
    Ok(name.to_string())
}

/// The entry for language code `lang` (`ENU`, `DEU`) of an ML property such as `SummaryML = ENU = 'x', DEU = 'y'`: `Ok(None)` when it has
/// none, `Err` when the property is not a list of literals.
pub(crate) fn ml_entry(ml: &str, lang: &str) -> Result<Option<String>, String> {
    for part in split_outside_quotes(ml) {
        let Some((code, text)) = part.trim().split_once('=') else {
            return Err(format!("`{ml}` is not a list of language = text entries"));
        };
        if code.trim().eq_ignore_ascii_case(lang) {
            return string_lit(text.trim())
                .map(Some)
                .ok_or_else(|| format!("`{ml}`: the {lang} entry is not a literal"));
        }
    }
    Ok(None)
}

/// `a='x,y',b='z'` split at the commas outside single quotes.
fn split_outside_quotes(s: &str) -> Vec<&str> {
    let (mut out, mut start, mut quoted) = (Vec::new(), 0, false);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

pub(super) fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The object caption: the `Caption` property's literal, else the object name.
pub(super) fn caption(o: &Object) -> String {
    prop(o, "Caption")
        .and_then(string_lit)
        .unwrap_or_else(|| o.name.clone())
}

fn namespace(o: &Object) -> String {
    o.namespace.clone().unwrap_or_default()
}

// ---- AllObj and AllObjWithCaption -----------------------------------------------------------------

/// The text of "Object Subtype" for an object (AllObjWithCaption).
fn subtype_text(c: &Catalog, o: &Object) -> Result<String, String> {
    let kind = o.kind;
    let unknown = |what: &str, v: &str| format!("{kind} '{}': {what} `{v}` is not understood", o.name);
    Ok(match kind {
        al_ir::ObjKind::Table => match prop(o, "TableType") {
            None => "Normal".into(),
            Some(v) => member(TABLE_TYPES, v).ok_or_else(|| unknown("TableType", v))?.into(),
        },
        al_ir::ObjKind::Page => match prop(o, "PageType") {
            None => "Card".into(),
            Some(v) => member(PAGE_TYPES, v).ok_or_else(|| unknown("PageType", v))?.into(),
        },
        al_ir::ObjKind::Query => match prop(o, "QueryType") {
            None => "Normal".into(),
            Some(v) => member(QUERY_TYPES, v).ok_or_else(|| unknown("QueryType", v))?.into(),
        },
        // A codeunit's Normal subtype is the empty string, and so is Install, which the compiler
        // writes into the metadata as Normal (TestAllObjVirtualTable.al, the codeunit and install cases).
        al_ir::ObjKind::Codeunit => match prop(o, "Subtype") {
            None => String::new(),
            Some(v) => match member(CODEUNIT_SUBTYPES, v).ok_or_else(|| unknown("Subtype", v))? {
                "Normal" | "Install" => String::new(),
                other => other.into(),
            },
        },
        // The id of the object the extension extends, as a decimal string.
        al_ir::ObjKind::PageExtension
        | al_ir::ObjKind::TableExtension
        | al_ir::ObjKind::EnumExtension
        | al_ir::ObjKind::PermissionSetExtension
        | al_ir::ObjKind::ReportExtension => {
            let _ = c;
            o.extends_id.map(|id| id.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    })
}

/// Builds one `Entry` per object: its row, or a poisoned entry (key only) with the reason.
pub(super) fn entries(
    c: &Catalog,
    t: &TableDef,
    wanted: impl Fn(&Object) -> Option<Vec<(u32, Value)>>,
    build: impl Fn(&Object, &mut Row) -> Result<(), String>,
) -> Vec<Entry> {
    let mut out = Vec::new();
    let blank = Blank::new(t);
    for o in c.objects {
        let Some(key) = wanted(o) else { continue };
        let mut r = blank.row();
        for (id, v) in &key {
            r[pos(t, *id)] = v.clone();
        }
        match build(o, &mut r) {
            Ok(()) => out.push(Entry::ok(r)),
            Err(why) => {
                let mut k = blank.row();
                for (id, v) in key {
                    k[pos(t, id)] = v;
                }
                out.push(Entry::poisoned(k, why));
            }
        }
    }
    out
}

/// A platform object (a system table): it has a package id, shared by all of them, and no runtime package id.
fn is_platform(o: &Object) -> bool {
    o.id.is_some_and(|id| id >= 2_000_000_000)
}

/// The package id AllObj reports for `o`: the owning app's, or the platform's own for a platform object.
fn package_of(c: &Catalog, o: &Object) -> [u8; 16] {
    if is_platform(o) {
        ids::package_id([0x70; 16])
    } else {
        ids::package_id(c.owner(o))
    }
}

/// "App Runtime Package ID": the package id for an object of the app under test, a different one for an object of a
/// dependency app (Base Application), and none for a platform object (metadata/W6AllObj.al:
/// AllObj_PackageIds_ThisApp_BaseApp_Platform, BC 27.5 and 29.0).
fn runtime_package_id(c: &Catalog, o: &Object) -> [u8; 16] {
    if is_platform(o) {
        return [0; 16];
    }
    let dependency = c.apps.get(o.app as usize).is_some_and(|a| a.dependency);
    ids::runtime_package_id(c.owner(o), dependency)
}

pub(super) fn all_obj_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| {
            let (Some(id), Some(ty)) = (o.id, object_type(o.kind)) else {
                return None;
            };
            Some(vec![(1, option(t, 1, ty)), (3, Value::Integer(id as i32))])
        },
        |o, r| {
            let pkg = package_of(&c, o);
            r[pos(t, 4)] = Value::Text(o.name.clone().into());
            r[pos(t, 60)] = Value::Guid(pkg);
            r[pos(t, 61)] = Value::Guid(runtime_package_id(&c, o));
            r[pos(t, 62)] = Value::Text(namespace(o).into());
            Ok(())
        },
    )
}

pub(super) fn all_obj_with_caption_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| {
            let (Some(id), Some(ty)) = (o.id, object_type(o.kind)) else {
                return None;
            };
            Some(vec![(1, option(t, 1, ty)), (3, Value::Integer(id as i32))])
        },
        |o, r| {
            let owner = c.owner(o);
            let pkg = package_of(&c, o);
            r[pos(t, 4)] = Value::Text(o.name.clone().into());
            r[pos(t, 20)] = Value::Text(truncate(&caption(o), 249).into());
            r[pos(t, 30)] = Value::Text(subtype_text(&c, o)?.into());
            r[pos(t, 60)] = Value::Guid(pkg);
            r[pos(t, 61)] = Value::Guid(runtime_package_id(&c, o));
            r[pos(t, 62)] = Value::Guid(if carries_app_id(o.kind) { owner } else { [0; 16] });
            r[pos(t, 63)] = Value::Text(namespace(o).into());
            Ok(())
        },
    )
}

// ---- CodeUnit Metadata ----------------------------------------------------------------------------

pub(super) fn codeunit_metadata_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| match (o.id, o.kind) {
            (Some(id), al_ir::ObjKind::Codeunit) => Some(vec![(1, Value::Integer(id as i32))]),
            _ => None,
        },
        |o, r| {
            let subtype = match prop(o, "Subtype") {
                None => "Normal",
                Some(v) => member(CODEUNIT_SUBTYPES, v)
                    .ok_or_else(|| format!("codeunit '{}': Subtype `{v}` is not understood", o.name))?,
            };
            // Install does not reach the metadata: the compiler writes Normal (TestCodeunitMetadataVirtualTable.al,
            // `InstallCodeunit_ReportsSubtypeNormal`, `QuotedSubtypeInstall_ReportsSubtypeNormal`).
            let ordinal = match subtype {
                "Test" => 1,
                "TestRunner" => 2,
                "Upgrade" => 3,
                _ => 0,
            };
            r[pos(t, 2)] = Value::Text(o.name.clone().into());
            r[pos(t, 3)] = Value::Integer(table_no(&c, o)? as i32);
            r[pos(t, 4)] = Value::Boolean(prop(o, "SingleInstance").is_some_and(truthy));
            r[pos(t, 5)] = option(t, 5, ordinal);
            r[pos(t, 6)] = Value::Guid(c.owner(o));
            r[pos(t, 7)] = Value::Text(truncate(&ident(prop(o, "InherentPermissions").unwrap_or("")), 5).into());
            r[pos(t, 8)] = Value::Text(truncate(&ident(prop(o, "InherentEntitlements").unwrap_or("")), 5).into());
            // A test codeunit is a unit test; nothing else the corpus pins is anything but None
            // (TestCodeunitMetadataVirtualTable.al, `TestCodeunit_ReportsTestTypeUnitTest`).
            r[pos(t, 9)] = option(t, 9, i32::from(subtype == "Test"));
            // RequiredTestIsolation reads None for every codeunit (same file,
            // `RequiredTestIsolation_ReportsNoneForEveryCodeunit`).
            r[pos(t, 10)] = option(t, 10, 0);
            r[pos(t, 11)] = Value::Text(namespace(o).into());
            Ok(())
        },
    )
}

/// The id of the table a codeunit's `TableNo` names; 0 for none.
fn table_no(c: &Catalog, o: &Object) -> Result<u32, String> {
    // A compiled codeunit has `Rec`: a global of the table's record type.
    if let Some(slot) = o.rec_slot {
        if let Some(RtType::Record(t, _)) = o.globals.get(slot).map(|g| &g.ty) {
            return Ok(c.objects[*t as usize].id.unwrap_or(0));
        }
    }
    match prop(o, "TableNo") {
        None => Ok(0),
        Some(v) => c
            .id_of(al_ir::ObjKind::Table, v)
            .map_err(|why| format!("codeunit '{}': TableNo {why}", o.name)),
    }
}

// ---- Table Metadata -------------------------------------------------------------------------------

const DATA_CLASSIFICATIONS: &[&str] = &[
    "CustomerContent",
    "ToBeClassified",
    "EndUserIdentifiableInformation",
    "AccountData",
    "EndUserPseudonymousIdentifiers",
    "OrganizationIdentifiableInformation",
    "SystemMetadata",
];
const OBSOLETE_STATES: &[&str] = &["No", "Pending", "Removed"];
const COMPRESSION_TYPES: &[&str] = &["Unspecified", "None", "Row", "Page"];
const SCOPES: &[&str] = &["Cloud", "OnPrem"];
const ACCESS: &[&str] = &["Public", "Internal"];

fn ordinal_of(members: &[&str], text: &str) -> Option<i32> {
    members
        .iter()
        .position(|m| m.eq_ignore_ascii_case(&ident(text)))
        .map(|i| i as i32)
}

pub(super) fn table_metadata_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| match (o.id, o.kind) {
            (Some(id), al_ir::ObjKind::Table) => Some(vec![(1, Value::Integer(id as i32))]),
            _ => None,
        },
        |o, r| {
            let bad = |name: &str, v: &str| format!("table '{}': {name} `{v}` is not understood", o.name);
            let opt = |name: &str, members: &[&str], default: i32| -> Result<i32, String> {
                match prop(o, name) {
                    None => Ok(default),
                    Some(v) => ordinal_of(members, v).ok_or_else(|| bad(name, v)),
                }
            };
            let page = |name: &str| -> Result<i32, String> {
                match prop(o, name) {
                    None => Ok(0),
                    Some(v) => c
                        .id_of(al_ir::ObjKind::Page, v)
                        .map(|i| i as i32)
                        .map_err(|why| format!("table '{}': {name} {why}", o.name)),
                }
            };
            r[pos(t, 2)] = Value::Text(o.name.clone().into());
            r[pos(t, 3)] = Value::Text(truncate(&caption(o), 80).into());
            // DataPerCompany defaults to true (TestTableMetadataVirtualTable.al, `DeclaredTable_ReturnsMatchingRow`).
            r[pos(t, 4)] = Value::Boolean(prop(o, "DataPerCompany").is_none_or(truthy));
            r[pos(t, 5)] = Value::Integer(page("LookupPageId")?);
            r[pos(t, 6)] = Value::Integer(page("DrillDownPageId")?);
            // PasteIsValid defaults to true in AL; no corpus test reads it.
            r[pos(t, 8)] = Value::Boolean(prop(o, "PasteIsValid").is_none_or(truthy));
            r[pos(t, 11)] = option(t, 11, opt("TableType", TABLE_TYPES, 0)?);
            r[pos(t, 12)] =
                Value::Text(truncate(&prop(o, "ExternalName").and_then(string_lit).unwrap_or_default(), 248).into());
            r[pos(t, 13)] = option(t, 13, opt("ObsoleteState", OBSOLETE_STATES, 0)?);
            r[pos(t, 14)] =
                Value::Text(truncate(&prop(o, "ObsoleteReason").and_then(string_lit).unwrap_or_default(), 248).into());
            // No DataClassification reads CustomerContent (same file, `TableDeclaringNoDataClassification_ReportsTheDefault`).
            r[pos(t, 15)] = option(t, 15, opt("DataClassification", DATA_CLASSIFICATIONS, 0)?);
            r[pos(t, 16)] = Value::Boolean(prop(o, "ReplicateData").is_none_or(truthy));
            r[pos(t, 17)] = option(t, 17, opt("CompressionType", COMPRESSION_TYPES, 0)?);
            r[pos(t, 18)] = Value::Guid(c.owner(o));
            r[pos(t, 19)] = Value::Text(truncate(&ident(prop(o, "InherentPermissions").unwrap_or("")), 5).into());
            r[pos(t, 20)] = Value::Text(truncate(&ident(prop(o, "InherentEntitlements").unwrap_or("")), 5).into());
            r[pos(t, 21)] = option(t, 21, opt("Scope", SCOPES, 0)?);
            r[pos(t, 22)] = option(t, 22, opt("Access", ACCESS, 0)?);
            r[pos(t, 23)] = Value::Text(namespace(o).into());
            Ok(())
        },
    )
}
