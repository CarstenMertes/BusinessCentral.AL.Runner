//! The link step (ADR-033): symbolic objects in, the dense [`Program`] out.
//!
//! A symbolic object ([`SymObject`]) refers to other objects and routines by stable key through
//! its own [`Refs`](al_ir::sym::Refs) tables. [`link`] gives every object its position, resolves
//! each key to the position it now has, and moves the source positions of every object to their
//! place in `Program::positions`. The result is what lowering used to write directly: the VM reads
//! it unchanged.
//!
//! Core crate: no OS access (ADR-006).

use al_ir::sym::{FieldKey, LocalKey, ObjKey, RoutineKey, SymObject};
use al_ir::walk::{RefVisitor, TableOf, walk_object};
use al_ir::{Object, Program, SrcPos};
use std::cell::OnceCell;
use std::collections::HashMap;

/// An object whose references are already positions in the program it goes into (a dependency
/// object taken from a compile cache before the cache holds symbolic objects). It is placed as it
/// is. Other objects still refer to it by key.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseObject {
    pub key: ObjKey,
    /// The key of each routine of `object`, in order.
    pub routines: Vec<LocalKey>,
    /// For a table: the number of each field, in the order of its field list. Used where `object`
    /// is only a placeholder (a layer build marks the objects of the layers below as supplied and
    /// never reads their lowered form).
    pub fields: Vec<u32>,
    pub object: Object,
    /// Source positions numbered from 0, as in [`SymObject::positions`].
    pub positions: Vec<SrcPos>,
}

/// One object to link.
#[derive(Debug, Clone, PartialEq)]
pub enum Unit {
    Sym(SymObject),
    Dense(DenseObject),
}

impl Unit {
    fn key(&self) -> &ObjKey {
        match self {
            Unit::Sym(s) => &s.key,
            Unit::Dense(d) => &d.key,
        }
    }

    fn routines(&self) -> &[LocalKey] {
        match self {
            Unit::Sym(s) => &s.routines,
            Unit::Dense(d) => &d.routines,
        }
    }

    fn object(&self) -> &Object {
        match self {
            Unit::Sym(s) => &s.object,
            Unit::Dense(d) => &d.object,
        }
    }
}

/// Why a set of objects cannot be linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Two objects have the same key.
    DuplicateObject(ObjKey),
    /// An object refers to an object that is not among the ones linked.
    UnknownObject { from: ObjKey, to: ObjKey },
    /// An object refers to a routine that the target object does not have.
    UnknownRoutine { from: ObjKey, to: RoutineKey },
    /// An object refers to a field that the target table does not have.
    UnknownField { from: ObjKey, to: FieldKey },
    /// Two routines of one object have the same key.
    DuplicateRoutine { object: ObjKey, key: LocalKey },
    /// A reference in the IR is not an index into the object's table (a site that lowering did not
    /// move into `refs`, or a table that is too short).
    BadReference {
        object: ObjKey,
        class: &'static str,
        index: usize,
    },
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::DuplicateObject(k) => write!(f, "two objects have the key {k}"),
            LinkError::UnknownObject { from, to } => write!(f, "{from} refers to {to}, which is not linked"),
            LinkError::UnknownRoutine { from, to } => {
                write!(
                    f,
                    "{from} refers to routine {:?} of {}, which has none",
                    to.local, to.obj
                )
            }
            LinkError::UnknownField { from, to } => {
                write!(f, "{from} refers to field {} of {}, which has none", to.no, to.table)
            }
            LinkError::DuplicateRoutine { object, key } => {
                write!(f, "{object} has two routines with the key {key:?}")
            }
            LinkError::BadReference { object, class, index } => {
                write!(f, "{object} has {class} reference {index}, outside its table")
            }
        }
    }
}

impl std::error::Error for LinkError {}

/// The linked program and where each object's source positions went.
pub struct Linked {
    pub program: Program,
    /// Per object: where its source positions are in `Program::positions` (start, count).
    pub pos_ranges: Vec<(u32, u32)>,
}

/// Links `units` into `base`, which holds everything that is not an object (`apps`, `dotnet_types`,
/// `sources`, `uncompiled_subscribers`; its `objects` and `positions` are empty). The objects come
/// out in the order of `units`.
pub fn link(units: Vec<Unit>, mut base: Program) -> Result<Linked, Box<LinkError>> {
    debug_assert!(base.objects.is_empty() && base.positions.is_empty());
    let mut at: HashMap<&ObjKey, usize> = HashMap::with_capacity(units.len());
    for (i, u) in units.iter().enumerate() {
        if at.insert(u.key(), i).is_some() {
            return Err(Box::new(LinkError::DuplicateObject(u.key().clone())));
        }
    }
    let routine_at: Vec<OnceCell<HashMap<&LocalKey, usize>>> = units.iter().map(|_| OnceCell::new()).collect();
    let routine_table = |i: usize| -> Result<&HashMap<&LocalKey, usize>, Box<LinkError>> {
        if let Some(t) = routine_at[i].get() {
            return Ok(t);
        }
        let mut t = HashMap::with_capacity(units[i].routines().len());
        for (r, k) in units[i].routines().iter().enumerate() {
            if t.insert(k, r).is_some() {
                return Err(Box::new(LinkError::DuplicateRoutine {
                    object: units[i].key().clone(),
                    key: k.clone(),
                }));
            }
        }
        Ok(routine_at[i].get_or_init(|| t))
    };
    // The position of each field number in a table: its place in the table's merged field list.
    // A query's columns are named by their position, which is the number.
    let field_at: Vec<OnceCell<HashMap<u32, usize>>> = units.iter().map(|_| OnceCell::new()).collect();
    let field_table = |i: usize| -> &HashMap<u32, usize> {
        field_at[i].get_or_init(|| {
            if let Unit::Dense(d) = &units[i] {
                if !d.fields.is_empty() {
                    return first_of(d.fields.iter().copied());
                }
            }
            match &units[i].object().table {
                Some(t) => first_of(t.fields.iter().map(|f| f.id)),
                None => HashMap::new(),
            }
        })
    };
    // What each symbolic object's tables mean in the program: the position of every key.
    let mut resolved: Vec<Option<Resolved>> = Vec::with_capacity(units.len());
    for u in &units {
        let Unit::Sym(s) = u else {
            resolved.push(None);
            continue;
        };
        let mut objects = Vec::with_capacity(s.refs.objects.len());
        for k in &s.refs.objects {
            objects.push(*at.get(k).ok_or_else(|| {
                Box::new(LinkError::UnknownObject {
                    from: s.key.clone(),
                    to: k.clone(),
                })
            })?);
        }
        let mut routines = Vec::with_capacity(s.refs.routines.len());
        for k in &s.refs.routines {
            let o = *at.get(&k.obj).ok_or_else(|| {
                Box::new(LinkError::UnknownObject {
                    from: s.key.clone(),
                    to: k.obj.clone(),
                })
            })?;
            let r = *routine_table(o)?
                .get(&k.local)
                .ok_or_else(|| LinkError::UnknownRoutine {
                    from: s.key.clone(),
                    to: k.clone(),
                })?;
            routines.push(r);
        }
        let mut fields = Vec::with_capacity(s.refs.fields.len());
        for k in &s.refs.fields {
            let o = *at.get(&k.table).ok_or_else(|| {
                Box::new(LinkError::UnknownObject {
                    from: s.key.clone(),
                    to: k.table.clone(),
                })
            })?;
            // A query's column is named by its position, which is the number.
            let pos = if k.table.kind == al_ir::ObjKind::Query {
                Some(k.no as usize)
            } else {
                field_table(o).get(&k.no).copied()
            };
            let pos = pos.ok_or_else(|| {
                Box::new(LinkError::UnknownField {
                    from: s.key.clone(),
                    to: k.clone(),
                })
            })?;
            fields.push(pos);
        }
        resolved.push(Some(Resolved {
            objects,
            routines,
            fields,
        }));
    }
    drop(routine_at);
    drop(field_at);
    drop(at);
    let mut pos_ranges = Vec::with_capacity(units.len());
    base.objects.reserve(units.len());
    for (i, (u, res)) in units.into_iter().zip(resolved).enumerate() {
        let (mut object, mut positions, key) = match u {
            Unit::Sym(s) => (s.object, s.positions, s.key),
            Unit::Dense(d) => (d.object, d.positions, d.key),
        };
        if let Some(res) = res {
            let mut w = Rewrite { res: &res, bad: None };
            walk_object(&mut object, &mut w);
            if let Some((class, index)) = w.bad {
                return Err(Box::new(LinkError::BadReference {
                    object: key,
                    class,
                    index,
                }));
            }
        }
        let start = base.positions.len() as u32;
        pos_ranges.push((start, positions.len() as u32));
        if !positions.is_empty() {
            for r in &mut object.routines {
                r.shift_positions(start);
            }
            for p in &mut positions {
                p.object = i as u32;
            }
            base.positions.append(&mut positions);
        }
        base.objects.push(object);
    }
    Ok(Linked {
        program: base,
        pos_ranges,
    })
}

/// The positions the keys of one symbolic object's tables resolved to.
struct Resolved {
    objects: Vec<usize>,
    routines: Vec<usize>,
    fields: Vec<usize>,
}

struct Rewrite<'a> {
    res: &'a Resolved,
    bad: Option<(&'static str, usize)>,
}

impl RefVisitor for Rewrite<'_> {
    fn object(&mut self, r: &mut usize) {
        match self.res.objects.get(*r) {
            Some(&p) => *r = p,
            None => self.bad = self.bad.or(Some(("object", *r))),
        }
    }

    fn routine(&mut self, r: &mut usize, _object: usize) {
        match self.res.routines.get(*r) {
            Some(&p) => *r = p,
            None => self.bad = self.bad.or(Some(("routine", *r))),
        }
    }

    fn field(&mut self, r: &mut usize, _table: TableOf) {
        match self.res.fields.get(*r) {
            Some(&p) => *r = p,
            None => self.bad = self.bad.or(Some(("field", *r))),
        }
    }
}

/// The position of each field number. A merged field list can hold a number twice (an app that
/// extends a table redeclares a field); the lowering took the first, so a key does too.
fn first_of(ids: impl Iterator<Item = u32>) -> HashMap<u32, usize> {
    let mut at = HashMap::new();
    for (p, id) in ids.enumerate() {
        at.entry(id).or_insert(p);
    }
    at
}

/// The first place where two programs differ, readable; `None` when they are equal. The proof of
/// ADR-033: what link makes must be exactly what lowering used to write.
pub fn first_difference(a: &Program, b: &Program) -> Option<String> {
    if a == b {
        return None;
    }
    if a.objects.len() != b.objects.len() {
        return Some(format!("{} objects against {}", a.objects.len(), b.objects.len()));
    }
    for (i, (x, y)) in a.objects.iter().zip(&b.objects).enumerate() {
        if x != y {
            return Some(format!("object {i} `{}`: {}", x.name, text_difference(x, y)));
        }
    }
    if a.positions != b.positions {
        let at = a
            .positions
            .iter()
            .zip(&b.positions)
            .position(|(x, y)| x != y)
            .unwrap_or(a.positions.len().min(b.positions.len()));
        return Some(format!(
            "source position {at}: {:?} against {:?} ({} and {} positions)",
            a.positions.get(at),
            b.positions.get(at),
            a.positions.len(),
            b.positions.len()
        ));
    }
    if a.apps != b.apps {
        return Some("apps".into());
    }
    if a.dotnet_types != b.dotnet_types {
        return Some("dotnet_types".into());
    }
    if a.sources != b.sources {
        return Some("sources".into());
    }
    if a.uncompiled_subscribers != b.uncompiled_subscribers {
        return Some("uncompiled_subscribers".into());
    }
    Some("programs differ in a part this report does not know".into())
}

fn text_difference<T: std::fmt::Debug>(a: &T, b: &T) -> String {
    let (x, y) = (format!("{a:?}"), format!("{b:?}"));
    let at = x
        .bytes()
        .zip(y.bytes())
        .position(|(p, q)| p != q)
        .unwrap_or(x.len().min(y.len()));
    let from = (0..=at.saturating_sub(120))
        .rev()
        .find(|&i| x.is_char_boundary(i) && y.is_char_boundary(i))
        .unwrap_or(0);
    let cut = |t: &str| {
        let end = (at + 120).min(t.len());
        let end = (end..=t.len()).find(|&i| t.is_char_boundary(i)).unwrap_or(t.len());
        t[from..end].to_string()
    };
    format!(
        "first difference at byte {at}:\n  expected ...{}\n  linked   ...{}",
        cut(&x),
        cut(&y)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_ir::sym::{ObjName, Refs};
    use al_ir::{Arg, Expr, ObjKind, Routine, Stmt};

    fn key(id: u32) -> ObjKey {
        ObjKey {
            app: "app".into(),
            kind: ObjKind::Codeunit,
            name: ObjName::Id(id),
        }
    }

    fn local(name: &str, ordinal: u32) -> LocalKey {
        LocalKey {
            name: name.into(),
            section: String::new(),
            sig: String::new(),
            ordinal,
        }
    }

    fn routine(name: &str, body: Vec<Stmt>) -> Routine {
        Routine {
            name: name.into(),
            field: None,
            container: None,
            is_trigger: false,
            attributes: vec![],
            params: 0,
            by_var: vec![],
            locals: vec![],
            ret: None,
            ret_local: None,
            body,
            pos: al_ir::NO_POS,
            end_pos: al_ir::NO_POS,
            begin_pos: al_ir::NO_POS,
        }
    }

    fn object(id: u32, routines: Vec<Routine>) -> Object {
        Object {
            kind: ObjKind::Codeunit,
            id: Some(id),
            name: format!("C{id}"),
            routines,
            ..Default::default()
        }
    }

    /// Codeunit `from` whose routine `Run` calls routine `Second` of codeunit `to`.
    fn caller(from: u32, to: u32) -> SymObject {
        let call = Stmt::Expr(Expr::Call {
            object: 0,
            routine: 0,
            instance: None,
            args: Vec::<Arg>::new(),
            try_call: false,
        });
        SymObject {
            key: key(from),
            routines: vec![local("run", 0)],
            refs: Refs {
                objects: vec![key(to)],
                routines: vec![RoutineKey {
                    obj: key(to),
                    local: local("second", 0),
                }],
                ..Default::default()
            },
            object: object(from, vec![routine("Run", vec![call])]),
            positions: vec![],
        }
    }

    fn callee(id: u32) -> SymObject {
        SymObject {
            key: key(id),
            routines: vec![local("first", 0), local("second", 0)],
            refs: Refs::default(),
            object: object(id, vec![routine("First", vec![]), routine("Second", vec![])]),
            positions: vec![],
        }
    }

    fn call_of(p: &Program, at: usize) -> (usize, usize) {
        match &p.objects[at].routines[0].body[0] {
            Stmt::Expr(Expr::Call { object, routine, .. }) => (*object, *routine),
            other => panic!("not a call: {other:?}"),
        }
    }

    #[test]
    fn a_call_finds_its_target_wherever_the_target_is() {
        let a = link(vec![Unit::Sym(caller(1, 2)), Unit::Sym(callee(2))], Program::default()).unwrap();
        assert_eq!(call_of(&a.program, 0), (1, 1));
        let b = link(
            vec![Unit::Sym(callee(2)), Unit::Sym(callee(3)), Unit::Sym(caller(1, 2))],
            Program::default(),
        )
        .unwrap();
        assert_eq!(call_of(&b.program, 2), (0, 1));
    }

    #[test]
    fn a_dependency_object_placed_as_it_is_can_still_be_called_by_key() {
        let dense = DenseObject {
            key: key(2),
            routines: vec![local("first", 0), local("second", 0)],
            fields: vec![],
            object: object(2, vec![routine("First", vec![]), routine("Second", vec![])]),
            positions: vec![],
        };
        let linked = link(vec![Unit::Dense(dense), Unit::Sym(caller(1, 2))], Program::default()).unwrap();
        assert_eq!(call_of(&linked.program, 1), (0, 1));
    }

    #[test]
    fn a_missing_target_is_an_error_that_names_it() {
        let e = link(vec![Unit::Sym(caller(1, 2))], Program::default()).err().unwrap();
        assert_eq!(
            *e,
            LinkError::UnknownObject {
                from: key(1),
                to: key(2)
            }
        );
        let mut c = callee(2);
        c.routines.truncate(1);
        c.object.routines.truncate(1);
        let e = link(vec![Unit::Sym(caller(1, 2)), Unit::Sym(c)], Program::default())
            .err()
            .unwrap();
        assert!(matches!(*e, LinkError::UnknownRoutine { .. }), "{e:?}");
    }

    #[test]
    fn two_objects_with_one_key_do_not_link() {
        let e = link(vec![Unit::Sym(callee(2)), Unit::Sym(callee(2))], Program::default())
            .err()
            .unwrap();
        assert_eq!(*e, LinkError::DuplicateObject(key(2)));
    }

    #[test]
    fn a_reference_outside_the_table_is_an_error() {
        let mut c = caller(1, 2);
        c.refs.objects.clear();
        let e = link(vec![Unit::Sym(c), Unit::Sym(callee(2))], Program::default())
            .err()
            .unwrap();
        assert!(matches!(*e, LinkError::BadReference { class: "object", .. }), "{e:?}");
    }

    #[test]
    fn source_positions_move_to_their_place_in_the_program() {
        let pos = |line| SrcPos {
            file: 0,
            line,
            col: 1,
            end_line: line,
            object: 0,
            routine: 0,
        };
        let mut a = callee(1);
        a.positions = vec![pos(1), pos(2)];
        a.object.routines[0].body = vec![Stmt::Pos(1)];
        a.object.routines[0].pos = 0;
        let mut b = callee(2);
        b.positions = vec![pos(7)];
        b.object.routines[1].pos = 0;
        let linked = link(vec![Unit::Sym(a), Unit::Sym(b)], Program::default()).unwrap();
        assert_eq!(linked.pos_ranges, vec![(0, 2), (2, 1)]);
        assert_eq!(linked.program.objects[1].routines[1].pos, 2);
        assert_eq!(linked.program.positions[2].line, 7);
        assert_eq!(linked.program.positions[2].object, 1);
    }
}
