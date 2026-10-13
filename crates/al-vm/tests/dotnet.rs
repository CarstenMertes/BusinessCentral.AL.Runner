//! ADR-014 in the VM, with a fake host that records every request: call order (registry, host,
//! refusal), release queue, marshalling, errors, policy. The real sidecar is exercised by
//! `al-test-cli/tests/dotnet_sidecar.rs`.

use al_builtins::native::Signal;
use al_builtins::value::{DotNetObject, Value};
use al_host::dotnet::*;
use al_sema::workspace::AppSource;
use al_vm::dotnet::policy::Policy;
use al_vm::dotnet::registry::{MemberKind, Pin, Registry, RegistryMember, RegistryType};
use al_vm::dotnet::{DotNetConfig, UNAVAILABLE};
use al_vm::{Engine, Session};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
enum Req {
    Open,
    Construct(String, Vec<DotNetArg>),
    Invoke(DotNetTarget, String, Vec<DotNetArg>),
    Get(DotNetTarget, String),
    Set(DotNetTarget, String, DotNetValue),
    Release(Vec<u64>),
    Close,
}

type Script = Arc<dyn Fn(&Req) -> Result<DotNetReply, DotNetFailure> + Send + Sync>;

#[derive(Clone)]
struct Fake {
    log: Arc<Mutex<Vec<Req>>>,
    script: Script,
    avail: Result<(), DotNetUnavailable>,
}

impl Fake {
    fn new(script: impl Fn(&Req) -> Result<DotNetReply, DotNetFailure> + Send + Sync + 'static) -> Fake {
        Fake {
            log: Default::default(),
            script: Arc::new(script),
            avail: Ok(()),
        }
    }
    fn log(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
    /// The requests that are not `Open`/`Release`/`Close` bookkeeping.
    fn calls(&self) -> Vec<Req> {
        self.log()
            .into_iter()
            .filter(|r| !matches!(r, Req::Open | Req::Release(_) | Req::Close))
            .collect()
    }
}

struct FakeSession(Fake);

impl FakeSession {
    fn go(&mut self, r: Req) -> Result<DotNetReply, DotNetFailure> {
        self.0.log.lock().unwrap().push(r.clone());
        (self.0.script)(&r)
    }
}

impl DotNetSession for FakeSession {
    fn construct(&mut self, ty: &DotNetType, args: Vec<DotNetArg>) -> Result<DotNetReply, DotNetFailure> {
        self.go(Req::Construct(ty.name.clone(), args))
    }
    fn invoke(&mut self, t: DotNetTarget, m: &str, a: Vec<DotNetArg>) -> Result<DotNetReply, DotNetFailure> {
        self.go(Req::Invoke(t, m.into(), a))
    }
    fn get_property(&mut self, t: DotNetTarget, n: &str) -> Result<DotNetReply, DotNetFailure> {
        self.go(Req::Get(t, n.into()))
    }
    fn set_property(&mut self, t: DotNetTarget, n: &str, v: DotNetValue) -> Result<DotNetReply, DotNetFailure> {
        self.go(Req::Set(t, n.into(), v))
    }
    fn release(&mut self, handles: &[DotNetHandle]) {
        self.0.log.lock().unwrap().push(Req::Release(handles.to_vec()));
    }
    fn close(self: Box<Self>) {
        self.0.log.lock().unwrap().push(Req::Close);
    }
}

impl DotNetHost for Fake {
    fn availability(&self) -> Result<(), DotNetUnavailable> {
        self.avail.clone()
    }
    fn open_session(&self, _: &DotNetSessionConfig) -> Box<dyn DotNetSession> {
        self.log.lock().unwrap().push(Req::Open);
        Box::new(FakeSession(self.clone()))
    }
}

fn reply(v: DotNetValue) -> Result<DotNetReply, DotNetFailure> {
    Ok(DotNetReply::value(v))
}

fn handle(h: u64, ty: &str) -> DotNetValue {
    DotNetValue::Handle { handle: h, ty: ty.into() }
}

const DOTNET: &str = "dotnet { assembly(\"netstandard\") {
    type(\"System.Convert\"; \"Convert\") { }
    type(\"System.IO.MemoryStream\"; \"MemoryStream\") { }
    type(\"System.Text.Encoding\"; \"Encoding\") { }
    type(\"System.DateTime\"; \"ClrDateTime\") { }
    type(\"System.IO.File\"; \"File\") { }
    type(\"Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper\"; \"NavTenantSettingsHelper\") { }
    type(\"System.Base64FormattingOptions\"; \"Base64FormattingOptions\") { }
  }
  assembly(\"Microsoft.Exchange.WebServices\") { type(\"Microsoft.Exchange.WebServices.Data.ExchangeService\"; \"ExchangeService\") { } }
}\n";

struct Ran {
    result: Result<Value, Signal>,
    session: Option<Session>,
}

fn compile(vars: &str, body: &str, ret: &str) -> (al_ir::Program, Vec<Vec<Option<String>>>, usize, usize) {
    let src = format!(
        "{DOTNET}codeunit 50100 C {{ procedure F(){ret} var {vars} begin {body} end; }}"
    );
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
    let ri = l.program.objects[oi].routines.iter().position(|r| r.name == "F").unwrap();
    (l.program, blocked, oi, ri)
}

fn run_with(cfg: DotNetConfig, vars: &str, body: &str, ret: &str) -> Ran {
    let (program, blocked, oi, ri) = compile(vars, body, ret);
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Ran {
            result: Err(Signal::Unsupported(r.clone())),
            session: None,
        };
    }
    let engine = Engine::with_dotnet(program, blocked, cfg);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    let result = s.call(oi, ri, inst, vec![]).map(|(v, _)| v);
    Ran {
        result,
        session: Some(s),
    }
}

fn cfg(host: &Fake) -> DotNetConfig {
    DotNetConfig {
        host: Arc::new(host.clone()),
        registry: Arc::new(Registry::builtin()),
        policy: Arc::new(Policy::builtin()),
    }
}

fn run(host: &Fake, vars: &str, body: &str, ret: &str) -> Ran {
    run_with(cfg(host), vars, body, ret)
}

fn unsupported(r: &Ran) -> String {
    match &r.result {
        Err(Signal::Unsupported(s)) => s.clone(),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

fn text(r: Ran) -> String {
    match r.result {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

// ---- call order -------------------------------------------------------------------------

fn convert_script() -> Fake {
    Fake::new(|r| match r {
        Req::Invoke(DotNetTarget::Static(t), m, a) if t.name == "System.Convert" && m == "ToBase64String" => {
            let DotNetValue::Str(s) = &a[0].value else { panic!("{a:?}") };
            reply(DotNetValue::Str(format!("b64({s})")))
        }
        other => panic!("unexpected request {other:?}"),
    })
}

#[test]
fn a_call_on_the_declared_type_reaches_the_host_with_the_clr_type_and_member() {
    let host = convert_script();
    let r = run(&host, "C: DotNet Convert;", "exit(C.ToBase64String('hi'));", ": Text");
    assert_eq!(text(r), "b64(hi)");
    let log = host.log();
    assert_eq!(log[0], Req::Open, "the session starts at the first call");
    assert!(matches!(&log[1], Req::Invoke(DotNetTarget::Static(t), m, _) if t.name == "System.Convert" && t.assembly == "netstandard" && m == "ToBase64String"));
    assert_eq!(log.last(), Some(&Req::Close), "the host session closes with the VM session");
}

#[test]
fn a_run_that_never_calls_dotnet_starts_nothing() {
    let host = convert_script();
    let r = run(&host, "C: DotNet Convert; I: Integer;", "I := 1; exit(Format(I));", ": Text");
    assert_eq!(text(r), "1");
    assert_eq!(host.log(), vec![], "no Open, no Close");
}

#[test]
fn without_a_host_the_call_is_refused_naming_the_type() {
    let r = run_with(DotNetConfig::default(), "C: DotNet Convert;", "exit(C.ToBase64String('hi'));", ": Text");
    let why = unsupported(&r);
    assert!(why.starts_with(UNAVAILABLE), "{why}");
    assert!(why.contains("DotNet 'System.Convert' needs a .NET host; none is configured"), "{why}");
}

#[test]
fn an_unavailable_host_refuses_with_its_reason_and_opens_no_session() {
    let mut host = convert_script();
    host.avail = Err(DotNetUnavailable(
        "needs the .NET 8 runtime; found 9.0.15; the sidecar needs Microsoft.NETCore.App 8.x".into(),
    ));
    let r = run(&host, "C: DotNet Convert;", "exit(C.ToBase64String('hi'));", ": Text");
    let why = unsupported(&r);
    assert!(why.starts_with(UNAVAILABLE) && why.contains("found 9.0.15"), "{why}");
    assert_eq!(host.log(), vec![]);
}

fn tenant_registry(hits: &'static std::sync::atomic::AtomicUsize) -> Registry {
    fn is_sandbox(_: &mut Session, _: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
        Ok(Value::Boolean(false))
    }
    let _ = hits;
    Registry::new(vec![RegistryType {
        clr_name: "Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper".into(),
        members: vec![RegistryMember {
            name: "IsSandbox".into(),
            kind: MemberKind::Call,
            pin: Pin::Probe("test".into()),
            call: is_sandbox,
        }],
    }])
    .unwrap()
}

#[test]
fn a_registered_type_is_served_by_the_registry_and_never_reaches_the_host() {
    static HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let host = Fake::new(|r| panic!("the host must not be called: {r:?}"));
    let mut c = cfg(&host);
    c.registry = Arc::new(tenant_registry(&HITS));
    let r = run_with(c, "H: DotNet NavTenantSettingsHelper; B: Boolean;", "B := H.IsSandbox(); exit(Format(B));", ": Text");
    assert_eq!(text(r), "No");
    assert_eq!(host.log(), vec![], "not even a session was opened");
}

#[test]
fn an_unregistered_member_of_a_registered_type_is_refused_by_name_not_forwarded() {
    static HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let host = Fake::new(|r| panic!("the host must not be called: {r:?}"));
    let mut c = cfg(&host);
    c.registry = Arc::new(tenant_registry(&HITS));
    let r = run_with(c, "H: DotNet NavTenantSettingsHelper; B: Boolean;", "B := H.IsProduction(); exit(Format(B));", ": Text");
    let why = unsupported(&r);
    assert!(why.contains("NavTenantSettingsHelper.IsProduction"), "{why}");
    assert!(why.contains("never forwarded"), "{why}");
    assert_eq!(host.log(), vec![]);
}

#[test]
fn a_registry_member_without_a_pin_does_not_build() {
    fn nothing(_: &mut Session, _: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
        Ok(Value::Null)
    }
    let err = Registry::new(vec![RegistryType {
        clr_name: "X".into(),
        members: vec![RegistryMember {
            name: "M".into(),
            kind: MemberKind::Call,
            pin: Pin::Corpus("  ".into()),
            call: nothing,
        }],
    }])
    .err()
    .expect("refused");
    assert!(err.contains("X.M") && err.contains("neither a corpus pin nor a probe"), "{err}");
    assert!(Registry::builtin().is_empty(), "no member is pinned yet");
}

// ---- policy ------------------------------------------------------------------------------

#[test]
fn policy_refusals_name_the_rule_and_never_reach_the_host() {
    let host = Fake::new(|r| panic!("the host must not be called: {r:?}"));
    let r = run(&host, "F: DotNet File;", "F.Delete('x');", "");
    let why = unsupported(&r);
    assert!(why.contains("System.IO.File") && why.contains("refused by policy"), "{why}");
    let r = run(&host, "D: DotNet ClrDateTime; V: DateTime;", "V := D.Now;", "");
    assert!(unsupported(&r).contains("System.DateTime.Now"));
    let r = run(&host, "E: DotNet ExchangeService;", "E.Foo();", "");
    let why = unsupported(&r);
    assert!(why.contains("Microsoft.Exchange.WebServices") && why.contains("network"), "{why}");
    assert_eq!(host.log(), vec![]);
}

#[test]
fn an_unregistered_server_class_is_refused_before_the_host() {
    let host = Fake::new(|r| panic!("the host must not be called: {r:?}"));
    let r = run(&host, "H: DotNet NavTenantSettingsHelper; B: Boolean;", "B := H.IsSandbox();", "");
    let why = unsupported(&r);
    assert!(why.contains("server class") && why.contains("neither registered nor classified pure"), "{why}");
    assert_eq!(host.log(), vec![]);
}

// ---- lifetime ----------------------------------------------------------------------------

fn stream_script() -> Fake {
    let next = Arc::new(std::sync::atomic::AtomicU64::new(1));
    Fake::new(move |r| match r {
        Req::Construct(t, _) if t == "System.IO.MemoryStream" => reply(handle(
            next.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            "System.IO.MemoryStream",
        )),
        Req::Get(DotNetTarget::Instance(_), n) if n == "Length" => reply(DotNetValue::Int64(3)),
        other => panic!("unexpected request {other:?}"),
    })
}

#[test]
fn a_dropped_object_goes_on_the_release_queue_and_the_queue_flushes_before_the_next_call() {
    let host = stream_script();
    let r = run(
        &host,
        "M: DotNet MemoryStream; L: BigInteger;",
        "M := M.MemoryStream(); M := M.MemoryStream(); L := M.Length;",
        "",
    );
    assert!(r.result.is_ok(), "{:?}", r.result);
    let log = host.log();
    // The first object is dropped by the second assignment. Dropping only queues it; the host
    // hears about it just before the next call.
    assert_eq!(log[1], Req::Construct("System.IO.MemoryStream".into(), vec![]));
    assert_eq!(log[2], Req::Construct("System.IO.MemoryStream".into(), vec![]));
    assert_eq!(log[3], Req::Release(vec![1]));
    assert_eq!(log[4], Req::Get(DotNetTarget::Instance(2), "Length".into()));
}

#[test]
fn assignment_shares_the_reference_and_a_handle_is_released_once_after_the_last_owner_goes() {
    let host = stream_script();
    let r = run(
        &host,
        "M: DotNet MemoryStream; N: DotNet MemoryStream; L: BigInteger;",
        "M := M.MemoryStream(); N := M; M := M.MemoryStream(); L := M.Length; L := N.Length;",
        "",
    );
    assert!(r.result.is_ok(), "{:?}", r.result);
    // N still holds the first object: nothing is released while it does, and both Length reads
    // address the handles of their own variable.
    assert_eq!(
        host.calls(),
        vec![
            Req::Construct("System.IO.MemoryStream".into(), vec![]),
            Req::Construct("System.IO.MemoryStream".into(), vec![]),
            Req::Get(DotNetTarget::Instance(2), "Length".into()),
            Req::Get(DotNetTarget::Instance(1), "Length".into()),
        ]
    );
    assert!(!host.log().iter().any(|q| matches!(q, Req::Release(_))), "{:?}", host.log());
    drop(r);
    let mut released: Vec<u64> = host
        .log()
        .iter()
        .filter_map(|q| if let Req::Release(h) = q { Some(h.clone()) } else { None })
        .flatten()
        .collect();
    released.sort();
    assert_eq!(released, vec![1, 2], "each handle exactly once, at session end");
    assert_eq!(host.log().last(), Some(&Req::Close));
}

#[test]
fn session_end_flushes_the_queue_and_closes_the_host() {
    let host = stream_script();
    let (program, blocked, oi, ri) = compile(
        "M: DotNet MemoryStream; L: BigInteger;",
        "M := M.MemoryStream(); L := M.Length;",
        "",
    );
    let engine = Engine::with_dotnet(program, blocked, cfg(&host));
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).unwrap();
    assert!(!host.log().contains(&Req::Close), "still open while the session lives");
    drop(s);
    let log = host.log();
    // The routine's locals (and the object it held) are gone: the handle is released, then the
    // host session closes.
    assert_eq!(&log[log.len() - 2..], &[Req::Release(vec![1]), Req::Close]);
}

#[test]
fn constructing_property_get_and_a_returned_handle_round_trip() {
    let host = stream_script();
    let r = run(
        &host,
        "M: DotNet MemoryStream; L: BigInteger;",
        "M := M.MemoryStream(); L := M.Length; exit(Format(L));",
        ": Text",
    );
    assert_eq!(text(r), "3");
    assert_eq!(
        host.calls(),
        vec![
            Req::Construct("System.IO.MemoryStream".into(), vec![]),
            Req::Get(DotNetTarget::Instance(1), "Length".into()),
        ]
    );
}
