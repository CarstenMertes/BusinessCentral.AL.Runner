//! Start-up time and per-call latency of the spike sidecar.
//! Usage: cargo run --release --bin bench [-- <calls>]
use dotnet_sidecar_spike::*;
use serde_json::json;
use std::time::Instant;

fn pct(sorted: &[u128], p: f64) -> u128 {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn report(name: &str, mut ns: Vec<u128>) {
    ns.sort();
    println!(
        "{name:<44} n={:<6} median {:>7.1} us   p99 {:>7.1} us   max {:>8.1} us",
        ns.len(),
        pct(&ns, 0.5) as f64 / 1e3,
        pct(&ns, 0.99) as f64 / 1e3,
        *ns.last().unwrap() as f64 / 1e3
    );
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let dll = Sidecar::locate().expect("build the sidecar first");

    // Start-up: spawn until the first ping answers, then until the first reflection call answers.
    let mut starts = Vec::new();
    let mut firsts = Vec::new();
    for _ in 0..10 {
        let t0 = Instant::now();
        let mut sc = Sidecar::start(&dll).unwrap();
        sc.request(json!({"op":"ping"})).unwrap();
        starts.push(t0.elapsed().as_nanos());
        let t1 = Instant::now();
        sc.call_static("System.Convert", "ToBase64String", vec![bytes(b"x")])
            .unwrap();
        firsts.push(t1.elapsed().as_nanos());
    }
    report("start-up: spawn to first ping", starts);
    report("first reflection call after ping", firsts);

    if let Some(dir) = bc_service_dir() {
        let dir = dir.to_string_lossy().to_string();
        let mut sc = Sidecar::start(&dll).unwrap();
        sc.request(json!({"op":"ping"})).unwrap();
        let t0 = Instant::now();
        sc.request(json!({"op":"probe","dirs":[dir]})).unwrap();
        for d in ["Types", "Ncl", "NavUserAccount"] {
            let t = Instant::now();
            sc.request(json!({"op":"load","path":format!("{dir}/Microsoft.Dynamics.Nav.{d}.dll")}))
                .unwrap();
            println!(
                "load Microsoft.Dynamics.Nav.{d}.dll: {:.1} ms",
                t.elapsed().as_secs_f64() * 1e3
            );
        }
        let t = Instant::now();
        sc.call_static(
            "Microsoft.Dynamics.Nav.Types.Base64UrlHelper",
            "EncodeBase64UrlString",
            vec![bytes(b"x")],
        )
        .unwrap();
        println!(
            "first Base64UrlHelper call: {:.1} ms; all loads {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3,
            t0.elapsed().as_secs_f64() * 1e3
        );
    }

    let mut sc = Sidecar::start(&dll).unwrap();
    sc.request(json!({"op":"ping"})).unwrap();

    let mut t = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        sc.request(json!({"op":"ping"})).unwrap();
        t.push(t0.elapsed().as_nanos());
    }
    report("ping (protocol floor, no reflection)", t);

    let mut t = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let r = sc
            .call_static("System.Convert", "ToBase64String", vec![bytes(b"hello, AL")])
            .unwrap();
        t.push(t0.elapsed().as_nanos());
        assert_eq!(as_str(&r), "aGVsbG8sIEFM");
    }
    report("Convert.ToBase64String(9 bytes)", t);

    let mut t = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        sc.call_static("System.Convert", "FromBase64String", vec![s("aGVsbG8sIEFM")])
            .unwrap();
        t.push(t0.elapsed().as_nanos());
    }
    report("Convert.FromBase64String(12 chars)", t);

    let ms = sc.new_object("System.IO.MemoryStream", vec![]).unwrap();
    let mut t = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        sc.call(ms, "WriteByte", vec![json!({"t":"i4","v":65})])
            .unwrap_or_else(|e| panic!("{e:?}"));
        t.push(t0.elapsed().as_nanos());
    }
    report("MemoryStream.WriteByte (handle call)", t);

    // Large payload: 1 MiB through the pipe, to see the cost of the JSON/base64 framing.
    let big = vec![7u8; 1 << 20];
    let mut t = Vec::new();
    for _ in 0..50 {
        let t0 = Instant::now();
        let r = sc
            .call_static("System.Convert", "ToBase64String", vec![bytes(&big)])
            .unwrap();
        t.push(t0.elapsed().as_nanos());
        assert_eq!(as_str(&r).len(), 1_398_104);
    }
    report("Convert.ToBase64String(1 MiB), 50 calls", t);
}
