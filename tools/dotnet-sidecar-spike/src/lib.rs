//! Spike client for the .NET sidecar (ADR-014). One JSON object per line over
//! the child's stdin and stdout. Throwaway: the real seam lives behind a trait
//! in `al-host`.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A tagged value, as the sidecar protocol writes it.
pub fn s(v: &str) -> Value {
    json!({"t":"s","v":v})
}
pub fn i4(v: i32) -> Value {
    json!({"t":"i4","v":v})
}
pub fn bytes(v: &[u8]) -> Value {
    json!({"t":"bytes","v":base64(v)})
}
pub fn enum_(ty: &str, name: &str) -> Value {
    json!({"t":"enum","type":ty,"v":name})
}
pub fn handle(h: i64) -> Value {
    json!({"t":"h","v":h})
}

pub struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

#[derive(Debug)]
pub struct DotNetError {
    pub ty: String,
    pub message: String,
}

impl Sidecar {
    /// Locate the built sidecar: `ALRT_DOTNET_SIDECAR`, else the spike's own build output.
    pub fn locate() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("ALRT_DOTNET_SIDECAR") {
            return Some(PathBuf::from(p));
        }
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("sidecar/bin/AlDotNetSidecarSpike.dll");
        p.exists().then_some(p)
    }

    pub fn start(dll: &Path) -> std::io::Result<Self> {
        let mut child = Command::new("dotnet")
            .arg(dll)
            .env("DOTNET_TieredPGO", "0")
            .env("DOTNET_TC_QuickJitForLoops", "1")
            .env("DOTNET_NOLOGO", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        })
    }

    /// Send one request, return the `value` or the .NET error.
    pub fn request(&mut self, mut req: Value) -> Result<Value, DotNetError> {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = json!(id);
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).expect("sidecar wrote invalid JSON");
        assert_eq!(resp["id"], json!(id), "response out of order");
        if resp["ok"] == json!(true) {
            Ok(resp["value"].clone())
        } else {
            Err(DotNetError {
                ty: resp["error"]["type"].as_str().unwrap_or("").to_string(),
                message: resp["error"]["message"].as_str().unwrap_or("").to_string(),
            })
        }
    }

    pub fn call_static(&mut self, ty: &str, method: &str, args: Vec<Value>) -> Result<Value, DotNetError> {
        self.request(json!({"op":"call","type":ty,"method":method,"args":args}))
    }
    pub fn new_object(&mut self, ty: &str, args: Vec<Value>) -> Result<i64, DotNetError> {
        let v = self.request(json!({"op":"new","type":ty,"args":args}))?;
        Ok(v["v"].as_i64().expect("expected a handle"))
    }
    pub fn call(&mut self, target: i64, method: &str, args: Vec<Value>) -> Result<Value, DotNetError> {
        self.request(json!({"op":"call","target":target,"method":method,"args":args}))
    }
    pub fn release(&mut self, h: i64) {
        self.request(json!({"op":"release","handle":h})).unwrap();
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Unwrap a tagged value to a string.
pub fn as_str(v: &Value) -> &str {
    assert_eq!(v["t"], "s", "not a string: {v}");
    v["v"].as_str().unwrap()
}
/// Unwrap a tagged value to bytes.
pub fn as_bytes(v: &Value) -> Vec<u8> {
    assert_eq!(v["t"], "bytes", "not bytes: {v}");
    unbase64(v["v"].as_str().unwrap())
}

/// A cached BC service-tier folder that holds `Microsoft.Dynamics.Nav.Types.dll`:
/// `ALRT_BC_SERVICE_DIR`, else build 27.5.46862.53931 (the oracle's major.minor),
/// else the highest-sorting one. Types move between builds (see ADR-014), so the
/// spike pins a build by default.
pub fn bc_service_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ALRT_BC_SERVICE_DIR") {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(&std::env::var("HOME").ok()?).join(".local/share/al-runner/artifacts");
    let pinned = root.join("27.5.46862.53931");
    if pinned.join("Microsoft.Dynamics.Nav.Types.dll").exists() {
        return Some(pinned);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("Microsoft.Dynamics.Nav.Types.dll").exists())
        .collect();
    dirs.sort();
    dirs.pop()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n =
            (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn unbase64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|c| *c != b'=') {
        acc = acc << 6 | B64.iter().position(|b| *b == c).expect("bad base64") as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    out
}
