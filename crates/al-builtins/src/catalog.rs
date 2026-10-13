//! Generating the signature catalog from the docs scrape (ADR-008).

use crate::signature::{DocEntry, Signature};
use std::collections::{BTreeMap, HashMap};

/// The checked-in catalog, compiled into the binary (no file access at runtime, ADR-006).
const SIGNATURES: &str = include_str!("../catalog/signatures.txt");

/// Receivers whose static methods can be called without a qualifier
/// (`Message(...)`, `Format(...)`, `CopyStr(...)`, `Commit()`, `BindSubscription(...)`).
pub const GLOBAL_RECEIVERS: &[&str] = &["System", "Dialog", "Text", "Database", "Session"];

pub struct Catalog {
    pub signatures: Vec<Signature>,
    by_method: HashMap<(String, String), Vec<usize>>,
    globals: HashMap<String, Vec<usize>>,
}

impl Catalog {
    pub fn load() -> Catalog {
        let signatures: Vec<Signature> = SIGNATURES
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| Signature::from_line(l).unwrap_or_else(|e| panic!("bad catalog line `{l}`: {e}")))
            .collect();
        let mut by_method: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut globals: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, s) in signatures.iter().enumerate() {
            by_method
                .entry((s.receiver.to_lowercase(), s.name.to_lowercase()))
                .or_default()
                .push(i);
            if !s.instance && GLOBAL_RECEIVERS.iter().any(|g| g.eq_ignore_ascii_case(&s.receiver)) {
                globals.entry(s.name.to_lowercase()).or_default().push(i);
            }
        }
        Catalog {
            signatures,
            by_method,
            globals,
        }
    }

    /// Overloads of `receiver.method` (case-insensitive), static and instance.
    pub fn overloads(&self, receiver: &str, method: &str) -> Vec<&Signature> {
        self.by_method
            .get(&(receiver.to_lowercase(), method.to_lowercase()))
            .map(|v| v.iter().map(|&i| &self.signatures[i]).collect())
            .unwrap_or_default()
    }

    /// Overloads callable unqualified as `name(...)`.
    pub fn global(&self, name: &str) -> Vec<&Signature> {
        self.globals
            .get(&name.to_lowercase())
            .map(|v| v.iter().map(|&i| &self.signatures[i]).collect())
            .unwrap_or_default()
    }

    /// Stable overload id: `Receiver.Method#n`, n counting from 1 in catalog order.
    pub fn id(&self, sig: &Signature) -> String {
        let all = &self.by_method[&(sig.receiver.to_lowercase(), sig.name.to_lowercase())];
        let n = all
            .iter()
            .position(|&i| &self.signatures[i] == sig)
            .expect("signature from this catalog")
            + 1;
        format!("{}.{}#{n}", sig.receiver, sig.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_line_round_trips() {
        for l in SIGNATURES.lines().filter(|l| !l.starts_with('#')) {
            assert_eq!(Signature::from_line(l).unwrap().to_line(), l);
        }
    }

    #[test]
    fn lookup_by_receiver_and_method_is_case_insensitive() {
        let c = Catalog::load();
        assert_eq!(c.overloads("record", "INSERT").len(), 3);
        assert!(c.overloads("Record", "NoSuchMethod").is_empty());
    }

    #[test]
    fn globals_are_static_methods_of_the_global_receivers() {
        let c = Catalog::load();
        assert!(!c.global("Message").is_empty());
        assert!(!c.global("CopyStr").is_empty());
        assert!(!c.global("Commit").is_empty());
        assert!(c.global("Insert").is_empty(), "instance methods are not global");
        assert!(c.global("Open").is_empty(), "Dialog.Open is an instance method");
    }

    #[test]
    fn overload_ids_are_stable_and_distinct() {
        let c = Catalog::load();
        let ids: Vec<String> = c.overloads("Record", "Insert").iter().map(|s| c.id(s)).collect();
        assert_eq!(ids, vec!["Record.Insert#1", "Record.Insert#2", "Record.Insert#3"]);
    }
}

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
