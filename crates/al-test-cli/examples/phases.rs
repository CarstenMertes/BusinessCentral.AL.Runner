use al_test_cli::runtime;
use std::time::Instant;
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or(".".into()));
    let t = Instant::now();
    let tests = root.join("corpus/tests");
    let apps = vec![
        runtime::load_app(&tests.join("al-language-internals-fixture")).unwrap(),
        runtime::load_app(&tests.join("al-language")).unwrap(),
    ];
    println!("read files    {:?}", t.elapsed());
    let syms = ["BC27PLUS".to_string()].into();
    let bytes: usize = apps.iter().flat_map(|a| a.files.iter()).map(|f| f.1.len()).sum();
    println!("bytes {bytes}");
    let t = Instant::now();
    for a in &apps {
        for f in &a.files {
            std::hint::black_box(al_syntax::lex(&f.1));
        }
    }
    println!("lex only      {:?}", t.elapsed());
    let t = Instant::now();
    for a in &apps {
        for f in &a.files {
            std::hint::black_box(al_syntax::parse(&f.1, &syms));
        }
    }
    println!("parse (all)   {:?}", t.elapsed());
    let t = Instant::now();
    let ws = al_sema::workspace::Workspace::load(apps, &syms);
    println!("ws load       {:?}", t.elapsed());
    let t = Instant::now();
    let members = al_sema::members::build(&ws);
    println!("members       {:?}", t.elapsed());
    let t = Instant::now();
    let cat = al_builtins::catalog::Catalog::load();
    println!("catalog load  {:?}", t.elapsed());
    let t = Instant::now();
    let checked = al_sema::check::check(&ws, &members, &cat);
    println!("check         {:?}", t.elapsed());
    let t = Instant::now();
    let lowered = al_sema::lower::lower(&ws, &members, &checked);
    println!("lower         {:?}", t.elapsed());
    let t = Instant::now();
    let blocked = lowered
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = al_vm::Engine::new(lowered.program.clone(), blocked);
    println!(
        "engine new    {:?} ({} objs, {} diags)",
        t.elapsed(),
        lowered.program.objects.len(),
        checked.diagnostics.len()
    );
    drop(engine);
}
