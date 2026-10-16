//! The symbolic form of an object (ADR-033): what lowering produces and the link step turns into
//! [`crate::Program`].
//!
//! A symbolic object is an [`Object`] whose references to other objects, routines and fields are
//! not positions in the program but indices into the object's own [`Refs`] tables. Each table
//! entry is a stable key (decision D1 and D2 of the plan: an object by kind and id or name, a
//! routine by name and parameter-type signature, a field by table and field number). The object
//! therefore does not depend on which other objects are in the program or in which order they
//! come. Only link knows positions.

use crate::{Object, SrcPos};
use serde::{Deserialize, Serialize};

pub use crate::ObjKind;

/// The name part of an [`ObjKey`] (owner decision D1: the id where the object has one).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ObjName {
    Id(u32),
    /// The namespace-qualified name in lower case, for a kind without an id (an interface).
    Name(String),
}

impl Default for ObjName {
    fn default() -> ObjName {
        ObjName::Id(0)
    }
}

/// The identity of an object: the app that declares it, its kind, and its id or name.
///
/// The app is part of the key because two apps of one program can declare the same kind and id
/// (the Microsoft apps of one BC version do: a few tables and many permission sets exist twice).
/// A reference resolved to one of them must not resolve to the other when the set of apps changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct ObjKey {
    /// The id of the declaring app (`AppInfo::id`).
    pub app: String,
    pub kind: ObjKind,
    pub name: ObjName,
}

impl ObjKey {
    /// The key of an object with this app, kind, id, namespace and name.
    pub fn new(app: &str, kind: ObjKind, id: Option<u32>, namespace: Option<&str>, name: &str) -> ObjKey {
        let name = match id {
            Some(id) => ObjName::Id(id),
            None => ObjName::Name(match namespace {
                Some(ns) if !ns.is_empty() => format!("{}.{}", ns.to_lowercase(), name.to_lowercase()),
                _ => name.to_lowercase(),
            }),
        };
        ObjKey {
            app: app.to_string(),
            kind,
            name,
        }
    }

    /// The key of an object of a lowered program; `apps` is `Program::apps`.
    pub fn of(o: &Object, apps: &[crate::AppInfo]) -> ObjKey {
        let app = apps.get(o.app as usize).map_or("", |a| a.id.as_str());
        ObjKey::new(app, o.kind, o.id, o.namespace.as_deref(), &o.name)
    }
}

impl std::fmt::Display for ObjKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            ObjName::Id(id) => write!(f, "{} {id}", self.kind)?,
            ObjName::Name(n) => write!(f, "{} \"{n}\"", self.kind)?,
        }
        write!(f, " of app {}", self.app)
    }
}

/// The identity of a routine inside its object (owner decision D2): its name in lower case, the
/// section it belongs to when it is a trigger of a field or a control, and the parameter types.
/// `ordinal` separates routines that are equal in everything else, in declaration order (zero for
/// nearly every routine).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LocalKey {
    pub name: String,
    /// `field 7`, `control Name`, `action Name`, ...: the owning section of a trigger. Empty for
    /// a procedure and for a trigger of the object itself.
    pub section: String,
    /// The parameter types, one text per parameter, joined by `,`.
    pub sig: String,
    pub ordinal: u32,
}

/// The identity of a routine of any object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RoutineKey {
    pub obj: ObjKey,
    pub local: LocalKey,
}

/// The identity of a field: the table and the field number, extensions included.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldKey {
    pub table: ObjKey,
    pub no: u32,
}

/// The identity of a control of a page: the page and the control name (unique in a page,
/// extensions included).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ControlKey {
    pub page: ObjKey,
    pub name: String,
}

/// What a symbolic object refers to. A reference inside the object's IR is an index into one of
/// these lists.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Refs {
    pub objects: Vec<ObjKey>,
    pub routines: Vec<RoutineKey>,
    pub fields: Vec<FieldKey>,
    pub controls: Vec<ControlKey>,
}

impl Refs {
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.routines.is_empty() && self.fields.is_empty() && self.controls.is_empty()
    }
}

/// An object as lowering produces it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymObject {
    pub key: ObjKey,
    /// The key of each routine of `object`, in the order of `object.routines`.
    pub routines: Vec<LocalKey>,
    pub refs: Refs,
    /// The object. Every reference class that has moved into `refs` holds an index into it.
    pub object: Object,
    /// The source positions of the object's statements and routines, numbered from 0 (a statement
    /// marker and a routine's `pos` hold an index into this list). Link moves them to their place
    /// in `Program::positions`. Their `object` and `routine` are the object's own.
    pub positions: Vec<SrcPos>,
}
