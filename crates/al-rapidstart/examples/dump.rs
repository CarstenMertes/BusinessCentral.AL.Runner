//! `cargo run --release -p al-rapidstart --example dump -- PACKAGE.rapidstart`: the tables of a package.
fn main() {
    let path = std::env::args().nth(1).expect("a .rapidstart path");
    let bytes = std::fs::read(&path).expect("readable file");
    let t = std::time::Instant::now();
    let p = al_rapidstart::Package::parse(&bytes).expect("a package");
    let rows: usize = p.tables.iter().map(|t| t.rows.len()).sum();
    println!(
        "{} {}: {} tables, {} rows, {} with rows and triggers on, parsed in {:?}",
        p.code,
        p.product_version,
        p.tables.len(),
        rows,
        p.triggered_tables().len(),
        t.elapsed()
    );
    for id in p.triggered_tables() {
        println!("triggers run in BC for table {id}");
    }
}
