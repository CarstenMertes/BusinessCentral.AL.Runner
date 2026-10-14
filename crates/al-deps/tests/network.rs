//! Talks to Microsoft's artifact CDN. `#[ignore]`d: `cargo test` never uses the network.
//!
//! `cargo test --release -p al-deps --test network -- --ignored --nocapture`
//!
//! CI runs it in the release job, after the cache is restored. It reads about 2 MB of a
//! 1.3 GB artifact: the end of the file (the central directory) and the three test-library
//! archives, and checks their CRC-32.

use al_deps::artifact::{Cdn, HttpCdn, newest_published};
use al_deps::build::BuildQuery;
use al_deps::package::read_package;
use al_deps::remote::RemoteZip;
use std::time::Instant;

#[test]
#[ignore = "needs the network: cargo test --release -p al-deps --test network -- --ignored"]
fn the_cdn_serves_the_test_libraries_by_range() {
    let cdn = HttpCdn::from_env();
    let t = Instant::now();
    let chosen = newest_published(&cdn, &BuildQuery::new(Some("27.5"), 27, None)).unwrap();
    println!("newest 27.5 build: {} (index in {:?})", chosen.build, t.elapsed());
    assert!(chosen.build.matches_prefix("27.5"));

    let t = Instant::now();
    let (url, src) = cdn.platform(&chosen.build);
    let zip = RemoteZip::open(src).unwrap();
    let (total, tail) = zip.tail();
    println!(
        "{url}: {} MB artifact, {} entries, directory read ({} KB) in {:?}",
        total >> 20,
        zip.entries.len(),
        tail.len() >> 10,
        t.elapsed()
    );
    assert!(total > 500 << 20, "the platform artifact is about a gigabyte");

    for (entry, app) in [
        (
            "Applications/System Application/Test/System Application Test Library.Source.zip",
            "System Application Test Library",
        ),
        (
            "Applications/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip",
            "Permissions Mock",
        ),
        ("Applications/TestFramework/TestLibraries/Any/Any.Source.zip", "Any"),
    ] {
        let t = Instant::now();
        let e = zip
            .find(entry)
            .unwrap_or_else(|| panic!("{entry} is not in the artifact"));
        let bytes = zip.read(e).unwrap(); // CRC-32 and size are checked inside
        let (m, sources) = read_package(&bytes).unwrap();
        println!(
            "{app}: {} KB, {} .al files, {:?}",
            bytes.len() >> 10,
            sources.len(),
            t.elapsed()
        );
        assert_eq!(m.name, app);
        assert_eq!(m.publisher, "Microsoft");
        assert!(!sources.is_empty());
    }
}

#[test]
#[ignore = "needs the network: cargo test --release -p al-deps --test network -- --ignored"]
fn the_country_artifact_lists_the_demo_backup_in_its_central_directory() {
    let cdn = HttpCdn::from_env();
    let chosen = newest_published(&cdn, &BuildQuery::new(Some("27.5"), 27, None)).unwrap();
    let (url, src) = cdn.country(&chosen.build, "w1");
    let zip = RemoteZip::open(src).unwrap();
    let name = al_deps::demo::backup_name("w1");
    let e = zip
        .entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{url} holds no {name}"));
    println!(
        "{url}: {name}, {} MB stored, {} MB inflated",
        e.compressed_size() >> 20,
        e.size >> 20
    );
    assert!(e.size > 300 << 20, "the demo backup is hundreds of megabytes");
}
