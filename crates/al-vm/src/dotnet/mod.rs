//! `DotNet` calls (ADR-014). The router: registry of runtime-provided types, then the host
//! provider, then refusal. Marshalling between AL values and the host's wire values lives here
//! too.

pub mod policy;
pub mod registry;

use crate::{Frame, Session};
use al_builtins::native::Signal;
use al_builtins::ops;
use al_builtins::stream::BlobRef;
use al_builtins::value::{DotNetKind, DotNetObject, LibraryValue, RtType, Value};
use al_host::dotnet::{
    DotNetArg, DotNetFailure, DotNetHandle, DotNetHost, DotNetReply, DotNetSession, DotNetSessionConfig, DotNetTarget,
    DotNetType, DotNetValue,
};
use al_ir as ir;
use policy::{Access, Policy};
use registry::{MemberKind, Registry};
use rust_decimal::Decimal;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Marks a refusal that comes from the machine, not from the code under test: no .NET host, no
/// runtime, wrong major version, sidecar not built. The runner counts a baseline case refused
/// this way as skipped, not as a regression (ADR-014 section 11).
pub const UNAVAILABLE: &str = "dotnet-unavailable";

/// What the engine needs to serve `DotNet` calls. Shared by every session.
#[derive(Clone)]
pub struct DotNetConfig {
    pub host: Arc<dyn DotNetHost>,
    pub registry: Arc<Registry>,
    pub policy: Arc<Policy>,
}

impl Default for DotNetConfig {
    fn default() -> Self {
        DotNetConfig {
            host: Arc::new(al_host::dotnet::NoDotNet),
            registry: Arc::new(Registry::builtin()),
            policy: Arc::new(Policy::builtin()),
        }
    }
}

// The registry holds fn pointers and strings, the policy strings, the host is `Send + Sync`.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DotNetConfig>();
};

/// A session's .NET side: the host session (opened at the first call that reaches the host),
/// the queue of dropped handles, and the AL streams behind the host's write bridges.
#[derive(Default)]
pub struct DotNetState {
    session: Option<Box<dyn DotNetSession>>,
    release: Rc<RefCell<Vec<DotNetHandle>>>,
    /// `availability()` of the host, asked once per session.
    available: Option<Result<(), String>>,
    bridges: HashMap<u64, BlobRef>,
}

impl DotNetState {
    /// Hands dropped handles to the host. Called before every call and when the session closes.
    fn flush(&mut self) {
        let dropped: Vec<DotNetHandle> = std::mem::take(&mut *self.release.borrow_mut());
        if !dropped.is_empty() {
            if let Some(s) = &mut self.session {
                s.release(&dropped);
            }
        }
    }
}

impl Drop for DotNetState {
    fn drop(&mut self) {
        self.flush();
        if let Some(s) = self.session.take() {
            s.close();
        }
    }
}

/// What a call is made on, after looking at the receiver value.
enum Target {
    /// A static member (or a constructor) of the declared type.
    Static(DotNetType),
    /// An object the host holds.
    Host(DotNetHandle, String),
    /// An object the registry created.
    Internal(Rc<DotNetObject>, String),
}

/// Streams passed in one call, kept so the reply's effects can be applied.
#[derive(Default)]
struct StreamBook {
    ins: Vec<(usize, Rc<RefCell<LibraryValue>>)>,
    outs: Vec<(usize, BlobRef)>,
}

fn unsupported<T>(why: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(why.into()))
}

const TICKS_PER_MS: i64 = 10_000;
const TICKS_PER_DAY: i64 = 864_000_000_000;

impl Session {
    /// Evaluates one `Expr::DotNet` (ADR-014 section 1: registry, then host, then refuse).
    pub(crate) fn eval_dotnet(
        &mut self,
        ty: Option<usize>,
        receiver: Option<&ir::Expr>,
        op: &ir::DotNetOp,
        args: &[ir::Arg],
        f: &mut Frame,
    ) -> Result<Value, Signal> {
        let recv = match receiver {
            Some(e) => self.eval(e, f)?,
            None => Value::Null,
        };
        let mut values = Vec::with_capacity(args.len());
        let mut places: Vec<Option<ir::Place>> = Vec::with_capacity(args.len());
        for a in args {
            match a {
                ir::Arg::Value(e) => {
                    values.push(self.eval(e, f)?);
                    places.push(None);
                }
                ir::Arg::Ref(p) => {
                    values.push(self.read(p, f)?);
                    places.push(Some(p.clone()));
                }
            }
        }
        let decl = ty.map(|i| {
            let d = &self.engine.program.dotnet_types[i];
            DotNetType {
                assembly: d.assembly.clone(),
                version: d.version.clone(),
                culture: d.culture.clone(),
                public_key_token: d.public_key_token.clone(),
                name: d.type_name.clone(),
            }
        });
        let (kind, member) = match op {
            ir::DotNetOp::Construct => (
                MemberKind::Construct,
                decl.as_ref().map_or(String::new(), |d| d.name.clone()),
            ),
            ir::DotNetOp::Call(n) => (MemberKind::Call, n.clone()),
            ir::DotNetOp::Get(n) => (MemberKind::Get, n.clone()),
            ir::DotNetOp::Set(n) => (MemberKind::Set, n.clone()),
        };
        let target = match (kind, &recv) {
            (MemberKind::Construct, _) => match &decl {
                Some(d) => Target::Static(d.clone()),
                None => return unsupported("DotNet constructor without a declared type"),
            },
            (_, Value::DotNet(o)) => match &o.kind {
                DotNetKind::Host { handle, clr_type } => Target::Host(*handle, clr_type.clone()),
                DotNetKind::Internal { type_name, .. } => Target::Internal(o.clone(), type_name.clone()),
                DotNetKind::Enum { clr_type, .. } => {
                    return unsupported(format!(
                        "DotNet {clr_type}.{member}: members of a .NET enum value are not supported (ADR-014)"
                    ));
                }
            },
            (_, Value::Null) => match &decl {
                Some(d) => Target::Static(d.clone()),
                None => {
                    return unsupported(format!(
                        "DotNet member '{member}' on an unassigned DotNet value (the error BC raises is not pinned)"
                    ));
                }
            },
            (_, other) => return unsupported(format!("DotNet member '{member}' on {other:?}")),
        };
        let clr_type = match &target {
            Target::Static(d) => d.name.clone(),
            Target::Host(_, t) | Target::Internal(_, t) => t.clone(),
        };
        let shown = if kind == MemberKind::Construct {
            format!("new {clr_type}")
        } else {
            format!("{clr_type}.{member}")
        };

        // 1. The registry: a registered type is served here for all its members.
        let engine = self.engine.clone();
        if let Some(rt) = engine.dotnet.registry.get(&clr_type) {
            let Some(m) = rt.member(&member, kind) else {
                return unsupported(format!(
                    "DotNet {shown}: a member the runtime does not implement for a registry type (never forwarded to .NET, ADR-014 section 1)"
                ));
            };
            let this = match &target {
                Target::Internal(o, _) => Some(o),
                _ => None,
            };
            return (m.call)(self, this, &values);
        }

        // 2. The host provider, after the policy (ADR-014 section 8).
        let assembly = match &target {
            Target::Static(d) => Some(d.assembly.as_str()),
            _ => None,
        };
        let access = if kind == MemberKind::Construct {
            Access::Construct(values.len())
        } else {
            Access::Member(&member)
        };
        if let Err(why) = engine.dotnet.policy.check(assembly, &clr_type, access) {
            return unsupported(format!("DotNet {shown}: {why}"));
        }
        if matches!(target, Target::Internal(..)) {
            return unsupported(format!(
                "DotNet {shown}: a runtime object whose type is no longer registered"
            ));
        }

        // Marshal before the session is borrowed.
        let mut book = StreamBook::default();
        let mut wire = Vec::with_capacity(values.len());
        for (i, v) in values.iter().enumerate() {
            wire.push(DotNetArg {
                value: self.value_to_wire(v, i, &mut book).map_err(|e| prefix(e, &shown))?,
                by_ref: places[i].is_some(),
            });
        }
        self.dotnet_ready(&clr_type)?;
        let dn = &mut self.dotnet;
        let session = dn.session.as_mut().expect("opened by dotnet_ready");
        let reply = match (kind, &target) {
            (MemberKind::Construct, Target::Static(d)) => session.construct(d, wire),
            (MemberKind::Set, _) => {
                let value = wire.into_iter().next().map(|a| a.value).unwrap_or(DotNetValue::Null);
                session.set_property(host_target(&target), &member, value)
            }
            (MemberKind::Get, _) => session.get_property(host_target(&target), &member),
            (_, _) => session.invoke(host_target(&target), &member, wire),
        };
        match reply {
            Ok(r) => self.apply_reply(r, &book, &places, &shown, f),
            Err(DotNetFailure::Refused(why)) => unsupported(format!("DotNet {shown}: {why}")),
            Err(DotNetFailure::Exception(e)) => {
                let inner: String = e.inner.iter().map(|(t, m)| format!(" <- {t}: {m}")).collect();
                unsupported(format!(
                    "DotNet {shown} raised the .NET exception {}: {}{inner}; the text AL would see is not pinned (ADR-014 section 7)",
                    e.clr_type, e.message
                ))
            }
        }
    }

    /// Makes the host ready for a call: asks `availability()` once, opens the session at the
    /// first call, and flushes the release queue (ADR-014 sections 2 and 6).
    fn dotnet_ready(&mut self, clr_type: &str) -> Result<(), Signal> {
        let engine = self.engine.clone();
        let avail = self
            .dotnet
            .available
            .get_or_insert_with(|| engine.dotnet.host.availability().map_err(|e| e.0));
        if let Err(reason) = avail {
            return unsupported(format!("{UNAVAILABLE}: DotNet '{clr_type}' {reason}"));
        }
        if self.dotnet.session.is_none() {
            self.dotnet.session = Some(engine.dotnet.host.open_session(&DotNetSessionConfig::default()));
        }
        self.dotnet.flush();
        Ok(())
    }

    fn apply_reply(
        &mut self,
        reply: DotNetReply,
        book: &StreamBook,
        places: &[Option<ir::Place>],
        shown: &str,
        f: &mut Frame,
    ) -> Result<Value, Signal> {
        // Streams first: the bytes .NET read, the new bridges, the bytes written into any bridge.
        for (idx, n) in &reply.streams.consumed {
            if let Some((_, h)) = book.ins.iter().find(|(i, _)| i == idx) {
                if let LibraryValue::InStream(st) = &mut *h.borrow_mut() {
                    st.pos = (st.pos + n).min(st.len());
                }
            }
        }
        for (idx, id) in &reply.streams.bridges {
            if let Some((_, blob)) = book.outs.iter().find(|(i, _)| i == idx) {
                self.dotnet.bridges.insert(*id, blob.clone());
            }
        }
        for (id, bytes) in &reply.streams.written {
            match self.dotnet.bridges.get(id) {
                Some(blob) => al_builtins::stream::write_bytes(blob, bytes),
                None => return unsupported(format!("DotNet {shown}: bytes for an unknown stream bridge {id}")),
            }
        }
        let value = self.wire_to_value(reply.value).map_err(|e| prefix(e, shown))?;
        for (idx, v) in reply.by_ref {
            let Some(Some(place)) = places.get(idx) else { continue };
            let v = self.wire_to_value(v).map_err(|e| prefix(e, shown))?;
            let ty = self.place_type(place, f);
            let v = fit(v, &ty).map_err(|e| prefix(e, shown))?;
            self.write(place, v, f)?;
        }
        Ok(value)
    }

    /// An AL value as a wire value (ADR-014 section 4).
    fn value_to_wire(&self, v: &Value, arg: usize, book: &mut StreamBook) -> Result<DotNetValue, Signal> {
        Ok(match v {
            Value::Null => DotNetValue::Null,
            Value::Boolean(b) => DotNetValue::Bool(*b),
            Value::Integer(i) => DotNetValue::Int32(*i),
            Value::BigInteger(i) => DotNetValue::Int64(*i),
            Value::Byte(b) => DotNetValue::Byte(*b),
            Value::Char(c) => DotNetValue::Char(*c),
            Value::Decimal(d) => DotNetValue::Decimal(d.to_string()),
            Value::Text(s) | Value::Code(s) => DotNetValue::Str(s.clone()),
            // A Date or DateTime is a .NET DateTime of unspecified kind, with no time zone shift;
            // the kind and zone rule is a probe (docs/probes-needed.md).
            Value::Date(0, _) | Value::DateTime(0) => {
                return unsupported("an undefined Date or DateTime passed to .NET (what BC sends is not pinned)");
            }
            Value::Date(_, true) => return unsupported("a closing date passed to .NET (not pinned)"),
            Value::Date(d, false) => DotNetValue::DateTime((i64::from(*d) - 1) * TICKS_PER_DAY),
            Value::DateTime(ms) => DotNetValue::DateTime((ms - 1) * TICKS_PER_MS),
            Value::Time(_) => return unsupported("a Time passed to .NET (how BC maps it to DateTime is not pinned)"),
            Value::Duration(ms) => DotNetValue::TimeSpan(ms * TICKS_PER_MS),
            Value::Guid(g) => DotNetValue::Guid(*g),
            // A .NET enum parameter takes the ordinal; the host converts to the parameter's enum.
            Value::Option(o, _) => DotNetValue::Int32(*o),
            Value::Enum(_, o) => DotNetValue::Int32(*o),
            Value::DotNet(o) => match &o.kind {
                DotNetKind::Host { handle, clr_type } => DotNetValue::Handle {
                    handle: *handle,
                    ty: clr_type.clone(),
                },
                DotNetKind::Enum {
                    clr_type,
                    name,
                    ordinal,
                } => DotNetValue::Enum {
                    ty: clr_type.clone(),
                    name: name.clone(),
                    ordinal: *ordinal,
                },
                DotNetKind::Internal { type_name, .. } => {
                    return unsupported(format!("a runtime-provided {type_name} object passed to .NET code"));
                }
            },
            Value::Library(_, h) => match &*h.borrow() {
                LibraryValue::InStream(st) => {
                    book.ins.push((arg, h.clone()));
                    DotNetValue::InStream(st.rest())
                }
                LibraryValue::OutStream(blob, _) => {
                    book.outs.push((arg, blob.clone()));
                    DotNetValue::OutStream
                }
                other => return unsupported(format!("an AL library value ({other:?}) passed to .NET")),
            },
            other => {
                return unsupported(format!(
                    "{other:?} passed to .NET: the type has no marshalling row (ADR-014 section 4)"
                ));
            }
        })
    }

    /// A wire value as an AL value.
    fn wire_to_value(&self, v: DotNetValue) -> Result<Value, Signal> {
        Ok(match v {
            DotNetValue::Null => Value::Null,
            DotNetValue::Bool(b) => Value::Boolean(b),
            DotNetValue::Byte(b) => Value::Byte(b),
            DotNetValue::Char(c) => Value::Char(c),
            DotNetValue::Int32(i) => Value::Integer(i),
            DotNetValue::Int64(i) => Value::BigInteger(i),
            DotNetValue::Decimal(s) => match s.parse::<Decimal>() {
                Ok(d) => Value::Decimal(d),
                Err(_) => return unsupported(format!("a .NET decimal '{s}' does not fit an AL Decimal")),
            },
            DotNetValue::Str(s) => Value::Text(s),
            DotNetValue::DateTime(ticks) => Value::DateTime(ticks.div_euclid(TICKS_PER_MS) + 1),
            DotNetValue::TimeSpan(ticks) => Value::Duration(ticks.div_euclid(TICKS_PER_MS)),
            DotNetValue::Guid(g) => Value::Guid(g),
            DotNetValue::Enum { ty, name, ordinal } => Value::DotNet(Rc::new(DotNetObject::new(DotNetKind::Enum {
                clr_type: ty,
                name,
                ordinal,
            }))),
            DotNetValue::Handle { handle, ty } => {
                Value::DotNet(Rc::new(DotNetObject::host(handle, ty, self.dotnet.release.clone())))
            }
            DotNetValue::InStream(_) | DotNetValue::OutStream => {
                return unsupported("the host returned an AL stream value");
            }
        })
    }

    /// `Expr::DotNetCoerce`: a .NET result stored into a slot of type `t`.
    pub(crate) fn dotnet_coerce(&mut self, v: Value, t: &RtType) -> Result<Value, Signal> {
        fit(v, t)
    }
}

fn host_target(t: &Target) -> DotNetTarget {
    match t {
        Target::Static(d) => DotNetTarget::Static(d.clone()),
        Target::Host(h, _) => DotNetTarget::Instance(*h),
        Target::Internal(..) => unreachable!("registry objects never reach the host"),
    }
}

fn prefix(e: Signal, shown: &str) -> Signal {
    match e {
        Signal::Unsupported(s) => Signal::Unsupported(format!("DotNet {shown}: {s}")),
        other => other,
    }
}

/// The kind of a value, for messages.
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Boolean(_) => "Boolean",
        Value::Integer(_) => "Integer",
        Value::BigInteger(_) => "BigInteger",
        Value::Decimal(_) => "Decimal",
        Value::Byte(_) => "Byte",
        Value::Char(_) => "Char",
        Value::Text(_) => "Text",
        Value::Code(_) => "Code",
        Value::Date(..) => "Date",
        Value::Time(_) => "Time",
        Value::DateTime(_) => "DateTime",
        Value::Duration(_) => "Duration",
        Value::Guid(_) => "Guid",
        Value::DotNet(_) => "DotNet object",
        _ => "value",
    }
}

/// Stores a .NET result into a slot of type `t`. The kinds must fit exactly (Text and Code,
/// numbers widening); BC's conversion of a .NET result to a differently typed AL variable is not
/// pinned, so anything else is Unsupported (ADR-014 section 4).
pub(crate) fn fit(v: Value, t: &RtType) -> Result<Value, Signal> {
    use RtType as T;
    let ok = matches!(
        (&v, t),
        (_, T::Variant)
            | (Value::DotNet(_) | Value::Null, T::DotNet(_))
            | (Value::Text(_) | Value::Code(_), T::Text(_) | T::Code(_))
            | (Value::Boolean(_), T::Boolean)
            | (Value::Integer(_) | Value::Byte(_), T::Integer)
            | (Value::Integer(_) | Value::BigInteger(_) | Value::Byte(_), T::BigInteger)
            | (
                Value::Integer(_) | Value::BigInteger(_) | Value::Byte(_) | Value::Decimal(_),
                T::Decimal
            )
            | (Value::Byte(_), T::Byte)
            | (Value::Char(_), T::Char)
            | (Value::Date(..), T::Date)
            | (Value::DateTime(_), T::DateTime)
            | (Value::Time(_), T::Time)
            | (Value::Duration(_), T::Duration)
            | (Value::Guid(_), T::Guid)
    );
    if !ok {
        return unsupported(format!(
            "a .NET result of kind {} stored into {t:?}: BC's conversion is not pinned (ADR-014 section 4)",
            kind_of(&v)
        ));
    }
    match (&v, t) {
        (Value::DotNet(_) | Value::Null, T::DotNet(_)) => Ok(v),
        _ => ops::convert(v, t),
    }
}
