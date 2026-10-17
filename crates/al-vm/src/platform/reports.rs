//! The report tables: Report Layout List, Report Metadata and Report Data Items.
//!
//! Pinned by corpus handlers/TestSourceReportLayoutFlags.al and TestPrecompiledReportLayouts.al (the
//! layouts of a report, from its `rendering` section: their format, `ObsoleteState = Pending` as
//! IsObsolete, `ExcelLayoutMultipleDataSheets` as Default / Single / Multiple) and
//! handlers/TestReportMetadataColumns.al (`ProcessingOnly`, `UseRequestPage`, `WordMergeDataItem`, the
//! table of the first data item; and per data item: name, related table, indentation, the field numbers
//! of `RequestFilterFields` and of the sorting key, and a view that ends the sort direction with
//! `ORDER(1)` when it is descending).
//!
//! The layouts of any report come from the program, shells included (the declaration scan reads them).
//! The data items exist only for a report that compiled; the rows of another are poisoned.

use super::mat::Entry;
use super::objects::{Catalog, ident, prop, string_lit, truthy};
use super::{Providers, blank, ids, option, pos};
use al_builtins::describe::Describe;
use al_builtins::value::{RtType, Value};
use al_data::TableDef;
use al_ir::{Object, Table};

fn lprop<'a>(l: &'a al_ir::LayoutDecl, name: &str) -> Option<&'a str> {
    l.properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

const FORMATS: &[&str] = &["RDLC", "Word", "Excel", "Custom"];

/// Report Layout List: "Report ID" (1), Name (2), "Layout Format" (7), IsObsolete (16),
/// ExcelLayoutMultipleDataSheets (17); the key also holds "Runtime Package ID" (5).
pub(super) fn layout_list_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let mut out = Vec::new();
    // A reportextension's layouts are rows of the report it extends, owned by the extension's app
    // (record/TestEventSubscriptionColumnsReportLayoutListContracts.al:131-165).
    for o in c
        .objects
        .iter()
        .filter(|o| matches!(o.kind, al_ir::ObjKind::Report | al_ir::ObjKind::ReportExtension))
    {
        let id = if o.kind == al_ir::ObjKind::Report {
            o.id
        } else {
            o.extends_id
        };
        let Some(id) = id else { continue };
        let pkg = ids::package_id(c.owner(o));
        for l in &o.layouts {
            let mut r = blank(t);
            r[pos(t, 1)] = Value::Integer(id as i32);
            r[pos(t, 2)] = Value::Text(l.name.chars().take(250).collect());
            r[pos(t, 5)] = Value::Guid(pkg);
            // The app that owns the report (the layout is published with it; probe batch 5).
            r[pos(t, 11)] = Value::Guid(c.owner(o));
            match layout_row(o, l, t, &mut r) {
                Ok(()) => out.push(Entry::ok(r)),
                Err(why) => {
                    let mut k = blank(t);
                    k[pos(t, 1)] = Value::Integer(id as i32);
                    k[pos(t, 2)] = Value::Text(l.name.chars().take(250).collect());
                    k[pos(t, 5)] = Value::Guid(pkg);
                    out.push(Entry::poisoned(k, why));
                }
            }
        }
    }
    out
}

fn layout_row(o: &Object, l: &al_ir::LayoutDecl, t: &TableDef, r: &mut al_data::Row) -> Result<(), String> {
    let what = || format!("layout '{}' of report '{}'", l.name, o.name);
    let format = lprop(l, "Type").ok_or_else(|| format!("{}: no Type", what()))?;
    let format = FORMATS
        .iter()
        .position(|f| f.eq_ignore_ascii_case(&ident(format)))
        .ok_or_else(|| format!("{}: Type `{format}` is not pinned", what()))?;
    r[pos(t, 7)] = option(t, 7, format as i32);
    // Caption is the Caption literal, else the layout's name; Description the Summary literal, else the layout's name.
    let literal = |name: &str| -> Result<Option<String>, String> {
        lprop(l, name)
            .map(|v| string_lit(v).ok_or_else(|| format!("{}: {name} `{v}` is not a literal", what())))
            .transpose()
    };
    // A layout without a Caption reads its name (session/PWPlatformTables.al:126-139).
    r[pos(t, 3)] = Value::Text(
        literal("Caption")?
            .unwrap_or_else(|| l.name.clone())
            .chars()
            .take(250)
            .collect(),
    );
    r[pos(t, 10)] = Value::Text(
        literal("Summary")?
            .unwrap_or_else(|| l.name.clone())
            .chars()
            .take(250)
            .collect(),
    );
    // Pending is obsolete (TestPrecompiledReportLayouts.al:63-72); Removed is not pinned.
    r[pos(t, 16)] = Value::Boolean(match lprop(l, "ObsoleteState").map(ident) {
        None => false,
        Some(s) if s.eq_ignore_ascii_case("No") => false,
        Some(s) if s.eq_ignore_ascii_case("Pending") => true,
        Some(s) => return Err(format!("{}: ObsoleteState `{s}` is not pinned", what())),
    });
    // The enum "Excel Sheet Configuration": Default, Single Data sheet, Multiple data sheets.
    let sheets = match lprop(l, "ExcelLayoutMultipleDataSheets") {
        None => 0,
        Some(v) if truthy(v) => 2,
        Some(v) if v.eq_ignore_ascii_case("false") => 1,
        Some(v) => return Err(format!("{}: ExcelLayoutMultipleDataSheets `{v}`", what())),
    };
    r[pos(t, 17)] = match &t.fields[pos(t, 17)].ty {
        RtType::Enum(e) => Value::Enum(*e, sheets),
        other => return Err(format!("field 17 is a {}, not an enum", other.describe())),
    };
    Ok(())
}

/// Report Metadata: ID (1), UseRequestPage (4), ProcessingOnly (9), WordMergeDataItem (17),
/// FirstDataItemTableID (18). Needs the data items, so a report nothing compiled is poisoned.
pub(super) fn report_metadata_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let mut out = Vec::new();
    for o in c.objects.iter().filter(|o| o.kind == al_ir::ObjKind::Report) {
        let Some(id) = o.id else { continue };
        let mut key = blank(t);
        key[pos(t, 1)] = Value::Integer(id as i32);
        let Some(rep) = &o.report else {
            out.push(Entry::poisoned(
                key,
                format!("report '{}' was not compiled, so its data items are not known", o.name),
            ));
            continue;
        };
        let mut r = key.clone();
        r[pos(t, 2)] = Value::Text(o.name.clone().into());
        // UseRequestPage defaults to true, ProcessingOnly to false (TestReportMetadataColumns.al:30-60).
        r[pos(t, 4)] = Value::Boolean(prop(o, "UseRequestPage").is_none_or(truthy));
        r[pos(t, 9)] = Value::Boolean(prop(o, "ProcessingOnly").is_some_and(truthy));
        r[pos(t, 17)] = Value::Text(prop(o, "WordMergeDataItem").map(ident).unwrap_or_default().into());
        // DefaultLayoutName: the layout `DefaultRenderingLayout` names, else the file the Word, RDLC or Excel layout property gives, as
        // written (metadata/lt-platform-metadata-columns/PlatformMetadataColumnValues.al:ReportMetadata_DefaultLayoutName_OwnAndBase:
        // `RdlcFixture`, `StandardSalesInvoice.docx`, `./System/Notifications/NotificationEmail.docx`, blank for a report with neither).
        // DefaultLayout: the kind the property names (`Word` for report 1306), RDLC when it names none.
        let rendering_kind = rep
            .default_rendering_layout
            .as_ref()
            .and_then(|n| rep.rendering_layouts.iter().find(|l| l.name.eq_ignore_ascii_case(n)))
            .map(|l| l.kind.clone());
        let kind_text = prop(o, "DefaultLayout").map(ident).or(rendering_kind);
        if let (Some(col), Some(kind)) = (t.fields.iter().position(|x| x.name == "DefaultLayout"), kind_text) {
            if let Value::Option(_, members) = &r[col] {
                if let Some(n) = members.iter().position(|m| m.eq_ignore_ascii_case(&kind)) {
                    r[col] = Value::Option(n as i32, members.clone());
                }
            }
        }
        if let Some(col) = t.fields.iter().position(|x| x.name == "DefaultLayoutName") {
            let name = prop(o, "DefaultRenderingLayout")
                .map(ident)
                .or_else(|| {
                    ["WordLayout", "RDLCLayout", "ExcelLayout"]
                        .iter()
                        .find_map(|n| prop(o, n))
                        .and_then(string_lit)
                })
                .unwrap_or_default();
            r[col] = Value::Text(name.into());
        }
        let first = rep
            .items
            .iter()
            .find(|i| i.parent.is_none())
            .and_then(|i| i.table)
            .and_then(|t| c.objects[t].id);
        match first {
            Some(tid) => r[pos(t, 18)] = Value::Integer(tid as i32),
            None if rep.items.is_empty() => {}
            None => {
                out.push(Entry::poisoned(
                    key,
                    format!("report '{}': the table of its first data item is not compiled", o.name),
                ));
                continue;
            }
        }
        out.push(Entry::ok(r));
    }
    out
}

/// `sorting(a, b)`, `order(...)` and `where(...)` of a view, each with the text between its brackets.
fn view_segments(view: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let b: Vec<char> = view.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_alphanumeric() {
            i += 1;
        }
        let word: String = b[start..i].iter().collect::<String>().to_ascii_lowercase();
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        if word.is_empty() || b.get(i) != Some(&'(') {
            return Err(format!("view `{view}`"));
        }
        let (mut depth, mut quoted) = (0, false);
        let open = i;
        while i < b.len() {
            match b[i] {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if i >= b.len() {
            return Err(format!("view `{view}`"));
        }
        out.push((word, b[open + 1..i].iter().collect::<String>()));
        i += 1;
    }
    Ok(out)
}

/// The field numbers a comma separated list of field names names, in order.
fn field_numbers(table: &Table, list: &str) -> Result<Vec<u32>, String> {
    list.split(',')
        .map(|n| {
            let n = ident(n);
            table
                .fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&n))
                .map(|f| f.id)
                .ok_or_else(|| format!("`{n}` is not a field of the table"))
        })
        .collect()
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

/// Report Data Items: "Report ID" (1), "Data Item ID" (2), Name (3), "Request Filter Fields" (4),
/// "Related Table ID" (5), "Indentation Level" (6), "Sorting Fields" (7), "Data Item Table View" (8).
/// The id is not a declaration ordinal (TestReportMetadataColumns.al:200-232, which only needs
/// the ids of two items to differ and not to be 1 and 2): it is the position times ten.
pub(super) fn report_data_item_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let mut out = Vec::new();
    for o in c.objects.iter().filter(|o| o.kind == al_ir::ObjKind::Report) {
        let Some(id) = o.id else { continue };
        let Some(rep) = &o.report else {
            let mut k = blank(t);
            k[pos(t, 1)] = Value::Integer(id as i32);
            out.push(Entry::poisoned_prefix(
                k,
                1,
                format!("report '{}' was not compiled, so its data items are not known", o.name),
            ));
            continue;
        };
        for (n, item) in rep.items.iter().enumerate() {
            let item_id = (n as i32 + 1) * 10;
            let mut key = blank(t);
            key[pos(t, 1)] = Value::Integer(id as i32);
            key[pos(t, 2)] = Value::Integer(item_id);
            let mut r = key.clone();
            match data_item_row(&c, o, rep, item, t, &mut r) {
                Ok(()) => out.push(Entry::ok(r)),
                Err(why) => out.push(Entry::poisoned(key, why)),
            }
        }
    }
    out
}

fn data_item_row(
    c: &Catalog,
    o: &Object,
    rep: &al_ir::Report,
    item: &al_ir::ReportItem,
    t: &TableDef,
    r: &mut al_data::Row,
) -> Result<(), String> {
    let what = || format!("data item '{}' of report '{}'", item.name, o.name);
    let table_obj = item
        .table
        .map(|i| &c.objects[i])
        .ok_or_else(|| format!("{}: its table is not compiled", what()))?;
    let table = table_obj
        .table
        .as_ref()
        .ok_or_else(|| format!("{}: its table was not compiled", what()))?;
    r[pos(t, 3)] = Value::Text(item.name.clone().into());
    r[pos(t, 5)] = Value::Integer(table_obj.id.unwrap_or(0) as i32);
    let mut depth = 0;
    let mut at = item.parent;
    while let Some(parent) = at {
        depth += 1;
        at = rep.items[parent].parent;
    }
    r[pos(t, 6)] = Value::Integer(depth);
    // The field numbers of RequestFilterFields in declaration order ('2,5').
    let filter_fields = match &item.request_filter_fields {
        None => String::new(),
        Some(list) => join(&field_numbers(table, list).map_err(|e| format!("{}: RequestFilterFields {e}", what()))?),
    };
    r[pos(t, 4)] = Value::Text(filter_fields.into());
    // The sorting key's field numbers in the key's own order ('5,2'), and the view BC formats from the
    // view: SORTING(FieldN,..) then ORDER(1) when descending, then WHERE for a single `field = const(v)`.
    let mut sorting = String::new();
    let mut view = Vec::new();
    if let Some(text) = &item.view {
        let mut where_text = None;
        let mut descending = false;
        for (word, inner) in view_segments(text).map_err(|e| format!("{}: {e}", what()))? {
            match word.as_str() {
                "sorting" => {
                    let ids = field_numbers(table, &inner).map_err(|e| format!("{}: sorting {e}", what()))?;
                    sorting = join(&ids);
                    view.push(format!(
                        "SORTING({})",
                        ids.iter().map(|i| format!("Field{i}")).collect::<Vec<_>>().join(",")
                    ));
                }
                "order" if inner.trim().eq_ignore_ascii_case("descending") => descending = true,
                "order" => return Err(format!("{}: order({}) is not pinned", what(), inner.trim())),
                "where" => where_text = Some(inner),
                other => return Err(format!("{}: `{other}` in the view is not pinned", what())),
            }
        }
        if descending {
            view.push("ORDER(1)".into());
        }
        if let Some(w) = where_text {
            view.push(where_const(table, c.objects, &w).map_err(|e| format!("{}: where({w}): {e}", what()))?);
        }
    }
    r[pos(t, 7)] = Value::Text(sorting.into());
    r[pos(t, 8)] = Value::Text(view.join(" ").into());
    Ok(())
}

/// `WHERE(Field6=CONST(2))` for the single `field = const(value)` condition; others are not pinned.
fn where_const(table: &Table, objects: &[Object], inner: &str) -> Result<String, String> {
    let (field, rest) = inner.split_once('=').ok_or("not a field = const(value)")?;
    let rest = rest.trim();
    let lower = rest.to_ascii_lowercase();
    let value = lower
        .strip_prefix("const(")
        .and_then(|_| rest[6..].strip_suffix(')'))
        .ok_or("only const(value) is pinned")?
        .trim();
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err("the value is not pinned".into());
    }
    let name = ident(field);
    let f = table
        .fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(&name))
        .ok_or("no such field")?;
    let shown = match &f.ty {
        RtType::Option(m) => m
            .iter()
            .position(|x| x.eq_ignore_ascii_case(value))
            .ok_or("not an option member")?
            .to_string(),
        RtType::Enum(e) => objects[*e as usize]
            .enum_values
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(value))
            .map(|(v, _)| v.to_string())
            .ok_or("not an enum member")?,
        RtType::Integer | RtType::BigInteger if value.parse::<i64>().is_ok() => value.to_string(),
        _ => return Err("only numbers and option members are pinned".into()),
    };
    Ok(format!("WHERE(Field{}=CONST({shown}))", f.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_splits_into_its_words_and_bracketed_texts() {
        let v = view_segments("sorting(\"No.\", Name) ORDER(descending) where(Kind = const(B), \"A (b)\" = const(1))")
            .unwrap();
        assert_eq!(
            v,
            vec![
                ("sorting".to_string(), "\"No.\", Name".to_string()),
                ("order".to_string(), "descending".to_string()),
                ("where".to_string(), "Kind = const(B), \"A (b)\" = const(1)".to_string()),
            ]
        );
        assert!(view_segments("sorting(a").is_err());
        assert!(view_segments("nonsense").is_err());
    }
}
