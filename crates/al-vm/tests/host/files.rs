//! The `File` type on the host file system capability (ADR-006), end to end.
//!
//! The corpus pins no `File` behaviour: its only reach is Base Application report 91 "Export
//! Consolidation" (handlers/TestPrecompiledOptionCaption.al and others), which needs the calls to
//! lower. What runs here is the documented behaviour of the Business Central `File` data type
//! (Microsoft Learn, "File data type") on an in-memory file system, and each assumption is a row
//! in docs/probes-needed.md. Whatever is not documented plainly is refused.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_host::{Host, MemFileSystem};
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn compile(body: &str) -> (std::sync::Arc<Engine>, usize) {
    let src = format!("codeunit 50100 C {{ {body} }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    for (ri, r) in l.program.objects[oi].routines.iter().enumerate() {
        if let Some(Some(why)) = blocked.get(oi).and_then(|o| o.get(ri)) {
            panic!("{} is blocked: {why}", r.name);
        }
    }
    (Engine::new(l.program, blocked), oi)
}

fn call(s: &mut Session, oi: usize) -> Result<Value, Signal> {
    let ri = s.engine.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

/// Runs `F` on a session whose file system is `fs`; returns the answer and the session.
fn run_on(fs: MemFileSystem, vars: &str, body: &str) -> (Result<Value, Signal>, Session) {
    let (engine, oi) = compile(&format!("procedure F(): Text var {vars} begin {body} end;"));
    let mut s = Session::with_host(engine, Host::deterministic().with_files(fs));
    let r = call(&mut s, oi);
    (r, s)
}

fn text(vars: &str, body: &str) -> String {
    match run_on(MemFileSystem::default(), vars, body).0 {
        Ok(Value::Text(t)) => t.to_string(),
        other => panic!("{other:?}"),
    }
}

fn refused(vars: &str, body: &str) -> String {
    match run_on(MemFileSystem::default(), vars, body).0 {
        Err(Signal::Unsupported(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_created_file_exists_after_close_with_its_text_and_line_ends() {
    let (r, s) = run_on(
        MemFileSystem::default(),
        "F: File;",
        "F.TextMode := true; F.WriteMode := true; F.Create('out.txt', TextEncoding::UTF8);
         F.Write('abc'); F.Write('d'); F.Close(); exit(Format(File.Exists('out.txt')));",
    );
    assert!(matches!(r, Ok(Value::Text(ref t)) if t == "Yes"), "{r:?}");
    assert_eq!(s.files().read("out.txt").unwrap(), b"abc\r\nd\r\n");
}

#[test]
fn a_file_read_back_through_an_instream_gives_the_lines() {
    assert_eq!(
        text(
            "F: File; I: InStream; T: Text;",
            "F.Create('a.txt'); F.TextMode(true); F.Write('one'); F.Write('two'); F.Close();
             F.Open('a.txt'); F.TextMode(true); F.CreateInStream(I); I.ReadText(T); T := T + '/'; I.ReadText(T); F.Close(); exit(T);",
        ),
        "two"
    );
}

#[test]
fn an_outstream_on_a_created_file_writes_to_it() {
    let (r, s) = run_on(
        MemFileSystem::default(),
        "F: File; O: OutStream;",
        "F.Create('s.bin'); F.CreateOutStream(O); O.WriteText('xyz'); F.Close();",
    );
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(s.files().read("s.bin").unwrap(), b"xyz");
}

#[test]
fn exists_erase_copy_and_rename_work_on_the_host_files() {
    assert_eq!(
        text(
            "F: File; R: Text;",
            "F.Create('a'); F.TextMode := true; F.Write('x'); F.Close();
             R := Format(File.Exists('a')) + Format(File.Exists('nope'));
             File.Copy('a', 'b'); R += Format(File.Exists('b'));
             File.Rename('b', 'c'); R += Format(File.Exists('b')) + Format(File.Exists('c'));
             File.Erase('a'); R += Format(File.Exists('a')); exit(R);",
        ),
        "YesNoYesNoYesNo"
    );
}

#[test]
fn a_temporary_file_has_a_name_and_is_temporary() {
    assert_eq!(
        text(
            "F: File;",
            "F.CreateTempFile(); exit(Format(F.Name() <> '') + Format(File.IsPathTemporary(F.Name())));",
        ),
        "YesYes"
    );
}

#[test]
fn what_needs_a_client_or_is_not_documented_plainly_is_refused() {
    assert!(refused("F: File; T: Text;", "File.Download('a', 'b', 'c', 'd', T);").contains("File.Download"));
    assert!(
        refused(
            "F: File; I: InStream; T: Text;",
            "File.UploadIntoStream('t', '', '', T, I);"
        )
        .contains("File.UploadIntoStream")
    );
    // A write in binary mode: the binary form of a Text is not documented plainly.
    assert!(refused("F: File;", "F.Create('a'); F.Write('x');").contains("File.Write"));
}

#[test]
fn the_static_file_methods_can_be_called_without_the_type_name() {
    // Base Application's "File Management" calls `Download(...)`, `Exists(...)` and `Erase(...)`
    // unqualified (reached by the report 91 tests, handlers/TestPrecompiledOptionCaption.al).
    assert_eq!(
        text(
            "F: File;",
            "F.Create('a'); F.Close(); exit(Format(Exists('a')) + Format(Exists('b')) + Format(Erase('a')) + Format(Exists('a')));",
        ),
        "YesNoYesNo"
    );
}

/// Windows BC (runs 39125184978 BC 27.5, 39125190236 BC 29.0) answers No for drive-letter, UNC and
/// root paths; the Linux container answers Yes. Only the forms both agree on run.
#[test]
fn is_path_temporary_refuses_the_forms_where_windows_and_linux_differ() {
    for p in [
        "C:\\\\x.tmp",
        "D:\\\\x.tmp",
        "Z:/x.tmp",
        "\\\\\\\\srv\\\\share\\\\x.tmp",
        "/",
        "/etc/passwd",
    ] {
        let r = refused("B: Boolean;", &format!("B := File.IsPathTemporary('{p}');"));
        assert!(r.contains("File.IsPathTemporary"), "{p}: {r}");
    }
    for p in ["", ' '.to_string().as_str(), "rel.txt", "sub/rel.txt", "temp/1.tmp"] {
        assert_eq!(
            text("", &format!("exit(Format(File.IsPathTemporary('{p}')));")),
            "Yes",
            "{p}"
        );
    }
}
