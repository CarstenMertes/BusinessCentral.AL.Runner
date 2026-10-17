//! Per-phase cost of opening a backup and reading tables through the provider.
//! `cargo run --release -p al-bcdb-std --example tdbench -- FILE.bak [Table ...]`
use al_bcdb_std::BackupProvider;
use al_host::testdata::{TableProvider, TableRequest};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("backup path");
    let tables: Vec<String> = args.collect();
    let mut best_open = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let p = BackupProvider::open(std::path::Path::new(&path), None).unwrap();
        best_open = best_open.min(t.elapsed().as_secs_f64() * 1e3);
        drop(p);
    }
    println!("open (min of 5): {best_open:.1} ms");
    let p = BackupProvider::open(std::path::Path::new(&path), None).unwrap();
    for name in &tables {
        let req = TableRequest {
            name: name.clone(),
            app_id: None,
            per_company: true,
            ..Default::default()
        };
        let mut best = f64::MAX;
        let mut rows = 0;
        for _ in 0..7 {
            let t = Instant::now();
            let r = p.read(&req);
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            rows = r.map(|r| r.rows.len()).unwrap_or(0);
        }
        println!("{best:8.2} ms {rows:8} rows  {name}");
    }
}
