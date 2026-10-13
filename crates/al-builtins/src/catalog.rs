//! Generating the signature catalog from the docs scrape (ADR-008).

use crate::signature::{DocEntry, Signature};
use std::collections::{BTreeMap, HashMap};

/// The checked-in catalog, compiled into the binary (no file access at runtime, ADR-006).
const SIGNATURES: &str = include_str!("../catalog/signatures.txt");
const OVERRIDES: &str = include_str!("../catalog/overrides.txt");

/// Applies `overrides` to the generated `lines`. Every change must follow a
/// `#` evidence comment, and every removed line must exist.
pub fn apply_overrides(lines: &[&str], overrides: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let mut has_evidence = false;
    for (n, raw) in overrides.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            has_evidence = false;
        } else if line.starts_with('#') {
            has_evidence = true;
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("+ ")) {
            if !has_evidence {
                return Err(format!(
                    "overrides.txt:{}: change without a preceding `#` evidence comment",
                    n + 1
                ));
            }
            if line.starts_with('-') {
                let i = out
                    .iter()
                    .position(|l| l == rest)
                    .ok_or_else(|| format!("overrides.txt:{}: `{rest}` is not in the catalog", n + 1))?;
                out.remove(i);
            } else {
                out.push(rest.to_string());
            }
        } else {
            return Err(format!("overrides.txt:{}: expected `#`, `- ` or `+ `", n + 1));
        }
    }
    out.sort();
    Ok(out)
}

/// Receivers whose static methods can be called without a qualifier
/// (`Message(...)`, `Format(...)`, `CopyStr(...)`, `Commit()`, `BindSubscription(...)`).
/// `SecretText` is here because corpus types/TestSecretText.al:73 calls
/// `SecretStrSubstNo(...)` unqualified and BC compiles it.
pub const GLOBAL_RECEIVERS: &[&str] = &["System", "Dialog", "Text", "Database", "Session", "SecretText"];

pub struct Catalog {
    pub signatures: Vec<Signature>,
    by_method: HashMap<(String, String), Vec<usize>>,
    globals: HashMap<String, Vec<usize>>,
}

impl Catalog {
    pub fn load() -> Catalog {
        let base: Vec<&str> = SIGNATURES
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .collect();
        let lines = apply_overrides(&base, OVERRIDES).unwrap_or_else(|e| panic!("{e}"));
        let signatures: Vec<Signature> = lines
            .iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_remove_and_add_lines() {
        let out = apply_overrides(&["A.x inst ()", "A.y inst ()"], "# why\n- A.x inst ()\n+ A.z inst ()\n").unwrap();
        assert_eq!(out, vec!["A.y inst ()", "A.z inst ()"]);
    }

    #[test]
    fn override_without_evidence_is_rejected() {
        assert!(apply_overrides(&["A.x inst ()"], "- A.x inst ()\n").is_err());
        assert!(
            apply_overrides(&["A.x inst ()"], "# why\n- A.x inst ()\n\n+ A.z inst ()\n").is_err(),
            "blank line ends a block"
        );
    }

    #[test]
    fn removing_a_line_that_does_not_exist_is_rejected() {
        assert!(apply_overrides(&["A.x inst ()"], "# why\n- A.nope inst ()\n").is_err());
    }

    #[test]
    fn checked_in_overrides_apply_and_parse() {
        let base: Vec<&str> = SIGNATURES.lines().filter(|l| !l.starts_with('#')).collect();
        for l in apply_overrides(&base, OVERRIDES).unwrap() {
            Signature::from_line(&l).unwrap();
        }
        let c = Catalog::load();
        assert!(
            c.overloads("Codeunit", "Run")
                .iter()
                .any(|s| s.instance && s.params[0].optional)
        );
    }

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
