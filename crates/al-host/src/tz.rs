//! Time zone data for the `TimeZoneInfo` stand-in (ADR-038 amendment 2026-10-17).
//!
//! Windows ids come from the generated CLDR table; zone rules come from `jiff` with its bundled tz database, so no file
//! is read from the machine (ADR-006).

use crate::windows_zones::WINDOWS_ZONES;

pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Windows ids that go to the sidecar because hosts disagree on them. The IANA zone a Windows id maps to comes from the
/// ICU of the machine that runs .NET, and ICU releases carry different CLDR versions. Compared CLDR 42 to 48 (ICU 72 to
/// 78, the range of current Linux distributions): these two ids map to different zones (found by the CI failure of
/// pull request 223: `Central Asia Standard Time` is Almaty, +5, in CLDR 44 and Bishkek, +6, in CLDR 45 and later).
pub const FORWARDED_IDS: &[&str] = &["Central Asia Standard Time", "Mountain Standard Time (Mexico)"];

/// The IANA id behind a Windows id of the table, or for `UTC`.
fn iana_of(windows_id: &str) -> Option<&'static str> {
    WINDOWS_ZONES
        .binary_search_by(|(w, _)| (*w).cmp(windows_id))
        .ok()
        .map(|i| WINDOWS_ZONES[i].1)
}

/// `TimeZoneInfo.FindSystemTimeZoneById(id).BaseUtcOffset` in ticks for an id served in process, `None` when the
/// sidecar must answer (an id not in the table, a forwarded id, or a zone the bundled data does not know).
pub fn base_utc_offset_ticks(windows_id: &str) -> Option<i64> {
    if FORWARDED_IDS.contains(&windows_id) {
        return None;
    }
    let iana = iana_of(windows_id)?;
    let tz = jiff::tz::TimeZone::get(iana).ok()?;
    // The standard (non-DST) offset in force in the latest rules: the offsets of January and July 2037, which the
    // POSIX rule at the end of the data covers; the one without DST is the base.
    let base_at = |year: i16| -> Option<i32> {
        let mut base = None;
        for month in ["01", "07"] {
            let ts: jiff::Timestamp = format!("{year}-{month}-15T12:00:00Z").parse().ok()?;
            let info = tz.to_offset_info(ts);
            if !info.dst().is_dst() {
                base = Some(info.offset().seconds());
            }
        }
        base
    };
    let latest = base_at(2037)?;
    // A zone whose standard offset changed in the years when tz releases still differ between hosts (a government changed
    // its zone: Kazakhstan 2024, Mexico 2022, Paraguay 2024 ...) goes to the sidecar, which has the host's own data.
    if [2015, 2019, 2022, 2023, 2024, 2025]
        .iter()
        .any(|&y| base_at(y) != Some(latest))
    {
        return None;
    }
    Some(i64::from(latest) * TICKS_PER_SECOND)
}
