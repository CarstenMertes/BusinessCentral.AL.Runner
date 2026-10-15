//! Dumps everything the front end produces for the pinned corpus: workspace
//! files, objects and diagnostics, the checker's diagnostics, types and
//! resolutions, and the lowered program. For a performance change, dump
//! before and after and `cmp` the two files: they must be identical.
//!
//! `cargo run --release -p al-test-cli --example compile_dump -- ROOT OUT.txt [deps]`
//!
//! With `deps`, the Microsoft apps of the corpus's `.alpackages` are compiled too, the way a
//! run does (demand-driven, with the install baseline as a root). The output is then one line
//! per object with a hash of its lowered form (the full program would be gigabytes), plus the
//! hashes of the call graph, the refusals and the program's other parts.

use al_test_cli::runtime;
use std::io::Write;
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let out = std::env::args().nth(2).unwrap();
    let tests = root.join("corpus/tests");
    let apps = vec![
        runtime::load_app(&tests.join("al-language-internals-fixture")).unwrap(),
        runtime::load_app(&tests.join("al-language")).unwrap(),
    ];
    let syms = al_version::corpus_symbols(27);
    if std::env::args().nth(3).is_some_and(|a| a == "deps") {
        return dump_with_deps(&tests, apps, &syms, &out);
    }
    let ws = al_sema::workspace::Workspace::load(apps, &syms);
    let members = al_sema::members::build(&ws);
    let checked = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let lowered = al_sema::lower::lower(&ws, &members, &checked);
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).unwrap());
    writeln!(f, "files {}", ws.files.len()).unwrap();
    for x in &ws.files {
        writeln!(
            f,
            "file {} app{} ns={:?} usings={:?} len={:?}",
            x.path,
            x.app,
            x.namespace,
            x.usings,
            ws.green(ws.files.iter().position(|g| g.path == x.path).unwrap())
                .text_len()
        )
        .unwrap();
    }
    for o in &ws.objects {
        writeln!(f, "obj {o:?}").unwrap();
    }
    for d in &ws.diagnostics {
        writeln!(f, "wsdiag {d:?}").unwrap();
    }
    for d in &checked.diagnostics {
        writeln!(f, "chkdiag {d:?}").unwrap();
    }
    let mut t: Vec<_> = checked.types.iter().collect();
    t.sort_by_key(|(k, _)| **k);
    for (k, v) in t {
        writeln!(f, "ty {k:?} {v:?}").unwrap();
    }
    let mut r: Vec<_> = checked.resolutions.iter().collect();
    r.sort_by_key(|(k, _)| **k);
    for (k, v) in r {
        writeln!(f, "res {k:?} {v:?}").unwrap();
    }
    writeln!(f, "unsupported {:?}", lowered.unsupported).unwrap();
    writeln!(f, "calls {:?}", lowered.calls).unwrap();
    writeln!(
        f,
        "program {}",
        legacy_json(serde_json::to_string(&*lowered.program).unwrap())
    )
    .unwrap();
}

/// The JSON as it was before the object kind became typed (ADR-032 step 1): `"kind":"uncompiled table"`
/// and no `uncompiled` field. A dump from before and after the change is comparable byte for byte.
fn legacy_json(mut json: String) -> String {
    for k in al_ir::ObjKind::ALL {
        json = json.replace(
            &format!("{{\"kind\":\"{k}\",\"uncompiled\":true,"),
            &format!("{{\"kind\":\"uncompiled {k}\","),
        );
    }
    json.replace("\"uncompiled\":false,", "")
}

fn hash_of<T: serde::Serialize + ?Sized>(v: &T) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    legacy_json(serde_json::to_string(v).unwrap()).hash(&mut h);
    h.finish()
}

/// The pipeline of a run with dependencies, summarised by hashes.
fn dump_with_deps(
    tests: &std::path::Path,
    mut apps: Vec<al_sema::workspace::AppSource>,
    syms: &std::collections::BTreeSet<String>,
    out: &str,
) {
    let mut kinds = vec![al_sema::AppKind::Workspace; apps.len()];
    let deps = al_test_cli::deps::load(
        &tests.join("al-language"),
        &[tests.join("al-language-internals-fixture")],
        &[],
    )
    .unwrap();
    kinds.extend(deps.apps.iter().map(|_| al_sema::AppKind::Dependency));
    apps.extend(deps.apps);
    let roots = al_sema::check::Roots {
        baseline: true,
        ..Default::default()
    };
    let (ws, lowered) = al_sema::lower::compile_apps_with(apps, kinds, syms, None, roots);
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).unwrap());
    for d in &ws.diagnostics {
        writeln!(f, "wsdiag {d:?}").unwrap();
    }
    for (i, o) in lowered.program.objects.iter().enumerate() {
        writeln!(f, "object {i} {} {:016x}", o.name, hash_of(o)).unwrap();
    }
    writeln!(f, "unsupported {:016x}", hash_of(&lowered.unsupported)).unwrap();
    writeln!(f, "calls {:016x}", hash_of(&lowered.calls)).unwrap();
    writeln!(f, "dependency {:016x}", hash_of(&lowered.dependency)).unwrap();
    writeln!(f, "positions {:016x}", hash_of(&lowered.program.positions)).unwrap();
    writeln!(f, "sources {:016x}", hash_of(&lowered.program.sources)).unwrap();
    writeln!(f, "apps {:016x}", hash_of(&lowered.program.apps)).unwrap();
    writeln!(
        f,
        "uncompiled_subscribers {:016x}",
        hash_of(&lowered.program.uncompiled_subscribers)
    )
    .unwrap();
    writeln!(f, "dotnet_types {:016x}", hash_of(&lowered.program.dotnet_types)).unwrap();
}
