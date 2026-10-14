//! The key of a cache entry: every input of the compile of the dependency set.

use sha2::{Digest, Sha256};

/// The format integer of the entry file. Bump it when the layout of the file or of a type it
/// stores changes. (Any change to the compiler already changes `Key::runtime_build`.)
pub const FORMAT: u32 = 2;

/// One dependency app, as the compile saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppKey {
    pub id: String,
    pub name: String,
    pub dependencies: Vec<String>,
    pub publisher: String,
    pub version: String,
    /// A hash of the app's source, made by the caller.
    pub source: [u8; 32],
}

/// Everything the lowered form of the dependency set depends on. The digest destructures the
/// struct without `..`, so a field added here and not hashed does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// [`FORMAT`] when the key is made.
    pub format: u32,
    /// A hash of the running executable: the compiler, the builtins catalog and every crate.
    pub runtime_build: [u8; 32],
    /// The target BC version and runtime (`Target`, ADR-015), as text. Empty: none.
    pub target: String,
    /// The preprocessor symbols the sources were parsed with, sorted.
    pub symbols: Vec<String>,
    /// The dependency apps in program order (dependencies first).
    pub apps: Vec<AppKey>,
}

fn put(h: &mut Sha256, bytes: &[u8]) {
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn put_list<S: AsRef<[u8]>>(h: &mut Sha256, items: &[S]) {
    h.update((items.len() as u64).to_le_bytes());
    for i in items {
        put(h, i.as_ref());
    }
}

impl Key {
    /// A key with the current [`FORMAT`].
    pub fn new(runtime_build: [u8; 32], target: String, mut symbols: Vec<String>, apps: Vec<AppKey>) -> Key {
        symbols.sort();
        Key {
            format: FORMAT,
            runtime_build,
            target,
            symbols,
            apps,
        }
    }

    /// The names of the fields, for the test that every field has a miss test.
    pub const FIELDS: [&'static str; 5] = ["format", "runtime_build", "target", "symbols", "apps"];

    /// The digest of the key. Equal keys give equal digests; a key that differs in any field
    /// gives a different one.
    pub fn digest(&self) -> [u8; 32] {
        let Key {
            format,
            runtime_build,
            target,
            symbols,
            apps,
        } = self;
        let mut h = Sha256::new();
        h.update(b"al-runtime compile cache key");
        h.update(format.to_le_bytes());
        put(&mut h, runtime_build);
        put(&mut h, target.as_bytes());
        put_list(&mut h, symbols);
        h.update((apps.len() as u64).to_le_bytes());
        for a in apps {
            let AppKey {
                id,
                name,
                dependencies,
                publisher,
                version,
                source,
            } = a;
            put(&mut h, id.as_bytes());
            put(&mut h, name.as_bytes());
            put_list(&mut h, dependencies);
            put(&mut h, publisher.as_bytes());
            put(&mut h, version.as_bytes());
            put(&mut h, source);
        }
        h.finalize().into()
    }

    /// The digest as lower-case hex.
    pub fn hex(&self) -> String {
        self.digest().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The file name of the entry for this key.
    pub fn file_name(&self) -> String {
        format!("deps-{}.alcc", &self.hex()[..32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Key {
        Key::new(
            [7; 32],
            "BC 27.5 runtime 16.0".into(),
            vec!["CLEAN27".into(), "CLEAN26".into()],
            vec![
                AppKey {
                    id: "a".into(),
                    name: "Base".into(),
                    dependencies: vec!["s".into()],
                    publisher: "Microsoft".into(),
                    version: "27.5.0.0".into(),
                    source: [1; 32],
                },
                AppKey {
                    id: "b".into(),
                    name: "App".into(),
                    dependencies: vec!["a".into()],
                    publisher: "Microsoft".into(),
                    version: "27.5.0.0".into(),
                    source: [2; 32],
                },
            ],
        )
    }

    /// Each input, changed alone, misses. `FIELDS` names the fields of `Key` and this table has
    /// one row per name, so a new field needs a row (and the digest needs to hash it to compile).
    #[test]
    fn every_input_of_the_key_misses_when_it_changes() {
        type Change = fn(&mut Key);
        let changes: Vec<(&str, Change)> = vec![
            ("format", |k| k.format += 1),
            ("runtime_build", |k| k.runtime_build[31] ^= 1),
            ("target", |k| k.target = "BC 28.0 runtime 17.0".into()),
            ("symbols", |k| k.symbols.push("EXTRA".into())),
            ("apps", |k| k.apps.reverse()),
        ];
        let names: Vec<&str> = changes.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, Key::FIELDS, "one miss test per field of the key");
        let base = sample();
        for (name, change) in changes {
            let mut k = base.clone();
            change(&mut k);
            assert_ne!(k.digest(), base.digest(), "changing `{name}` must change the digest");
        }
    }

    /// Each input of one app, changed alone, misses.
    #[test]
    fn every_input_of_an_app_misses_when_it_changes() {
        type Change = fn(&mut AppKey);
        let changes: Vec<(&str, Change)> = vec![
            ("id", |a| a.id = "z".into()),
            ("name", |a| a.name = "Other".into()),
            ("dependencies", |a| a.dependencies.push("x".into())),
            ("publisher", |a| a.publisher = "Other".into()),
            ("version", |a| a.version = "27.5.0.1".into()),
            ("source", |a| a.source[0] ^= 1),
        ];
        let base = sample();
        for (name, change) in changes {
            let mut k = base.clone();
            change(&mut k.apps[0]);
            assert_ne!(
                k.digest(),
                base.digest(),
                "changing `{name}` of an app must change the digest"
            );
        }
        // A fresh key with the same inputs hits.
        assert_eq!(sample().digest(), base.digest());
    }

    #[test]
    fn fields_do_not_run_into_each_other() {
        let mut a = sample();
        let mut b = sample();
        a.apps[0].id = "ab".into();
        a.apps[0].name = "c".into();
        b.apps[0].id = "a".into();
        b.apps[0].name = "bc".into();
        assert_ne!(a.digest(), b.digest());
    }
}
