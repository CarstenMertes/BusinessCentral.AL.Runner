//! Runtime-provided .NET classes (ADR-014 section 1): server internals that cannot work outside
//! a service tier (`NavTenantSettingsHelper`, `NavUserAccountHelper`, `ALConfigSettings`,
//! `NavDesignerALFunctions`, ...). The registry is consulted before the host. A type found here
//! is served here for all its members; a member the registry does not implement is refused as
//! `Type.Member` and never forwarded to .NET.
//!
//! Every member names its pin: a corpus test (`file:line`) or a probe id from
//! `docs/probes-needed.md`. A member without either does not build (`Registry::new` refuses it).
//! The shipped registry is empty until a probe pins a member; the members wanted first are
//! listed in `docs/probes-needed.md`.

use crate::Session;
use al_builtins::native::Signal;
use al_builtins::value::{DotNetObject, Value};
use std::rc::Rc;

/// What a member is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Construct,
    Call,
    Get,
    Set,
}

/// Evidence for a member's behaviour (hard rule 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    /// A corpus test, as `file:line`.
    Corpus(String),
    /// A probe listed in `docs/probes-needed.md` and merged into the corpus repo.
    Probe(String),
}

impl Pin {
    fn is_empty(&self) -> bool {
        match self {
            Pin::Corpus(s) | Pin::Probe(s) => s.trim().is_empty(),
        }
    }
}

/// The implementation of one member. `this` is the registry object the call is made on (`None`
/// for a static member or a constructor); the result of a constructor is the new object.
pub type MemberFn = fn(&mut Session, Option<&Rc<DotNetObject>>, &[Value]) -> Result<Value, Signal>;

pub struct RegistryMember {
    pub name: String,
    pub kind: MemberKind,
    pub pin: Pin,
    pub call: MemberFn,
}

pub struct RegistryType {
    /// The CLR type name the `dotnet { }` block declares.
    pub clr_name: String,
    pub members: Vec<RegistryMember>,
}

#[derive(Default)]
pub struct Registry {
    types: Vec<RegistryType>,
}

impl Registry {
    /// The registry shipped with the runtime. Empty: no member is pinned yet.
    pub fn builtin() -> Registry {
        Registry::default()
    }

    /// Builds a registry, refusing any member without a pin.
    pub fn new(types: Vec<RegistryType>) -> Result<Registry, String> {
        for t in &types {
            for m in &t.members {
                if m.pin.is_empty() {
                    return Err(format!(
                        "registry member {}.{} names neither a corpus pin nor a probe",
                        t.clr_name, m.name
                    ));
                }
            }
        }
        Ok(Registry { types })
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn get(&self, clr_name: &str) -> Option<&RegistryType> {
        self.types.iter().find(|t| t.clr_name.eq_ignore_ascii_case(clr_name))
    }
}

impl RegistryType {
    /// The member for an access. A property read (`Get`) also finds a `Call` member, because AL
    /// lets a method be called without parentheses.
    pub fn member(&self, name: &str, kind: MemberKind) -> Option<&RegistryMember> {
        let by = |k: MemberKind| {
            self.members
                .iter()
                .find(|m| m.kind == k && m.name.eq_ignore_ascii_case(name))
        };
        by(kind).or_else(|| {
            if kind == MemberKind::Get {
                by(MemberKind::Call)
            } else {
                None
            }
        })
    }
}
