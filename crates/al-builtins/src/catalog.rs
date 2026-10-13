//! Generating the signature catalog from the docs scrape (ADR-008).

use crate::signature::{DocEntry, Signature};
use std::collections::BTreeMap;

/// All overloads in `al-surface.json`, one catalog line each, sorted. Entries
/// whose syntax line does not parse are returned as errors, never dropped.
pub fn generate(surface_json: &str) -> Result<Vec<String>, String> {
    let surface: BTreeMap<String, BTreeMap<String, Vec<DocEntry>>> =
        serde_json::from_str(surface_json).map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for (ty, methods) in &surface {
        for (key, overloads) in methods {
            for o in overloads {
                match Signature::from_doc(o) {
                    Ok(s) => lines.push(s.to_line()),
                    Err(e) => errors.push(format!("{ty}.{key}: {e}")),
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    lines.sort();
    lines.dedup();
    Ok(lines)
}
