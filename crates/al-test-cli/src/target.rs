//! Reads the version fields of an app.json (ADR-015 step 4).

use al_version::AppVersions;
use std::path::Path;

/// `runtime`, `platform` and `application` of `dir/app.json`. A field that is absent stays `None`.
pub fn read_app_versions(dir: &Path) -> Result<AppVersions, String> {
    let path = dir.join("app.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    app_versions_from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn app_versions_from_json(text: &str) -> Result<AppVersions, String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let field = |k: &str| json.get(k).and_then(serde_json::Value::as_str);
    AppVersions::parse(field("runtime"), field("platform"), field("application")).map_err(|e| format!("app.json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_version::{BcVersion, Runtime};

    #[test]
    fn reads_runtime_platform_and_application() {
        let v = app_versions_from_json(r#"{"id":"x","runtime":"16.0","platform":"27.0.0.0","application":"27.1.0.0"}"#)
            .unwrap();
        assert_eq!(v.runtime, Some(Runtime { major: 16, minor: 0 }));
        assert_eq!(v.platform, Some(BcVersion::new(27, 0)));
        assert_eq!(v.floor(), Some(&BcVersion::new(27, 1)));
    }

    #[test]
    fn absent_fields_stay_absent() {
        assert_eq!(app_versions_from_json(r#"{"id":"x"}"#).unwrap(), AppVersions::default());
    }

    #[test]
    fn a_bad_runtime_is_an_error() {
        assert!(app_versions_from_json(r#"{"runtime":"soon"}"#).is_err());
        assert!(app_versions_from_json("{").is_err());
    }

    #[test]
    fn reads_the_pinned_corpus_app() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests/al-language");
        let v = read_app_versions(&dir).unwrap();
        assert_eq!(v.runtime, Some(Runtime { major: 16, minor: 0 }), "ADR-015 2.0");
    }
}
