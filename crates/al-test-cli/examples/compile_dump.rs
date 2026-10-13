//! Dumps everything the front end produces for the pinned corpus: workspace
//! files, objects and diagnostics, the checker's diagnostics, types and
//! resolutions, and the lowered program. For a performance change, dump
//! before and after and `cmp` the two files: they must be identical.
//!
//! `cargo run --release -p al-test-cli --example compile_dump -- ROOT OUT.txt`

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
    let syms = ["BC27PLUS".to_string()].into();
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
    writeln!(f, "program {}", serde_json::to_string(&lowered.program).unwrap()).unwrap();
}
