//! `oracle/oracle.toml`: the corpus pin and one `[[version]]` table per BC major
//! (ADR-015 3.5).

use al_version::BcVersion;

#[derive(Debug, serde::Deserialize)]
pub struct OracleFile {
    pub corpus_repo: String,
    pub corpus_app: String,
    pub corpus_commit: String,
    #[serde(rename = "version")]
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct VersionEntry {
    pub major: u16,
    /// The corpus CI leg the oracle came from (the newest minor of the major).
    pub bc_version: String,
    /// The corpus commit this oracle was recorded for, when it is older than the pin. Absent
    /// means the pin. Accepted only when the corpus diff since then adds files and changes none
    /// (`pin::check`, ADR-007).
    pub corpus_commit: Option<String>,
    /// The full build, once the corpus records it.
    pub bc_build: Option<String>,
    pub workflow_run: Option<u64>,
    pub artifact_id: Option<u64>,
    /// JUnit path, relative to the directory of `oracle.toml`.
    pub junit: String,
    /// Ratchet path, relative to the repository root.
    pub baseline: String,
}

impl VersionEntry {
    /// The corpus commit this version's oracle belongs to.
    pub fn oracle_commit<'a>(&'a self, pin: &'a str) -> &'a str {
        self.corpus_commit.as_deref().unwrap_or(pin)
    }

    /// The BC version of the oracle leg, with the build when it is known.
    pub fn bc(&self) -> BcVersion {
        let mut v = BcVersion::parse(&self.bc_version).unwrap_or_else(|_| BcVersion::new(self.major, 0));
        if let Some(b) = self.bc_build.as_deref().and_then(|b| BcVersion::parse(b).ok()) {
            v.build = b.build;
        }
        v
    }
}

impl OracleFile {
    pub fn parse(text: &str) -> Result<OracleFile, String> {
        let f: OracleFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if f.versions.is_empty() {
            return Err("no [[version]] table".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for v in &f.versions {
            let leg = BcVersion::parse(&v.bc_version)
                .map_err(|e| format!("version {}: bc_version `{}`: {e}", v.major, v.bc_version))?;
            if leg.major != v.major {
                return Err(format!("bc_version `{}` is not in major {}", v.bc_version, v.major));
            }
            if !seen.insert(v.major) {
                return Err(format!("major {} is listed twice", v.major));
            }
        }
        Ok(f)
    }

    /// The entry for `--bc-version PREFIX`, or the newest entry without a flag.
    pub fn select(&self, requested: Option<&str>) -> Result<&VersionEntry, String> {
        let Some(prefix) = requested else {
            return Ok(self
                .versions
                .iter()
                .max_by_key(|v| v.major)
                .expect("parse checks for a table"));
        };
        let wanted = BcVersion::parse(prefix).map_err(|e| format!("--bc-version `{prefix}`: {e}"))?;
        if wanted.major < al_version::MIN_BC_MAJOR {
            return Err(format!(
                "BC {prefix} is not supported: BC {} and later only",
                al_version::MIN_BC_MAJOR
            ));
        }
        self.versions
            .iter()
            .filter(|v| v.bc().matches_prefix(prefix))
            .max_by_key(|v| v.major)
            .ok_or_else(|| {
                let mut known: Vec<_> = self.versions.iter().map(|v| (v.major, v.bc().to_string())).collect();
                known.sort();
                let known: Vec<_> = known.into_iter().map(|(_, s)| s).collect();
                format!("no oracle for BC {prefix}; known versions: {}", known.join(", "))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = r#"
corpus_repo   = "corpus"
corpus_app    = "corpus/tests/al-language"
corpus_commit = "abc"

[[version]]
major = 27
bc_version = "27.5"
junit = "27/junit.xml"
baseline = "metrics/27/passing.txt"

[[version]]
major = 29
bc_version = "29.0"
bc_build = "29.0.54011.55816"
workflow_run = 1
artifact_id = 2
junit = "29/junit.xml"
baseline = "metrics/29/passing.txt"

[[version]]
major = 28
bc_version = "28.5"
junit = "28/junit.xml"
baseline = "metrics/28/passing.txt"
"#;

    fn file() -> OracleFile {
        OracleFile::parse(TOML).unwrap()
    }

    #[test]
    fn parses_every_version_table() {
        let f = file();
        assert_eq!(f.versions.len(), 3);
        assert_eq!(f.versions[1].artifact_id, Some(2));
        assert_eq!(f.versions[0].bc_build, None);
    }

    #[test]
    fn a_version_may_record_an_older_corpus_commit() {
        let t = TOML.replace(
            "bc_build = \"29.0.54011.55816\"",
            "bc_build = \"29.0.54011.55816\"\ncorpus_commit = \"old\"",
        );
        let f = OracleFile::parse(&t).unwrap();
        assert_eq!(f.versions[1].oracle_commit("abc"), "old");
        assert_eq!(f.versions[0].oracle_commit("abc"), "abc");
    }

    #[test]
    fn no_flag_selects_the_newest_version() {
        assert_eq!(file().select(None).unwrap().major, 29);
    }

    #[test]
    fn a_prefix_selects_by_whole_segments() {
        let f = file();
        assert_eq!(f.select(Some("28")).unwrap().major, 28);
        assert_eq!(f.select(Some("27.5")).unwrap().major, 27);
        assert_eq!(f.select(Some("29.0.54011")).unwrap().major, 29);
    }

    #[test]
    fn an_unknown_prefix_lists_the_known_versions() {
        let e = file().select(Some("30")).unwrap_err();
        assert!(e.contains("no oracle for BC 30"), "{e}");
        assert!(e.contains("27.5") && e.contains("28.5") && e.contains("29.0"), "{e}");
        assert!(file().select(Some("27.50")).is_err());
        assert!(file().select(Some("27.0")).is_err(), "the oracle is the 27.5 leg");
    }

    #[test]
    fn a_version_below_27_is_not_supported() {
        let e = file().select(Some("26")).unwrap_err();
        assert!(e.contains("27 and later only"), "{e}");
    }

    #[test]
    fn a_garbage_flag_is_an_error() {
        assert!(file().select(Some("abc")).is_err());
    }

    #[test]
    fn the_leg_build_is_kept_when_recorded() {
        let f = file();
        assert_eq!(f.versions[1].bc().build.as_deref(), Some("29.0.54011.55816"));
        assert_eq!(f.versions[0].bc().to_string(), "27.5");
    }

    #[test]
    fn rejects_duplicate_majors_and_a_major_that_disagrees_with_the_leg() {
        let dup = TOML.replace("major = 28", "major = 27").replace("28.5", "27.4");
        assert!(OracleFile::parse(&dup).unwrap_err().contains("twice"));
        let bad = TOML.replace("bc_version = \"28.5\"", "bc_version = \"29.5\"");
        assert!(OracleFile::parse(&bad).unwrap_err().contains("28"));
        let none = "corpus_repo=\"c\"\ncorpus_app=\"a\"\ncorpus_commit=\"x\"\n";
        assert!(OracleFile::parse(none).is_err());
    }
}
