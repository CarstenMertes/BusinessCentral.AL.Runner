//! Helpers the CLI test binaries share.
#![allow(dead_code)]

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A ZIP with stored entries.
pub fn zip(entries: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, text) in entries {
        let data = text.as_bytes();
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend([0, 0]);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    let n = entries.len() as u16;
    let cd_len = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend(n.to_le_bytes());
    out.extend(n.to_le_bytes());
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend([0, 0]);
    out
}

/// A folder of `tests/fixtures/impact`.
pub fn impact_fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/impact")
        .join(name)
}

/// The impact fixture's extension without its own test codeunit, under `root/ext-only`.
pub fn ext_only(root: &std::path::Path) -> std::path::PathBuf {
    let ext = impact_fixture("ext");
    let only = root.join("ext-only");
    std::fs::create_dir_all(&only).unwrap();
    std::fs::copy(ext.join("app.json"), only.join("app.json")).unwrap();
    let al = std::fs::read_to_string(ext.join("Subscriber.Codeunit.al")).unwrap();
    let cut = al
        .find("// The extension's own tests")
        .expect("the fixture's own tests");
    std::fs::write(only.join("Subscriber.Codeunit.al"), &al[..cut]).unwrap();
    only
}

/// stdout and stderr of a run.
pub fn text(o: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The cells of a row of the `Sources` table, after the part's name.
pub fn source_row(out: &str, name: &str) -> String {
    let at = out
        .find("\nSources\n")
        .unwrap_or_else(|| panic!("no Sources table in:\n{out}"));
    let line = out[at..]
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no row {name} in:\n{out}"));
    line.trim_start()[name.len()..]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
