//! ADR-006: the core does no OS access. A lint over the crate's own source for the paths the
//! ADR forbids (no files, network, processes, environment, wall clock, threads).

#[test]
fn the_core_source_names_no_os_api() {
    // assembled so this file does not contain them itself
    let forbidden = [
        ["std::", "fs"].concat(),
        ["std::", "net"].concat(),
        ["std::", "process"].concat(),
        ["std::", "env"].concat(),
        ["std::", "thread"].concat(),
        ["System", "Time"].concat(),
        ["Instant", "::now"].concat(),
        ["std::", "os::"].concat(),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for f in &forbidden {
                assert!(!text.contains(f.as_str()), "{} uses {f}", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked > 15, "only {checked} source files were looked at");
}
