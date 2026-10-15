//! Tables whose rows are facts about the host: Time Zone and Windows Language.

use super::mat::Entry;
use super::{Ident, blank, pos};
use al_builtins::value::Value;
use al_data::TableDef;

/// The Time Zone table lists the host's zones, `No.` from 1 (record/TestTimeZoneVirtualTable.al:
/// more than one row, `No.` running 1 to N without gaps, a non-blank `ID`, `Get(1)` the first row of
/// a scan, `Get` past the end finds nothing).
pub(super) fn time_zone_rows(t: &TableDef, i: &Ident) -> Vec<Entry> {
    i.time_zones
        .iter()
        .enumerate()
        .map(|(n, z)| {
            let mut r = blank(t);
            r[pos(t, 1)] = Value::Text(z.id.clone());
            r[pos(t, 2)] = Value::Text(z.display_name.clone());
            r[pos(t, 3)] = Value::Integer(n as i32 + 1);
            Entry::ok(r)
        })
        .collect()
}

/// The Feature Key table of the BC 27.5 platform: 16 rows, `Data Update Required` set for three of them
/// (record/TestFieldLenAndFeatureKeyContracts.al:83-125) and `Enabled` None or All Users per row
/// (record/TestFeatureKeyEnabledContracts.al:FeatureKey_Enabled_PerId_On27; the state is per tenant, and the
/// corpus image is the 27.3 and 27.5 one). `Mandatory`, `Will Be Enabled On` and the other columns are not pinned. The rows
/// of other majors were not measured: one poisoned entry stands for all of them, so a read is refused.
pub(super) fn feature_key_rows(t: &TableDef, target: Option<&al_version::Target>) -> Vec<Entry> {
    const ROWS: &[(&str, bool, bool)] = &[
        ("AdvancedTellMe", false, false),
        ("CalcOnlyVisibleFlowFields", false, true),
        ("ConcealedMaskType", false, true),
        ("ConcurrentInventoryPosting", false, true),
        ("ConcurrentJobPosting", false, true),
        ("ConcurrentResourcePosting", false, true),
        ("DisableSoapWebServicesOnMicrosoftUIPages", false, true),
        ("EnableMcpAccess", false, true),
        ("FinancialReportDefaults", false, true),
        ("FullTextSearch", false, false),
        ("GLCurrencyRevaluation", true, false),
        ("Manufacturing_FlushingMethod_ActivateManualWoPick", true, false),
        ("ReminderTermsCommunicationTexts", false, true),
        ("SalesPrices", true, false),
        ("SaveValueToDatabasePromptly", false, true),
        ("SemanticMetadataSearch", false, true),
    ];
    if !al_version::feature_key_rows_known(target) {
        return vec![Entry::poisoned_prefix(
            blank(t),
            0,
            "Feature Key: its rows are measured for one BC major only (docs/probes-needed.md)".into(),
        )];
    }
    ROWS.iter()
        .map(|(id, data_update, all_users)| {
            let mut r = blank(t);
            r[pos(t, 1)] = Value::Text((*id).into());
            // Enabled: None or All Users, as the 27.3 and 27.5 service tiers read it
            // (record/TestFeatureKeyEnabledContracts.al:FeatureKey_Enabled_PerId_On27).
            r[pos(t, 2)] = option(t, 2, i32::from(*all_users));
            r[pos(t, 8)] = Value::Boolean(*data_update);
            Entry::ok(r)
        })
        .collect()
}

// ---- Windows Language -----------------------------------------------------------------------------

use super::{option, ranges, refused};
use al_builtins::filter::FieldFilter;
use al_data::{Row, Scan, StoreError};

/// The languages the corpus pins, as (LCID, name, abbreviated name, primary code page, tag). The table
/// lists every language of the host, which is not pinned beyond these three rows
/// (record/TestWindowsLanguageTable.al:19-60); a read that would have to answer for any other language
/// of the Windows range is refused.
const LANGUAGES: &[(i32, &str, &str, &str, &str)] = &[
    (1031, "German (Germany)", "DEU", "850", "de-DE"),
    (1033, "English (United States)", "ENU", "437", "en-US"),
    (2057, "English (United Kingdom)", "ENG", "850", "en-GB"),
];
/// The ids a culture can have: a number above this is no language (TestWindowsLanguageTable.al:63-70
/// asks for 999999 and expects no row).
const MAX_LCID: i64 = 0xFFFF;
const LANGUAGE_ID: u32 = 1;

fn num(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i64::from(*i)),
        _ => None,
    }
}

fn language_row(t: &TableDef, lcid: i32, name: &str, abbr: &str, code_page: &str, tag: &str) -> Row {
    let mut r = blank(t);
    r[pos(t, 1)] = Value::Integer(lcid);
    // The primary language is the low ten bits of the LCID.
    r[pos(t, 2)] = Value::Integer(lcid & 0x3FF);
    r[pos(t, 10)] = Value::Text(name.into());
    r[pos(t, 11)] = Value::Text(abbr.into());
    r[pos(t, 30)] = Value::Text(code_page.into());
    r[pos(t, 35)] = Value::Text(tag.into());
    let _ = option;
    r
}

fn known(t: &TableDef, lcid: i64) -> Option<Row> {
    LANGUAGES
        .iter()
        .find(|l| i64::from(l.0) == lcid)
        .map(|l| language_row(t, l.0, l.1, l.2, l.3, l.4))
}

fn unknown_in(set: &[ranges::Iv]) -> Option<i64> {
    let known_ids: Vec<i64> = LANGUAGES.iter().map(|l| i64::from(l.0)).collect();
    for &(a, b) in set {
        let (a, b) = (a.max(1), b.min(MAX_LCID));
        if a > b {
            continue;
        }
        // The first id of the interval that is not a known row.
        let mut id = a;
        while id <= b {
            if !known_ids.contains(&id) {
                return Some(id);
            }
            id += 1;
        }
    }
    None
}

fn language_set(t: &TableDef, filters: &[FieldFilter]) -> Result<ranges::Set, StoreError> {
    ranges::of_field(filters, pos(t, LANGUAGE_ID), &num, i64::MIN / 4, i64::MAX / 4)
        .map_err(|e| refused(format!("Windows Language: {e}")))
}

pub(super) fn windows_language_get(t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
    let id = pk
        .first()
        .and_then(num)
        .ok_or_else(|| refused("Windows Language: a key that is not a number"))?;
    if !(1..=MAX_LCID).contains(&id) {
        return Ok(None);
    }
    match known(t, id) {
        Some(r) => Ok(Some(r)),
        None => Err(refused(format!(
            "Windows Language: whether language {id} is a row is not pinned (only 1031, 1033 and 2057 are)"
        ))),
    }
}

pub(super) fn windows_language_next(t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
    let set = language_set(t, scan.filters)?;
    if let Some(id) = unknown_in(&set) {
        return Err(refused(format!(
            "Windows Language: the filter takes in language {id}, whose row is not pinned (only 1031, 1033 and 2057 are)"
        )));
    }
    let after = scan.after.and_then(|a| a.first()).and_then(num);
    let mut cur = after;
    while let Some(id) = ranges::step(&set, cur, scan.ascending) {
        if let Some(r) = known(t, id) {
            if scan.filters.iter().all(|f| f.expr.matches(&r[f.field])) {
                return Ok(Some(r));
            }
        }
        cur = Some(id);
    }
    Ok(None)
}

pub(super) fn windows_language_count(t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError> {
    let set = language_set(t, filters)?;
    if let Some(id) = unknown_in(&set) {
        return Err(refused(format!(
            "Windows Language: the filter takes in language {id}, whose row is not pinned (only 1031, 1033 and 2057 are)"
        )));
    }
    Ok(LANGUAGES
        .iter()
        .filter_map(|l| known(t, i64::from(l.0)))
        .filter(|r| filters.iter().all(|f| f.expr.matches(&r[f.field])))
        .count())
}
