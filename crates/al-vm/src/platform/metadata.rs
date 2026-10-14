//! XmlPort Metadata and Query Metadata: one row per xmlport or query object, from the object's own
//! declaration (shells included), keyed by object id.
//!
//! Pinned by xmlport/TestXmlPortMetadataVirtualTable.al and query/TestQueryMetadataVirtualTable.al.
//! A property the object does not declare reads its documented default; those defaults are not
//! pinned by the corpus and are recorded in docs/probes-needed.md.

use super::mat::Entry;
use super::objects::{Catalog, base_kind, caption, entries, ident, member, prop, string_lit, truncate, truthy};
use super::{Providers, option, pos};
use al_builtins::value::Value;
use al_data::TableDef;
use al_ir::Object;

/// An option property as its ordinal: undeclared reads `default`, an unknown member is refused.
fn ordinal(o: &Object, name: &str, members: &[&str], default: i32) -> Result<i32, String> {
    match prop(o, name) {
        None => Ok(default),
        Some(v) => member(members, v)
            .and_then(|m| members.iter().position(|x| x == &m))
            .map(|i| i as i32)
            .ok_or_else(|| format!("{} '{}': {name} `{v}` is not understood", o.kind, o.name)),
    }
}

fn text(o: &Object, name: &str, max: usize) -> String {
    prop(o, name).and_then(string_lit).map(|s| truncate(&s, max)).unwrap_or_default()
}

const TRANSACTION_TYPES: &[&str] = &["UpdateNoLocks", "Update", "Snapshot", "Browse", "Report"];
const DIRECTIONS: &[&str] = &["Import", "Export", "Both"];
const TEXT_ENCODINGS: &[&str] = &["MSDos", "UTF8", "UTF16", "Windows"];
const FORMATS: &[&str] = &["Xml", "VariableText", "FixedText"];

/// XmlPort Metadata: ID (1), Name (2), Caption (3), UseRequestPage (4), TransactionType (5),
/// Direction (6), TextEncoding (8), Format (9), "App ID" (10).
pub(super) fn xmlport_metadata_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| match (o.id, base_kind(o)) {
            (Some(id), "xmlport") => Some(vec![(1, Value::Integer(id as i32))]),
            _ => None,
        },
        |o, r| {
            r[pos(t, 2)] = Value::Text(truncate(&o.name, 30));
            r[pos(t, 3)] = Value::Text(truncate(&caption(o), 80));
            r[pos(t, 4)] = Value::Boolean(prop(o, "UseRequestPage").is_none_or(truthy));
            r[pos(t, 5)] = option(t, 5, ordinal(o, "TransactionType", TRANSACTION_TYPES, 0)?);
            r[pos(t, 6)] = option(t, 6, ordinal(o, "Direction", DIRECTIONS, 2)?);
            r[pos(t, 8)] = option(t, 8, ordinal(o, "TextEncoding", TEXT_ENCODINGS, 1)?);
            r[pos(t, 9)] = option(t, 9, ordinal(o, "Format", FORMATS, 0)?);
            r[pos(t, 10)] = Value::Guid(c.owner(o));
            Ok(())
        },
    )
}

/// Query Metadata: ID (1), Name (2), Caption (3), APIPublisher (4), APIGroup (5), APIVersion (6),
/// EntitySetName (7), EntityName (8), "App ID" (10). A query that is not an API query leaves the
/// API columns blank (TestQueryMetadataVirtualTable.al:107-115). An API query that declares no
/// version is not pinned and is refused.
pub(super) fn query_metadata_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    entries(
        &c,
        t,
        |o| match (o.id, base_kind(o)) {
            (Some(id), "query") => Some(vec![(1, Value::Integer(id as i32))]),
            _ => None,
        },
        |o, r| {
            let api = prop(o, "QueryType").is_some_and(|v| ident(v).eq_ignore_ascii_case("API"));
            if api && prop(o, "APIVersion").is_none() {
                return Err(format!(
                    "query '{}': an API query that declares no APIVersion is not pinned",
                    o.name
                ));
            }
            r[pos(t, 2)] = Value::Text(truncate(&o.name, 30));
            r[pos(t, 3)] = Value::Text(truncate(&caption(o), 80));
            r[pos(t, 4)] = Value::Text(text(o, "APIPublisher", 40));
            r[pos(t, 5)] = Value::Text(text(o, "APIGroup", 40));
            r[pos(t, 6)] = Value::Text(text(o, "APIVersion", 250));
            r[pos(t, 7)] = Value::Text(text(o, "EntitySetName", 250));
            r[pos(t, 8)] = Value::Text(text(o, "EntityName", 250));
            r[pos(t, 10)] = Value::Guid(c.owner(o));
            Ok(())
        },
    )
}
