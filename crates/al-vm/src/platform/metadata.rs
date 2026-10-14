//! XmlPort Metadata and Query Metadata: one row per xmlport or query object, from the object's own
//! declaration (shells included), keyed by object id.
//!
//! Pinned by xmlport/TestXmlPortMetadataVirtualTable.al and query/TestQueryMetadataVirtualTable.al.
//! A property the object does not declare reads its documented default; those defaults are not
//! pinned by the corpus and are recorded in docs/probes-needed.md.

use super::mat::Entry;
use super::objects::{Catalog, base_kind, caption, entries, ident, member, prop, string_lit, truncate, truthy};
use super::{Providers, blank, option, pos};
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

// ---- Event Subscription -----------------------------------------------------------------------------

/// Event Subscription: "Subscriber Codeunit ID" (1), "Subscriber Function" (2), "Event Type" (3),
/// "Publisher Object Type" (4), "Publisher Object ID" (5), "Published Function" (6), Active (7).
/// One row per `[EventSubscriber]` routine of a compiled codeunit (record/TestEventSubscriptionVirtualTable.al).
///
/// A codeunit of a dependency that was not compiled may declare subscribers the program does not
/// hold, so each is one poisoned row with an empty function: a read that its key filters exclude is
/// answered, any other read is refused.
pub(super) fn event_subscription_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let mut out = Vec::new();
    for o in c.objects {
        let Some(id) = o.id else { continue };
        if o.kind == "uncompiled codeunit" {
            let mut k = blank(t);
            k[pos(t, 1)] = Value::Integer(id as i32);
            out.push(Entry::poisoned(
                k,
                format!(
                    "codeunit '{}' was not compiled, so the event subscribers it declares are not known",
                    o.name
                ),
            ));
            continue;
        }
        if o.kind != "codeunit" {
            continue;
        }
        for r in &o.routines {
            let Some(a) = r.attributes.iter().find(|a| a.name.eq_ignore_ascii_case("EventSubscriber")) else {
                continue;
            };
            let mut key = blank(t);
            key[pos(t, 1)] = Value::Integer(id as i32);
            key[pos(t, 2)] = Value::Text(truncate(&r.name, 250));
            match subscription_row(&c, o, &r.name, a, t, key.clone()) {
                Ok(row) => out.push(Entry::ok(row)),
                Err(why) => out.push(Entry::poisoned(key, why)),
            }
        }
    }
    out
}

fn subscription_row(
    c: &Catalog,
    o: &Object,
    routine: &str,
    a: &al_ir::Attribute,
    t: &TableDef,
    mut r: al_data::Row,
) -> Result<al_data::Row, String> {
    let what = || format!("subscriber '{}' of codeunit '{}'", routine, o.name);
    if o.properties.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("EventSubscriberInstance") && v.trim().eq_ignore_ascii_case("Manual")
    }) {
        return Err(format!("{}: a manual instance is not pinned (Active)", what()));
    }
    let args = crate::events::split_args(&a.text);
    if args.len() < 3 {
        return Err(format!("{}: the attribute has fewer than three arguments", what()));
    }
    let kind = crate::events::unquote(args[0].rsplit("::").next().unwrap_or("")).to_ascii_lowercase();
    let Some(object_type) = super::objects::object_type(&kind) else {
        return Err(format!("{}: publisher kind `{kind}` is not listed", what()));
    };
    let (target, _) = crate::events::publisher_name(args[1].rsplit("::").next().unwrap_or(""));
    let publisher_id = c.id_of(&kind, &target).map_err(|e| format!("{}: publisher {e}", what()))?;
    let event = crate::events::unquote(&args[2]);
    // The publisher's own routine says which kind of event it is; a table's database triggers are
    // the platform's (event names `On{Before,After}{Insert,Modify,Delete,Rename,Validate}Event`).
    let publisher = c
        .objects
        .iter()
        .find(|x| x.id == Some(publisher_id) && base_kind(x) == kind);
    let declared = publisher.and_then(|p| {
        p.routines.iter().find_map(|pr| {
            if !pr.name.eq_ignore_ascii_case(&event) {
                return None;
            }
            pr.attributes.iter().find_map(|pa| match pa.name.to_ascii_lowercase().as_str() {
                "businessevent" => Some((pr.name.clone(), 1)),
                "integrationevent" => Some((pr.name.clone(), 2)),
                "internalevent" => Some((pr.name.clone(), 3)),
                _ => None,
            })
        })
    });
    let (published, event_type) = match declared {
        Some(d) => d,
        None if kind == "table" && is_trigger_event(&event) => (event.clone(), 0),
        None => return Err(format!("{}: the kind of event `{event}` is not known", what())),
    };
    r[pos(t, 3)] = option(t, 3, event_type);
    r[pos(t, 4)] = option(t, 4, object_type);
    r[pos(t, 5)] = Value::Integer(publisher_id as i32);
    r[pos(t, 6)] = Value::Text(truncate(&published, 250));
    // A statically bound subscriber that resolved is active (TestEventSubscriptionVirtualTable.al:`_Row_DescribesTheSubscription`).
    r[pos(t, 7)] = Value::Boolean(true);
    Ok(r)
}

fn is_trigger_event(event: &str) -> bool {
    let e = event.to_ascii_lowercase();
    ["onbefore", "onafter"].iter().any(|p| {
        e.strip_prefix(p).is_some_and(|rest| {
            ["insert", "modify", "delete", "rename", "validate"]
                .iter()
                .any(|k| rest.strip_prefix(k) == Some("event"))
        })
    })
}
