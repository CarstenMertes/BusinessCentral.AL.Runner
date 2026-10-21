//! The one version line, shared by `--version` and the run header.

/// `alrunner <version> (<commit>)`. The version is the Cargo version, plus the build suffix
/// (`+edge.YYYYMMDD`) on an edge build.
pub fn line() -> String {
    format!("alrunner {} ({})", env!("ALRUNNER_VERSION"), env!("ALRUNNER_COMMIT"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_with_the_cargo_version_and_has_the_commit() {
        let l = line();
        assert!(l.starts_with(&format!("alrunner {}", env!("CARGO_PKG_VERSION"))), "{l}");
        assert!(l.ends_with(&format!(" ({})", env!("ALRUNNER_COMMIT"))), "{l}");
    }

    #[test]
    fn the_version_is_the_cargo_version_plus_an_optional_build_suffix() {
        let v = env!("ALRUNNER_VERSION");
        let rest = v.strip_prefix(env!("CARGO_PKG_VERSION")).expect(v);
        assert!(rest.is_empty() || rest.starts_with('+'), "{v}");
    }
}
