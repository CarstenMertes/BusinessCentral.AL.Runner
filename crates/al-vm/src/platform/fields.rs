//! The Field and Key tables: one row per field and per key of every compiled table.
//!
//! Pinned by corpus record/TestFieldDataClassificationVirtualTable.al,
//! TestFieldDataClassificationInheritance.al, TestFieldEnumTypeVirtualTable.al,
//! TestFieldObsoleteStateVirtualTable.al and TestKeyVirtualTable.al. A table nothing compiled has
//! no fields or keys in the program, so its rows are not known: they are poisoned, and a read that
//! might touch them is refused (`mat.rs`).

use super::mat::Entry;
use super::objects::{Catalog, base_kind, ident, prop, string_lit, truthy};
use super::{Blank, Providers, blank, pos};
use al_builtins::value::{RtType, Value};
use al_data::TableDef;
use al_ir::{Field, Object};

const OBSOLETE_STATES: &[&str] = &["No", "Pending", "Removed"];
const DATA_CLASSIFICATIONS: &[&str] = &[
    "CustomerContent",
    "ToBeClassified",
    "EndUserIdentifiableInformation",
    "AccountData",
    "EndUserPseudonymousIdentifiers",
    "OrganizationIdentifiableInformation",
    "SystemMetadata",
];

fn ordinal_of(members: &[&str], text: &str) -> Option<i32> {
    members
        .iter()
        .position(|m| m.eq_ignore_ascii_case(&ident(text)))
        .map(|i| i as i32)
}

/// The Field "Type" member (ordinal of `TableFilter,RecordID,OemText,Date,Time,DateFormula,Decimal,
/// Media,MediaSet,Text,Code,Binary,BLOB,Boolean,Integer,OemCode,Option,BigInteger,Duration,GUID,
/// DateTime`) for a field type. An enum field is an option (TestFieldEnumTypeVirtualTable.al:56).
fn type_ordinal(ty: &RtType) -> Option<i32> {
    Some(match ty {
        RtType::Library(n) => match n.to_ascii_lowercase().as_str() {
            "recordid" => 1,
            "media" => 7,
            "mediaset" => 8,
            "blob" => 12,
            "tablefilter" => 0,
            _ => return None,
        },
        RtType::Date => 3,
        RtType::Time => 4,
        RtType::DateFormula => 5,
        RtType::Decimal => 6,
        RtType::Text(_) => 9,
        RtType::Code(_) => 10,
        RtType::Boolean => 13,
        RtType::Integer => 14,
        RtType::Option(_) | RtType::Enum(_) => 16,
        RtType::BigInteger => 17,
        RtType::Duration => 18,
        RtType::Guid => 19,
        RtType::DateTime => 20,
        _ => return None,
    })
}

/// A table of the program that nothing compiled: it has a name and an id and no fields.
fn shell_entries(o: &Object, t: &TableDef, key_of: &dyn Fn(u32) -> Vec<(u32, Value)>) -> Entry {
    let mut k = blank(t);
    for (id, v) in key_of(o.id.unwrap_or(0)) {
        k[pos(t, id)] = v;
    }
    Entry::poisoned(
        k,
        format!(
            "table '{}' was not compiled, so its fields and keys are not known",
            o.name
        ),
    )
}

fn table_of<'a>(c: &'a Catalog, i: usize) -> Option<(&'a Object, &'a al_ir::Table)> {
    let o = &c.objects[i];
    o.table.as_ref().map(|t| (o, t))
}

pub(super) fn field_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let blank = Blank::new(t);
    let mut out = Vec::new();
    for (i, o) in c.objects.iter().enumerate() {
        let (Some(tid), "table") = (o.id, base_kind(o)) else {
            continue;
        };
        let Some((_, schema)) = table_of(&c, i) else {
            out.push(shell_entries(o, t, &|id| {
                vec![(1, Value::Integer(id as i32)), (2, Value::Integer(1))]
            }));
            continue;
        };
        for f in &schema.fields {
            let mut r = blank.row();
            r[pos(t, 1)] = Value::Integer(tid as i32);
            r[pos(t, 2)] = Value::Integer(f.id as i32);
            match field_row(&c, o, schema, f, t, &blank, &mut r) {
                Ok(()) => out.push(Entry::ok(r)),
                Err(why) => {
                    let mut k = blank.row();
                    k[pos(t, 1)] = Value::Integer(tid as i32);
                    k[pos(t, 2)] = Value::Integer(f.id as i32);
                    out.push(Entry::poisoned(k, why));
                }
            }
        }
    }
    out
}

fn field_row(
    c: &Catalog,
    table: &Object,
    schema: &al_ir::Table,
    f: &Field,
    t: &TableDef,
    blank: &Blank,
    r: &mut al_data::Row,
) -> Result<(), String> {
    let what = || format!("field '{}' of table '{}'", f.name, table.name);
    let fprop = |name: &str| {
        f.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    };
    r[pos(t, 4)] = Value::Text(f.name.clone());
    r[pos(t, 5)] = blank.option(
        t,
        5,
        type_ordinal(&f.ty).ok_or_else(|| format!("{}: the type is not understood", what()))?,
    );
    // Class: Normal, FlowField, FlowFilter (TestFieldDataClassificationInheritance.al, the FlowField case).
    let is_flow_field = !f.stored && !f.flow_filter;
    // OptionString: the member names of an option or enum field, comma-joined (TestFieldEnumTypeVirtualTable.al:19-47).
    let option_string = match &f.ty {
        RtType::Option(m) => m.join(","),
        RtType::Enum(e) => c.objects[*e as usize]
            .enum_values
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    r[pos(t, 24)] = Value::Text(option_string);
    // ObsoleteState and ObsoleteReason (TestFieldObsoleteStateVirtualTable.al:18-45).
    let state = match fprop("ObsoleteState") {
        None => 0,
        Some(v) => ordinal_of(OBSOLETE_STATES, v)
            .ok_or_else(|| format!("{}: ObsoleteState `{v}` is not understood", what()))?,
    };
    r[pos(t, 25)] = blank.option(t, 25, state);
    r[pos(t, 26)] = Value::Text(fprop("ObsoleteReason").and_then(string_lit).unwrap_or_default());
    // DataClassification: the field's own; else the table's; a FlowField is SystemMetadata and a
    // silent Blob is CustomerContent (TestFieldDataClassificationInheritance.al:17-50).
    let is_blob = matches!(&f.ty, RtType::Library(n) if n.eq_ignore_ascii_case("blob"));
    let dc = match (fprop("DataClassification"), is_flow_field, is_blob) {
        (_, true, _) => 6,
        (Some(v), _, _) => ordinal_of(DATA_CLASSIFICATIONS, v)
            .ok_or_else(|| format!("{}: DataClassification `{v}` is not understood", what()))?,
        (None, _, true) => 0,
        (None, _, false) => match prop(table, "DataClassification") {
            None => 0,
            Some(v) => ordinal_of(DATA_CLASSIFICATIONS, v)
                .ok_or_else(|| format!("table '{}': DataClassification `{v}` is not understood", table.name))?,
        },
    };
    r[pos(t, 27)] = blank.option(t, 27, dc);
    // RelationTableNo and RelationFieldNo (TestFieldDataClassificationVirtualTable.al:50-72).
    match f.relation_arms.as_slice() {
        [] if f.relation => return Err(format!("{}: the TableRelation is not understood", what())),
        [] => {}
        [arm] => {
            let Some(rt) = arm.table else {
                return Err(format!(
                    "{}: the related table `{}` is not compiled",
                    what(),
                    arm.table_name
                ));
            };
            let related = &c.objects[rt];
            let Some(related_schema) = &related.table else {
                return Err(format!(
                    "{}: the related table `{}` is not compiled",
                    what(),
                    arm.table_name
                ));
            };
            let field_pos = arm.field.unwrap_or_else(|| {
                related_schema
                    .keys
                    .first()
                    .and_then(|k| k.first())
                    .copied()
                    .unwrap_or(0)
            });
            r[pos(t, 21)] = Value::Integer(related.id.unwrap_or(0) as i32);
            r[pos(t, 22)] = Value::Integer(related_schema.fields.get(field_pos).map_or(0, |x| x.id) as i32);
        }
        _ => {
            return Err(format!(
                "{}: a conditional TableRelation (which table it reports is not pinned)",
                what()
            ));
        }
    }
    let _ = schema;
    Ok(())
}

pub(super) fn key_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    let blank = Blank::new(t);
    let mut out = Vec::new();
    for (i, o) in c.objects.iter().enumerate() {
        let (Some(tid), "table") = (o.id, base_kind(o)) else {
            continue;
        };
        let Some((_, schema)) = table_of(&c, i) else {
            out.push(shell_entries(o, t, &|id| {
                vec![(1, Value::Integer(id as i32)), (2, Value::Integer(1))]
            }));
            continue;
        };
        for (n, key) in schema.keys.iter().enumerate() {
            let props = schema.key_props.get(n).map_or(&[][..], Vec::as_slice);
            let kprop = |name: &str| {
                props
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.trim())
            };
            let mut r = blank.row();
            r[pos(t, 1)] = Value::Integer(tid as i32);
            r[pos(t, 2)] = Value::Integer(n as i32 + 1);
            // The fields, comma-separated in declaration order (TestKeyVirtualTable.al:73-82).
            r[pos(t, 4)] = Value::Text(
                key.iter()
                    .map(|&f| schema.fields[f].name.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            r[pos(t, 7)] = Value::Boolean(kprop("Enabled").is_none_or(truthy));
            r[pos(t, 10)] = Value::Boolean(n == 0 || kprop("Clustered").is_some_and(truthy));
            r[pos(t, 13)] = Value::Boolean(schema.unique_keys.get(n).copied().unwrap_or(false));
            out.push(Entry::ok(r));
        }
        // BC adds a unique key on the SystemId after the declared ones (TestKeyVirtualTable.al:50-56).
        let mut r = blank.row();
        r[pos(t, 1)] = Value::Integer(tid as i32);
        r[pos(t, 2)] = Value::Integer(schema.keys.len() as i32 + 1);
        r[pos(t, 4)] = Value::Text("$systemId".into());
        r[pos(t, 7)] = Value::Boolean(true);
        r[pos(t, 13)] = Value::Boolean(true);
        out.push(Entry::ok(r));
    }
    out
}
