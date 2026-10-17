//! Time zone data for the `TimeZoneInfo` stand-in (ADR-038 amendment 2026-10-17).
//!
//! Windows ids come from the generated CLDR table; zone rules come from `jiff` with its bundled tz database, so no file
//! is read from the machine (ADR-006).

use crate::windows_zones::WINDOWS_ZONES;

pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Windows ids whose base offset jiff does not give the way .NET on the oracle machine does. Found by the differential
/// test (`dotnet_diff.rs`); these go to the sidecar. Empty while the test finds none.
pub const FORWARDED_IDS: &[&str] = &[];

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
    let mut base = None;
    for ts in ["2037-01-15T12:00:00Z", "2037-07-15T12:00:00Z"] {
        let ts: jiff::Timestamp = ts.parse().ok()?;
        let info = tz.to_offset_info(ts);
        if !info.dst().is_dst() {
            base = Some(info.offset());
        }
    }
    base.map(|o| i64::from(o.seconds()) * TICKS_PER_SECOND)
}
