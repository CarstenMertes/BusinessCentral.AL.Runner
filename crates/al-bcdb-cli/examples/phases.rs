//! Prints how long each phase of reading one table from a `.bak` takes:
//! `cargo run --profile bcdb-bench -p al-bcdb-cli --example phases -- <file.bak> [table] [company]`
use al_bcdb::bak::BakSource;
use al_bcdb::catalog::Catalog;
use al_bcdb::io::ByteSource;
use al_bcdb::pagefile::PageFile;
use al_bcdb::query::{ReadRequest, TableSelector, plan_read};
use std::time::Instant;

struct P(std::fs::File);
impl ByteSource for P {
    fn len(&self) -> u64 {
        self.0.metadata().unwrap().len()
    }
    fn read_at(&self, o: u64, b: &mut [u8]) -> al_bcdb::Result<()> {
        use std::os::unix::fs::FileExt;
        self.0
            .read_exact_at(b, o)
            .map_err(|e| al_bcdb::Error::Io(e.to_string()))
    }
}

fn map(path: &str) -> Box<dyn ByteSource> {
    let f = std::fs::File::open(path).unwrap();
    Box::new(P(f))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let table = args.get(2).cloned().unwrap_or_else(|| "No. Series".into());
    let company = args.get(3).cloned().unwrap_or_else(|| "CRONUS".into());
    let t = Instant::now();
    let pf = PageFile::open(map(path)).unwrap();
    let t_map = t.elapsed();
    for r in &pf.mtf().mqda_regions {
        println!("region blocks {} ({} MB)", r.block_count, r.block_count * 8 / 1024);
    }
    let t = Instant::now();
    let cat = Catalog::open(&pf).unwrap();
    let t_cat = t.elapsed();
    println!(
        "page map     {:8.2} ms  ({} pages)",
        t_map.as_secs_f64() * 1e3,
        pf.page_count()
    );
    println!(
        "catalog      {:8.2} ms  ({} objects)",
        t_cat.as_secs_f64() * 1e3,
        cat.total_object_count
    );
    drop((pf, cat));
    let t = Instant::now();
    let bak = BakSource::open(map(path)).unwrap();
    println!(
        "BakSource    {:8.2} ms  (map + catalog + table list)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let req = ReadRequest {
        sel: TableSelector {
            table: Some(table),
            company: Some(company),
            app: None,
        },
        ..Default::default()
    };
    let t = Instant::now();
    let plan = plan_read(&bak, None, &req).unwrap();
    println!(
        "plan         {:8.2} ms  (resolve table, columns)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let mut n = 0u64;
    plan.run(&bak, &mut |_| {
        n += 1;
        Ok(true)
    })
    .unwrap();
    println!("rows         {:8.2} ms  ({n} rows)", t.elapsed().as_secs_f64() * 1e3);
}
