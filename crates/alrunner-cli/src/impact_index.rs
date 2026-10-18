//! The impact index (ADR-043 step 2): one run of the test apps without any extension that keeps, per test, its
//! outcome and what it raised and touched (`al_testrun::impact_trace`). An impact run with an index runs only the test
//! codeunits that reach an impact point of the extension, once, and takes the outcomes without it from here.
//!
//! The index is internal tooling (ADR-009): a gzip-compressed JSON document with a format number. It is kept under
//! `<cache dir>/impact/<key>/index.json.gz`, where the key hashes everything the outcomes depend on that is known
//! before the run: the runtime build (executable hash), the test apps' source, the country, the build asked for,
//! the company data and test data options, the isolation, the defines and the disabled-tests files. The Microsoft
//! apps the run compiled are stored in the index and checked against the impact run's compile (a different build
//! of the Base App makes the index stale).

use crate::apps;
use crate::args::TestArgs;
use crate::run::{self, Collected};
use al_testrun::Outcome;
use al_testrun::runtime::TestResult;
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The format of the index file. A file of another format is not read (it is rebuilt).
pub const FORMAT: u64 = 1;

/// One test of the index run.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedTest {
    /// The test app (index among the APPs given).
    pub app: usize,
    pub codeunit: u32,
    pub codeunit_name: String,
    pub test: String,
    pub outcome: Outcome,
    /// The ids of the unprobed assumptions it ran through (a pass with any is a flagged pass).
    pub assumptions: Vec<String>,
    /// What it raised and touched: numbers of `Index::keys`, sorted. The first test of a codeunit also holds what
    /// ran before it.
    pub keys: Vec<u32>,
}

/// What `alrunner impact-index build` keeps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Index {
    /// The apps of the index run's compile: (id, name, version).
    pub apps: Vec<(String, String, String)>,
    /// The keys the tests' numbers stand for.
    pub keys: Vec<String>,
    /// What the install baseline raised and touched; `None` when no baseline ran.
    pub baseline: Option<Vec<u32>>,
    pub tests: Vec<IndexedTest>,
}

impl Index {
    /// The keys of a test, as text.
    pub fn keys_of(&self, t: &IndexedTest) -> Vec<String> {
        t.keys.iter().map(|&k| self.keys[k as usize].clone()).collect()
    }

    pub fn baseline_keys(&self) -> Option<Vec<String>> {
        self.baseline
            .as_ref()
            .map(|b| b.iter().map(|&k| self.keys[k as usize].clone()).collect())
    }

    /// The test as a result of the "without" side of an impact run.
    pub fn result(&self, t: &IndexedTest) -> TestResult {
        TestResult {
            suite: 0,
            app: t.app,
            codeunit: t.codeunit,
            codeunit_name: t.codeunit_name.clone(),
            test: t.test.clone(),
            outcome: t.outcome.clone(),
            call_stack: String::new(),
            assumptions: t
                .assumptions
                .iter()
                .filter_map(|id| al_builtins::assumptions::by_id(id))
                .collect(),
            time: std::time::Duration::ZERO,
            coverage: Vec::new(),
            messages: Vec::new(),
            captured: None,
            impact: t.keys.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        // Each test's keys as differences from the one before: small numbers that compress well.
        let delta = |keys: &[u32]| -> Vec<u32> {
            let mut last = 0;
            keys.iter()
                .map(|&k| {
                    let d = k - last;
                    last = k;
                    d
                })
                .collect()
        };
        json!({
            "format": FORMAT,
            "apps": self.apps.iter().map(|(id, name, version)| json!([id, name, version])).collect::<Vec<_>>(),
            "keys": self.keys,
            "baseline": self.baseline.as_deref().map(delta),
            "tests": self.tests.iter().map(|t| {
                let (kind, message) = match &t.outcome {
                    Outcome::Pass => ("pass", ""),
                    Outcome::Fail(m) => ("fail", m.as_str()),
                    Outcome::NotRun(m) => ("notrun", m.as_str()),
                };
                json!({
                    "app": t.app,
                    "codeunit": t.codeunit,
                    "codeunitName": t.codeunit_name,
                    "test": t.test,
                    "outcome": kind,
                    "message": message,
                    "assumptions": t.assumptions,
                    "keys": delta(&t.keys),
                })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Result<Index, String> {
        if v["format"].as_u64() != Some(FORMAT) {
            return Err(format!(
                "the index has format {}, this runtime reads format {FORMAT}",
                v["format"]
            ));
        }
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let undelta = |v: &Value| -> Vec<u32> {
            let mut last = 0u32;
            v.as_array()
                .into_iter()
                .flatten()
                .map(|d| {
                    last += d.as_u64().unwrap_or(0) as u32;
                    last
                })
                .collect()
        };
        let apps = v["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| (text(&a[0]), text(&a[1]), text(&a[2])))
            .collect();
        let keys: Vec<String> = v["keys"].as_array().into_iter().flatten().map(text).collect();
        let baseline = (!v["baseline"].is_null()).then(|| undelta(&v["baseline"]));
        let mut tests = Vec::new();
        for t in v["tests"].as_array().into_iter().flatten() {
            let message = text(&t["message"]);
            let outcome = match t["outcome"].as_str() {
                Some("pass") => Outcome::Pass,
                Some("fail") => Outcome::Fail(message),
                Some("notrun") => Outcome::NotRun(message),
                other => {
                    return Err(format!(
                        "the index has a test with outcome `{}`",
                        other.unwrap_or_default()
                    ));
                }
            };
            let keys_of = undelta(&t["keys"]);
            if keys_of.iter().any(|&k| k as usize >= keys.len()) {
                return Err("the index has a test with a key it does not list".into());
            }
            tests.push(IndexedTest {
                app: t["app"].as_u64().unwrap_or(0) as usize,
                codeunit: t["codeunit"].as_u64().unwrap_or(0) as u32,
                codeunit_name: text(&t["codeunitName"]),
                test: text(&t["test"]),
                outcome,
                assumptions: t["assumptions"].as_array().into_iter().flatten().map(text).collect(),
                keys: keys_of,
            });
        }
        Ok(Index {
            apps,
            keys,
            baseline,
            tests,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(self.to_json().to_string().as_bytes())
            .expect("writing to memory");
        gz.finish().expect("writing to memory")
    }

    pub fn decode(bytes: &[u8]) -> Result<Index, String> {
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut text)
            .map_err(|e| format!("the index is not readable: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("the index is not JSON: {e}"))?;
        Index::from_json(&v)
    }
}

/// Hashes the files of a folder (or one file), in path order.
fn hash_tree(path: &Path, h: &mut impl Hasher) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        let mut kids: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        kids.sort();
        for k in kids {
            k.file_name().hash(h);
            hash_tree(&k, h)?;
        }
    } else {
        std::fs::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .hash(h);
    }
    Ok(())
}

/// The cache key of the index of the run `a` (its APPs are the test apps): see the module text.
pub fn key(a: &TestArgs) -> Result<String, String> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    FORMAT.hash(&mut h);
    al_testrun::compile_cache::runtime_build().hash(&mut h);
    for p in &a.apps {
        let input = apps::load(p, a.bc_version.as_deref())?;
        input.source.id.hash(&mut h);
        input.manifest.version.hash(&mut h);
        let mut files: Vec<&(String, String)> = input.source.files.iter().collect();
        files.sort();
        files.hash(&mut h);
    }
    a.country().hash(&mut h);
    a.bc_version.hash(&mut h);
    (
        a.wants_test_data(),
        a.no_test_data,
        &a.test_data_path,
        &a.test_data_company,
        a.company_package,
        &a.package_path,
        &a.company_name,
    )
        .hash(&mut h);
    a.isolation.hash(&mut h);
    a.defines.hash(&mut h);
    a.environment.as_ref().map(std::mem::discriminant).hash(&mut h);
    (&a.server_url, a.test_timeout_secs).hash(&mut h);
    if let Some(dir) = &a.disabled_tests_dir {
        hash_tree(dir, &mut h).map_err(|e| format!("--disabled-tests {e}"))?;
    }
    Ok(format!("{:016x}", h.finish()))
}

/// Where the index of the run `a` is kept.
pub fn path(a: &TestArgs) -> Result<PathBuf, String> {
    let root = al_deps::cache::cache_root_from_env(a.cache_dir.as_deref());
    Ok(root.join("impact").join(key(a)?).join("index.json.gz"))
}

/// The cached index of the run `a`, if there is one this runtime reads. A file that cannot be read is reported and
/// treated as missing.
pub fn lookup(a: &TestArgs) -> Result<Option<(Index, PathBuf)>, String> {
    let path = path(a)?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    match Index::decode(&bytes) {
        Ok(i) => Ok(Some((i, path))),
        Err(e) => {
            eprintln!("warning: impact index {} is not used: {e}; rebuild it", path.display());
            Ok(None)
        }
    }
}

/// Writes by a temporary file and a rename, as the compile cache does.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().expect("the index has a folder");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".index.tmp-{}", std::process::id()));
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|e| format!("{}: {e}", path.display()))
}

/// The index of a run's results.
pub fn of_run(c: &Collected) -> Index {
    let tests = c
        .results
        .iter()
        .map(|r| IndexedTest {
            app: r.app,
            codeunit: r.codeunit,
            codeunit_name: r.codeunit_name.clone(),
            test: r.test.clone(),
            outcome: r.outcome.clone(),
            assumptions: r.assumptions.iter().map(|a| a.info().id.to_string()).collect(),
            keys: r.impact.clone(),
        })
        .collect();
    Index {
        apps: c.program_apps.clone(),
        keys: c.impact_keys.clone(),
        baseline: c.impact_baseline.clone(),
        tests,
    }
}

/// `alrunner impact-index build APP...`: runs the tests of the APPs with the trace on and writes the index.
pub fn build(a: &TestArgs) -> Result<u8, String> {
    // The index must hold every test codeunit an impact run of these APPs can run.
    if !a.codeunits.is_empty() || !a.filters.is_empty() || !a.exact.is_empty() || !a.excludes.is_empty() {
        return Err("alrunner impact-index build runs every test of the APPs: --codeunit, --test, --exact and --exclude do not apply (an impact run selects from the whole index)".into());
    }
    let started = Instant::now();
    let path = path(a)?;
    let mut side = a.clone();
    side.quiet = !a.verbose;
    // Microsoft's disabled tests are left out of impact runs, and so of their index (ADR-043 owner decision 2).
    side.parity = a.disabled_tests_dir.is_some();
    side.test_apps = Some(a.apps.len());
    println!("building the impact index of {} test app(s)", a.apps.len());
    let mut c = Collected {
        impact_trace: true,
        ..Default::default()
    };
    let code = run::test_collect(&side, &mut c)?;
    if !matches!(code, 0 | 1 | 7 | 8) {
        return Ok(code);
    }
    let index = of_run(&c);
    let bytes = index.encode();
    write(&path, &bytes)?;
    let codeunits: std::collections::BTreeSet<(usize, u32)> = index.tests.iter().map(|t| (t.app, t.codeunit)).collect();
    let keys: usize = index.tests.iter().map(|t| t.keys.len()).sum();
    println!(
        "impact index: {} tests in {} codeunits, {} distinct keys ({:.0} per test on average), {} bytes, built in {:.1} s: {}",
        index.tests.len(),
        codeunits.len(),
        index.keys.len(),
        keys as f64 / index.tests.len().max(1) as f64,
        bytes.len(),
        started.elapsed().as_secs_f64(),
        path.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_reads_back_what_was_written() {
        let index = Index {
            apps: vec![("id".into(), "Base".into(), "27.5.0.0".into())],
            keys: vec![
                "table 18".into(),
                "event codeunit 80 onbeforepost".into(),
                "write 18".into(),
            ],
            baseline: Some(vec![0, 2]),
            tests: vec![
                IndexedTest {
                    app: 0,
                    codeunit: 134000,
                    codeunit_name: "ERM Tests".into(),
                    test: "Posts".into(),
                    outcome: Outcome::Pass,
                    assumptions: vec!["some-id".into()],
                    keys: vec![0, 1, 2],
                },
                IndexedTest {
                    app: 1,
                    codeunit: 134001,
                    codeunit_name: "ERM Other".into(),
                    test: "Fails".into(),
                    outcome: Outcome::Fail("Assert.AreEqual failed".into()),
                    assumptions: vec![],
                    keys: vec![],
                },
            ],
        };
        assert_eq!(Index::decode(&index.encode()).unwrap(), index);
        // Another format is not read.
        let mut v = index.to_json();
        v["format"] = json!(FORMAT + 1);
        assert!(Index::from_json(&v).is_err());
    }
}
