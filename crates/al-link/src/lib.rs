//! The link step (ADR-033): symbolic objects in, the dense [`Program`] out.
//!
//! A symbolic object ([`SymObject`]) refers to other objects and routines by stable key through
//! its own [`Refs`](al_ir::sym::Refs) tables. [`link`] gives every object its position, resolves
//! each key to the position it now has, and moves the source positions of every object to their
//! place in `Program::positions`. The result is what lowering used to write directly: the VM reads
//! it unchanged.
//!
//! Core crate: no OS access (ADR-006).
use al_ir::sym::{
    Contribution, ControlKey, Expansion, FieldKey, FxMap, ListKind, LocalKey, ObjKey, RoutineKey, SymObject,
};
use al_ir::walk::{ExpansionSite, RefVisitor, TableOf, walk_object, walk_table_contribution};
use al_ir::{Object, Program, SrcPos};
use std::sync::OnceLock;

/// An object whose references are already positions in the program it goes into (a dependency
/// object taken from a compile cache before the cache holds symbolic objects). It is placed as it
/// is. Other objects still refer to it by key.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseObject {
    pub key: ObjKey,
    /// For a codeunit: the interfaces it implements, those it lists and the ones they extend.
    pub implements: Vec<ObjKey>,
    /// For a table: the number of each field, in the order of its field list. Used where `object`
    /// is only a placeholder (a layer build marks the objects of the layers below as supplied and
    /// never reads their lowered form).
    pub fields: Vec<u32>,
    /// For a page: the name of each control in lower case (ASCII), in order. Used like `fields`.
    pub controls: Vec<String>,
    pub object: Object,
    /// Source positions numbered from 0, as in [`SymObject::positions`].
    pub positions: Vec<SrcPos>,
}

/// One object to link.
#[derive(Debug, Clone, PartialEq)]
/// (The two variants differ in size; units are made once and moved, so boxing them would cost more.)
#[allow(clippy::large_enum_variant)]
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

    fn object(&self) -> &Object {
        match self {
            Unit::Sym(s) => &s.object,
            Unit::Dense(d) => &d.object,
        }
    }

    fn position_count(&self) -> u32 {
        match self {
            Unit::Sym(s) => s.positions.len() as u32,
            Unit::Dense(d) => d.positions.len() as u32,
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
    /// A codeunit that implements an interface has no procedure of the shape of the one called.
    NoImplementation {
        from: ObjKey,
        codeunit: ObjKey,
        name: String,
    },
    /// An object refers to a control that the target page does not have.
    UnknownControl { from: ObjKey, to: ControlKey },
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
                write!(f, "{from} refers to routine {} of {}, which has none", to.local, to.obj)
            }
            LinkError::NoImplementation { from, codeunit, name } => {
                write!(
                    f,
                    "{from} calls `{name}` through an interface, which {codeunit} does not implement"
                )
            }
            LinkError::UnknownControl { from, to } => {
                write!(f, "{from} refers to control {} of {}, which has none", to.name, to.page)
            }
            LinkError::UnknownField { from, to } => {
                write!(f, "{from} refers to field {} of {}, which has none", to.no, to.table)
            }
            LinkError::DuplicateRoutine { object, key } => {
                write!(f, "{object} has two routines with the key {key}")
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

/// What keys can be resolved against: a set of objects, each with its key, the keys of its
/// routines and its object. [`link`] resolves against the objects it is given, [`relink`] against
/// a program that is already linked.
trait Space {
    fn len(&self) -> usize;
    fn key(&self, i: usize) -> &ObjKey;
    fn routines(&self, i: usize) -> &[LocalKey];
    fn object(&self, i: usize) -> &Object;
    /// A table's field numbers, in order, when `object` is only a placeholder.
    fn field_numbers(&self, _i: usize) -> Option<&[u32]> {
        None
    }
    /// A page's control names, in order, when `object` is only a placeholder.
    fn control_names(&self, _i: usize) -> Option<&[String]> {
        None
    }
    /// The interfaces codeunit `i` implements.
    fn implemented(&self, _i: usize) -> &[ObjKey] {
        &[]
    }
    /// The numbers of the fields that table extensions add to table `i`, in order.
    fn extra_field_numbers(&self, _i: usize) -> &[u32] {
        &[]
    }
}

/// The units to link, with the keys of their routines.
struct UnitSpace<'u, 'k> {
    units: &'u [Unit],
    /// Per table: the numbers of the fields its extensions add.
    extras: FxMap<usize, Vec<u32>>,
    routines: &'k (dyn Fn(usize) -> &'k [LocalKey] + Sync),
}

impl Space for UnitSpace<'_, '_> {
    fn len(&self) -> usize {
        self.units.len()
    }
    fn key(&self, i: usize) -> &ObjKey {
        self.units[i].key()
    }
    fn routines(&self, i: usize) -> &[LocalKey] {
        (self.routines)(i)
    }
    fn object(&self, i: usize) -> &Object {
        self.units[i].object()
    }
    fn field_numbers(&self, i: usize) -> Option<&[u32]> {
        match &self.units[i] {
            Unit::Dense(d) if !d.fields.is_empty() => Some(&d.fields),
            Unit::Sym(s) if !s.field_numbers.is_empty() => Some(&s.field_numbers),
            _ => None,
        }
    }
    fn control_names(&self, i: usize) -> Option<&[String]> {
        match &self.units[i] {
            Unit::Dense(d) if !d.controls.is_empty() => Some(&d.controls),
            _ => None,
        }
    }
    fn extra_field_numbers(&self, i: usize) -> &[u32] {
        self.extras.get(&i).map_or(&[], |v| v.as_slice())
    }
    fn implemented(&self, i: usize) -> &[ObjKey] {
        match &self.units[i] {
            Unit::Sym(s) => &s.implements,
            Unit::Dense(d) => &d.implements,
        }
    }
}

/// The numbers of the fields the table extensions of each table add, in program order.
fn extra_fields(units: &[Unit]) -> FxMap<usize, Vec<u32>> {
    let at: FxMap<&ObjKey, usize> = units.iter().enumerate().map(|(i, u)| (u.key(), i)).collect();
    let mut out: FxMap<usize, Vec<u32>> = FxMap::default();
    for u in units {
        let Unit::Sym(s) = u else { continue };
        let (Some(base), Some(t)) = (
            s.extends.as_ref().and_then(|k| at.get(k)).copied(),
            s.contribution.table.as_ref(),
        ) else {
            continue;
        };
        out.entry(base).or_default().extend(t.fields.iter().map(|f| f.id));
    }
    out
}

/// The positions of keys in a [`Space`], found when first asked for.
struct Tables<'a, S: Space + ?Sized> {
    space: &'a S,
    at: FxMap<&'a ObjKey, usize>,
    routine_at: Vec<OnceLock<FxMap<&'a LocalKey, usize>>>,
    field_at: Vec<OnceLock<FxMap<u32, usize>>>,
    control_at: Vec<OnceLock<FxMap<String, usize>>>,
    /// Per interface: the codeunits that implement it, in program order.
    implementers: OnceLock<FxMap<usize, Vec<usize>>>,
}

impl<'a, S: Space + ?Sized> Tables<'a, S> {
    fn new(space: &'a S) -> Result<Tables<'a, S>, Box<LinkError>> {
        let n = space.len();
        let mut at: FxMap<&ObjKey, usize> = FxMap::with_capacity_and_hasher(n, Default::default());
        for i in 0..n {
            if at.insert(space.key(i), i).is_some() {
                return Err(Box::new(LinkError::DuplicateObject(space.key(i).clone())));
            }
        }
        Ok(Tables {
            space,
            at,
            routine_at: (0..n).map(|_| OnceLock::new()).collect(),
            field_at: (0..n).map(|_| OnceLock::new()).collect(),
            control_at: (0..n).map(|_| OnceLock::new()).collect(),
            implementers: OnceLock::new(),
        })
    }

    fn object(&self, from: &ObjKey, key: &ObjKey) -> Result<usize, Box<LinkError>> {
        self.at.get(key).copied().ok_or_else(|| {
            Box::new(LinkError::UnknownObject {
                from: from.clone(),
                to: key.clone(),
            })
        })
    }

    fn routine_table(&self, i: usize) -> Result<&FxMap<&'a LocalKey, usize>, Box<LinkError>> {
        if let Some(t) = self.routine_at[i].get() {
            return Ok(t);
        }
        let keys = self.space.routines(i);
        let mut t = FxMap::with_capacity_and_hasher(keys.len(), Default::default());
        for (r, k) in keys.iter().enumerate() {
            if t.insert(k, r).is_some() {
                return Err(Box::new(LinkError::DuplicateRoutine {
                    object: self.space.key(i).clone(),
                    key: k.clone(),
                }));
            }
        }
        Ok(self.routine_at[i].get_or_init(|| t))
    }

    /// The position of each field number in a table: its place in the table's merged field list.
    fn field_table(&self, i: usize) -> &FxMap<u32, usize> {
        self.field_at[i].get_or_init(|| {
            if let Some(f) = self.space.field_numbers(i) {
                return first_of(f.iter().copied());
            }
            match &self.space.object(i).table {
                Some(t) => first_of(
                    t.fields
                        .iter()
                        .map(|f| f.id)
                        .chain(self.space.extra_field_numbers(i).iter().copied()),
                ),
                None => FxMap::default(),
            }
        })
    }

    /// The position of each control name in a page's control list: the first of a name, as
    /// lowering took it, compared without case.
    fn control_table(&self, i: usize) -> &FxMap<String, usize> {
        self.control_at[i].get_or_init(|| {
            let mut at = FxMap::default();
            if let Some(names) = self.space.control_names(i) {
                for (pos, c) in names.iter().enumerate() {
                    at.entry(c.clone()).or_insert(pos);
                }
            } else if let Some(p) = &self.space.object(i).page {
                for (pos, c) in p.controls.iter().enumerate() {
                    at.entry(c.name.to_ascii_lowercase()).or_insert(pos);
                }
            }
            at
        })
    }

    /// The codeunits that implement interface `iface`, in program order.
    fn implementers(&self, iface: usize) -> &[usize] {
        let all = self.implementers.get_or_init(|| {
            let mut m: FxMap<usize, Vec<usize>> = FxMap::default();
            for i in 0..self.space.len() {
                for k in self.space.implemented(i) {
                    if let Some(&j) = self.at.get(k) {
                        m.entry(j).or_default().push(i);
                    }
                }
            }
            m
        });
        all.get(&iface).map_or(&[], |v| v.as_slice())
    }

    /// The set an expansion stands for.
    fn expand(&self, from: &ObjKey, e: &Expansion) -> Result<Expanded, Box<LinkError>> {
        match e {
            Expansion::InterfaceTest { interface } => {
                let i = self.object(from, interface)?;
                Ok(Expanded::Test(self.implementers(i).iter().map(|&c| c as u32).collect()))
            }
            Expansion::InterfaceCall {
                interface,
                name,
                params,
            } => {
                let i = self.object(from, interface)?;
                let mut targets = Vec::new();
                for &cu in self.implementers(i) {
                    let shape = |k: &LocalKey| {
                        !k.local
                            && !k.trigger
                            && k.name == *name
                            && k.params.len() == params.len()
                            && k.params
                                .iter()
                                .zip(params)
                                .all(|(a, b)| a.starts_with("var ") == b.starts_with("var "))
                    };
                    let keys = self.space.routines(cu);
                    let same: Vec<usize> = (0..keys.len()).filter(|&r| shape(&keys[r])).collect();
                    let exact = same.iter().copied().find(|&r| keys[r].params == *params);
                    let Some(r) = exact.or(same.first().copied()) else {
                        return Err(Box::new(LinkError::NoImplementation {
                            from: from.clone(),
                            codeunit: self.space.key(cu).clone(),
                            name: name.clone(),
                        }));
                    };
                    targets.push((cu, r));
                }
                Ok(Expanded::Call(targets))
            }
            Expansion::EnumImpl { .. } => Err(Box::new(LinkError::BadReference {
                object: from.clone(),
                class: "enum implementation",
                index: 0,
            })),
        }
    }

    /// What the tables of symbolic object `s` mean here: the position of every key.
    fn resolve(&self, s: &SymObject) -> Result<Resolved, Box<LinkError>> {
        let mut objects = Vec::with_capacity(s.refs.objects.len());
        for k in &s.refs.objects {
            objects.push(self.object(&s.key, k)?);
        }
        let mut routines = Vec::with_capacity(s.refs.routines.len());
        for k in &s.refs.routines {
            let o = self.object(&s.key, &k.obj)?;
            let r = *self.routine_table(o)?.get(&k.local).ok_or_else(|| {
                Box::new(LinkError::UnknownRoutine {
                    from: s.key.clone(),
                    to: k.clone(),
                })
            })?;
            routines.push(r);
        }
        let mut fields = Vec::with_capacity(s.refs.fields.len());
        for k in &s.refs.fields {
            let o = self.object(&s.key, &k.table)?;
            // A query's column is named by its position, which is the number.
            let pos = if k.table.kind == al_ir::ObjKind::Query {
                Some(k.no as usize)
            } else {
                self.field_table(o).get(&k.no).copied()
            };
            fields.push(pos.ok_or_else(|| {
                Box::new(LinkError::UnknownField {
                    from: s.key.clone(),
                    to: k.clone(),
                })
            })?);
        }
        let mut controls = Vec::with_capacity(s.refs.controls.len());
        for k in &s.refs.controls {
            let o = self.object(&s.key, &k.page)?;
            controls.push(self.control_table(o).get(&k.name).copied().ok_or_else(|| {
                Box::new(LinkError::UnknownControl {
                    from: s.key.clone(),
                    to: k.clone(),
                })
            })?);
        }
        let mut expanded = Vec::with_capacity(s.refs.expansions.len());
        for e in &s.refs.expansions {
            expanded.push(self.expand(&s.key, e)?);
        }
        Ok(Resolved {
            objects,
            routines,
            fields,
            controls,
            expanded,
        })
    }
}

/// Rewrites `object`, whose references are indices into the tables of a symbolic object, with
/// what they resolved to.
fn rewrite(object: &mut Object, res: &Resolved, key: &ObjKey) -> Result<(), Box<LinkError>> {
    let mut w = Rewrite { res, bad: None };
    walk_object(object, &mut w);
    match w.bad {
        Some((class, index)) => Err(Box::new(LinkError::BadReference {
            object: key.clone(),
            class,
            index,
        })),
        None => Ok(()),
    }
}

/// Links `units` into `base`, which holds everything that is not an object (`apps`, `dotnet_types`,
/// `sources`, `uncompiled_subscribers`; its `objects` and `positions` are empty). The objects come
/// out in the order of `units`.
///
/// `routines` gives the keys of the routines of object `i`, asked for the objects that other objects
/// call.
pub fn link<'k>(
    units: Vec<Unit>,
    mut base: Program,
    workers: usize,
    routines: &'k (dyn Fn(usize) -> &'k [LocalKey] + Sync),
) -> Result<Linked, Box<LinkError>> {
    debug_assert!(base.objects.is_empty() && base.positions.is_empty());
    // What each symbolic object's tables mean in the program: the position of every key.
    let merges;
    let resolved: Vec<Option<Resolved>> = {
        let space = UnitSpace {
            units: &units,
            extras: extra_fields(&units),
            routines,
        };
        let tables = Tables::new(&space)?;
        merges = Merges::collect(&units, &tables);
        par_map(units.iter().collect(), workers, |_, u| match u {
            Unit::Sym(s) => tables.resolve(s).map(Some),
            Unit::Dense(_) => Ok(None),
        })
        .into_iter()
        .collect::<Result<_, _>>()?
    };
    // Where each object's source positions go.
    let mut pos_ranges = Vec::with_capacity(units.len());
    let mut next = 0u32;
    for u in &units {
        let len = u.position_count();
        pos_ranges.push((next, len));
        next += len;
    }
    let mut units = units;
    let contributions: Vec<Contribution> = units
        .iter_mut()
        .map(|u| match u {
            Unit::Sym(s) => std::mem::take(&mut s.contribution),
            Unit::Dense(_) => Contribution::default(),
        })
        .collect();
    let work: Vec<(Unit, Option<Resolved>, Contribution)> = units
        .into_iter()
        .zip(resolved)
        .zip(contributions)
        .map(|((u, r), c)| (u, r, c))
        .collect();
    let done = par_map(
        work,
        workers,
        |i, (u, res, mut contribution)| -> Result<Done, Box<LinkError>> {
            let (mut object, mut positions, key) = match u {
                Unit::Sym(s) => (s.object, s.positions, s.key),
                Unit::Dense(d) => (d.object, d.positions, d.key),
            };
            if let Some(res) = res {
                rewrite(&mut object, &res, &key)?;
                if let Some(t) = contribution.table.as_mut() {
                    let mut w = Rewrite { res: &res, bad: None };
                    walk_table_contribution(t, &mut w);
                    if let Some((class, index)) = w.bad {
                        return Err(Box::new(LinkError::BadReference {
                            object: key,
                            class,
                            index,
                        }));
                    }
                }
            }
            if !positions.is_empty() {
                for r in &mut object.routines {
                    r.shift_positions(pos_ranges[i].0);
                }
                for p in &mut positions {
                    p.object = i as u32;
                }
            }
            Ok((object, positions, contribution))
        },
    );
    base.objects.reserve(done.len());
    base.positions.reserve(next as usize);
    let mut contributions = Vec::with_capacity(done.len());
    for d in done {
        let (object, mut positions, contribution) = d?;
        base.positions.append(&mut positions);
        base.objects.push(object);
        contributions.push(contribution);
    }
    merges.apply(&mut base.objects, &mut contributions);
    Ok(Linked {
        program: base,
        pos_ranges,
    })
}

/// An object linked, the source positions it holds, and what it adds to its base.
type Done = (Object, Vec<SrcPos>, Contribution);

/// How many objects a worker takes at a time.
const CHUNK: usize = 8;
/// The stack of a worker: the walk recurses on the nesting of the code, as lowering does.
const STACK: usize = 64 << 20;

/// `f` applied to every item, on `workers` threads (the calling thread waits), results in the order of the items.
pub fn par_map<T: Send, R: Send>(items: Vec<T>, workers: usize, f: impl Fn(usize, T) -> R + Sync) -> Vec<R> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = items.len();
    let workers = workers.min(n / CHUNK).max(1);
    if workers == 1 {
        return items.into_iter().enumerate().map(|(i, x)| f(i, x)).collect();
    }
    let slots: Vec<Mutex<Option<T>>> = items.into_iter().map(|x| Mutex::new(Some(x))).collect();
    let out: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(STACK)
                    .spawn_scoped(scope, || {
                        loop {
                            let k = next.fetch_add(CHUNK, Ordering::Relaxed);
                            if k >= n {
                                return;
                            }
                            for i in k..(k + CHUNK).min(n) {
                                let item = slots[i].lock().expect("a worker panicked").take().expect("taken once");
                                let r = f(i, item);
                                *out[i].lock().expect("a worker panicked") = Some(r);
                            }
                        }
                    })
                    .expect("spawn a link worker")
            })
            .collect();
        for h in handles {
            h.join().expect("a link worker panicked");
        }
    });
    out.into_iter()
        .map(|m| m.into_inner().expect("a worker panicked").expect("every item is done"))
        .collect()
}

/// What the extension objects add to their bases (ADR-033 decision 3), found before the objects are
/// consumed and applied after they are linked. Only a base that is symbolic takes it: an object a
/// compile cache supplied already holds its lists.
/// (base, list, order, extension)
type ListEntry = (usize, ListKind, (u32, u32, u32), usize);

#[derive(Default)]
struct Merges {
    /// (base, list, order, extension)
    lists: Vec<ListEntry>,
    /// (base, extension) of the extensions with values, in program order.
    enums: Vec<(usize, usize)>,
    /// (base, extension) of the table extensions that add fields, keys or changes.
    tables: Vec<(usize, usize)>,
    symbolic: Vec<bool>,
}

impl Merges {
    fn collect<S: Space + ?Sized>(units: &[Unit], tables: &Tables<'_, S>) -> Merges {
        let mut m = Merges {
            symbolic: units.iter().map(|u| matches!(u, Unit::Sym(_))).collect(),
            ..Default::default()
        };
        for (i, u) in units.iter().enumerate() {
            let Unit::Sym(s) = u else { continue };
            let Some(base) = s.extends.as_ref().and_then(|k| tables.at.get(k)).copied() else {
                continue;
            };
            for j in &s.joins {
                m.lists.push((base, j.list, j.order, i));
            }
            if !s.contribution.enum_values.is_empty() || !s.contribution.enum_captions.is_empty() {
                m.enums.push((base, i));
            }
            if s.contribution.table.is_some() {
                m.tables.push((base, i));
            }
        }
        m.lists.sort();
        m
    }

    fn apply(&self, objects: &mut [Object], contributions: &mut [Contribution]) {
        for &(base, list, _, ext) in &self.lists {
            if !self.symbolic[base] || objects[base].uncompiled {
                continue;
            }
            let o = &mut objects[base];
            match list {
                ListKind::TableExtensions => {
                    if let Some(t) = o.table.as_mut() {
                        t.extensions.push(ext);
                    }
                }
                ListKind::PageExtensions => {
                    if let Some(p) = o.page.as_mut() {
                        p.extensions.push(ext);
                    }
                }
                ListKind::ReportExtensions => {
                    if let Some(r) = o.report.as_mut() {
                        r.extensions.push(ext);
                    }
                }
            }
        }
        self.apply_tables(objects, contributions);
        for &(base, ext) in &self.enums {
            if self.symbolic[base] && !objects[base].uncompiled {
                let c = &contributions[ext];
                objects[base].enum_values.extend(c.enum_values.iter().cloned());
                objects[base].enum_captions.extend(c.enum_captions.iter().cloned());
            }
        }
    }
}

impl Merges {
    /// Merges what table extensions add into their tables the way the members of a table are merged:
    /// the fields of each extension, then its `modify` blocks (the first field of the name takes the
    /// properties), then the triggers every field of the name takes, and the keys.
    fn apply_tables(&self, objects: &mut [Object], contributions: &mut [Contribution]) {
        let mut at = 0;
        while at < self.tables.len() {
            let base = self.tables[at].0;
            let mut end = at;
            while end < self.tables.len() && self.tables[end].0 == base {
                end += 1;
            }
            let exts: Vec<usize> = self.tables[at..end].iter().map(|&(_, e)| e).collect();
            at = end;
            if !self.symbolic[base] || objects[base].uncompiled {
                continue;
            }
            let Some(table) = objects[base].table.as_mut() else {
                continue;
            };
            let adds: Vec<al_ir::sym::TableContribution> =
                exts.iter().filter_map(|&e| contributions[e].table.take()).collect();
            for c in &adds {
                table.fields.extend(c.fields.iter().cloned());
                for m in &c.modifies {
                    if let Some(f) = table.fields.iter_mut().find(|f| f.name.eq_ignore_ascii_case(&m.field)) {
                        for (k, v) in &m.properties {
                            match f.properties.iter_mut().find(|(pk, _)| pk.eq_ignore_ascii_case(k)) {
                                Some(slot) => slot.1 = v.clone(),
                                None => f.properties.push((k.clone(), v.clone())),
                            }
                        }
                    }
                }
            }
            for c in &adds {
                for m in &c.modifies {
                    for f in table
                        .fields
                        .iter_mut()
                        .filter(|f| f.name.eq_ignore_ascii_case(&m.field))
                    {
                        f.ext_before_validate.extend(m.before_validate.iter().copied());
                        f.ext_after_validate.extend(m.after_validate.iter().copied());
                    }
                }
                for k in &c.keys {
                    table.keys.push(k.fields.clone());
                    table.unique_keys.push(k.unique);
                    table.key_props.push(k.props.clone());
                }
            }
        }
    }
}

/// The objects of a linked program, as a [`Space`]: `keys` and `routines` say what the program
/// does not (the key of each object, the keys of its routines).
struct ProgramSpace<'p, 'k> {
    program: &'p Program,
    keys: &'k [ObjKey],
    routines: &'k dyn Fn(usize) -> &'k [LocalKey],
}

impl Space for ProgramSpace<'_, '_> {
    fn len(&self) -> usize {
        self.program.objects.len()
    }
    fn key(&self, i: usize) -> &ObjKey {
        &self.keys[i]
    }
    fn routines(&self, i: usize) -> &[LocalKey] {
        (self.routines)(i)
    }
    fn object(&self, i: usize) -> &Object {
        &self.program.objects[i]
    }
}

/// Puts new versions of some objects into a program that is already linked: the incremental
/// compile (ADR-029). `replace` holds the symbolic objects, by their position. Their keys are
/// resolved against the program as it is, so the declarations of every object (its key, its
/// routines, its fields and controls) must be as they were. The source positions of the program
/// are renumbered in object order, the unchanged objects moved to their new place, as [`link`] would
/// number them. `pos_ranges` is where each object's positions are (start, count), as in [`Linked`].
pub fn relink<'k>(
    program: &mut Program,
    pos_ranges: &mut [(u32, u32)],
    keys: &'k [ObjKey],
    routines: &'k dyn Fn(usize) -> &'k [LocalKey],
    replace: Vec<(usize, SymObject)>,
) -> Result<(), Box<LinkError>> {
    let resolved: Vec<Resolved> = {
        let space = ProgramSpace {
            program,
            keys,
            routines,
        };
        let tables = Tables::new(&space)?;
        replace
            .iter()
            .map(|(_, s)| tables.resolve(s))
            .collect::<Result<_, _>>()?
    };
    let mut renewed: Vec<Option<(SymObject, Resolved)>> = (0..program.objects.len()).map(|_| None).collect();
    for ((oi, s), res) in replace.into_iter().zip(resolved) {
        renewed[oi] = Some((s, res));
    }
    let old_positions = std::mem::take(&mut program.positions);
    let mut positions: Vec<SrcPos> = Vec::with_capacity(old_positions.len());
    for (oi, slot) in renewed.into_iter().enumerate() {
        let cursor = positions.len() as u32;
        match slot {
            Some((s, res)) => {
                let SymObject {
                    key,
                    mut object,
                    positions: mut own,
                    ..
                } = s;
                rewrite(&mut object, &res, &key)?;
                // What the extensions add to the object is as it was: the declarations did not change.
                let old = &program.objects[oi];
                object.enum_values = old.enum_values.clone();
                object.enum_captions = old.enum_captions.clone();
                if let (Some(n), Some(o)) = (object.table.as_mut(), old.table.as_ref()) {
                    n.extensions = o.extensions.clone();
                }
                if let (Some(n), Some(o)) = (object.page.as_mut(), old.page.as_ref()) {
                    n.extensions = o.extensions.clone();
                }
                if let (Some(n), Some(o)) = (object.report.as_mut(), old.report.as_ref()) {
                    n.extensions = o.extensions.clone();
                }
                let len = own.len() as u32;
                for r in &mut object.routines {
                    r.shift_positions(cursor);
                }
                for p in &mut own {
                    p.object = oi as u32;
                }
                positions.append(&mut own);
                program.objects[oi] = object;
                pos_ranges[oi] = (cursor, len);
            }
            None => {
                let (start, len) = pos_ranges[oi];
                if len > 0 {
                    positions.extend_from_slice(&old_positions[start as usize..(start + len) as usize]);
                    if start != cursor {
                        let by = cursor.wrapping_sub(start);
                        for r in &mut program.objects[oi].routines {
                            r.shift_positions(by);
                        }
                    }
                }
                pos_ranges[oi] = (cursor, len);
            }
        }
    }
    program.positions = positions;
    Ok(())
}

/// The positions the keys of one symbolic object's tables resolved to.
struct Resolved {
    objects: Vec<usize>,
    routines: Vec<usize>,
    fields: Vec<usize>,
    controls: Vec<usize>,
    expanded: Vec<Expanded>,
}

/// A set that an expansion stood for.
enum Expanded {
    Call(Vec<(usize, usize)>),
    Test(Vec<u32>),
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

    fn expansion(&mut self, index: usize, site: ExpansionSite<'_>) {
        match (self.res.expanded.get(index), site) {
            (Some(Expanded::Call(t)), ExpansionSite::InterfaceCall { targets }) => *targets = t.clone(),
            (Some(Expanded::Test(o)), ExpansionSite::InterfaceTest { objects }) => *objects = o.clone(),
            _ => self.bad = self.bad.or(Some(("expansion", index))),
        }
    }

    fn control(&mut self, r: &mut usize) {
        match self.res.controls.get(*r) {
            Some(&p) => *r = p,
            None => self.bad = self.bad.or(Some(("control", *r))),
        }
    }
}

/// The position of each field number. A merged field list can hold a number twice (an app that
/// extends a table redeclares a field); the lowering took the first, so a key does too.
fn first_of(ids: impl Iterator<Item = u32>) -> FxMap<u32, usize> {
    let mut at = FxMap::default();
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

    fn keys_of(units: &[Unit]) -> Vec<Vec<LocalKey>> {
        units
            .iter()
            .map(|u| match u {
                Unit::Sym(s) => s.routines.clone(),
                Unit::Dense(_) => vec![local("first", 0), local("second", 0)],
            })
            .collect()
    }

    /// `link` with the routine keys the symbolic objects carry (a dense unit in these tests has `first` and `second`).
    fn link_all(units: Vec<Unit>, workers: usize) -> Result<Linked, Box<LinkError>> {
        let keys = keys_of(&units);
        let routines = |i: usize| keys[i].as_slice();
        link(units, Program::default(), workers, &routines)
    }

    fn local(name: &str, ordinal: u32) -> LocalKey {
        LocalKey {
            name: name.into(),
            section: String::new(),
            params: Vec::new(),
            local: false,
            trigger: false,
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
            field_numbers: vec![],
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
            ..Default::default()
        }
    }

    fn callee(id: u32) -> SymObject {
        SymObject {
            key: key(id),
            routines: vec![local("first", 0), local("second", 0)],
            refs: Refs::default(),
            field_numbers: vec![],
            object: object(id, vec![routine("First", vec![]), routine("Second", vec![])]),
            positions: vec![],
            ..Default::default()
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
        let a = link_all(vec![Unit::Sym(caller(1, 2)), Unit::Sym(callee(2))], 1).unwrap();
        assert_eq!(call_of(&a.program, 0), (1, 1));
        let b = link_all(
            vec![Unit::Sym(callee(2)), Unit::Sym(callee(3)), Unit::Sym(caller(1, 2))],
            1,
        )
        .unwrap();
        assert_eq!(call_of(&b.program, 2), (0, 1));
    }

    #[test]
    fn a_dependency_object_placed_as_it_is_can_still_be_called_by_key() {
        let dense = DenseObject {
            key: key(2),
            implements: vec![],
            fields: vec![],
            controls: vec![],
            object: object(2, vec![routine("First", vec![]), routine("Second", vec![])]),
            positions: vec![],
        };
        let linked = link_all(vec![Unit::Dense(dense), Unit::Sym(caller(1, 2))], 1).unwrap();
        assert_eq!(call_of(&linked.program, 1), (0, 1));
    }

    #[test]
    fn a_missing_target_is_an_error_that_names_it() {
        let e = link_all(vec![Unit::Sym(caller(1, 2))], 1).err().unwrap();
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
        let e = link_all(vec![Unit::Sym(caller(1, 2)), Unit::Sym(c)], 1).err().unwrap();
        assert!(matches!(*e, LinkError::UnknownRoutine { .. }), "{e:?}");
    }

    #[test]
    fn two_objects_with_one_key_do_not_link() {
        let e = link_all(vec![Unit::Sym(callee(2)), Unit::Sym(callee(2))], 1)
            .err()
            .unwrap();
        assert_eq!(*e, LinkError::DuplicateObject(key(2)));
    }

    #[test]
    fn a_reference_outside_the_table_is_an_error() {
        let mut c = caller(1, 2);
        c.refs.objects.clear();
        let e = link_all(vec![Unit::Sym(c), Unit::Sym(callee(2))], 1).err().unwrap();
        assert!(matches!(*e, LinkError::BadReference { class: "object", .. }), "{e:?}");
    }

    fn enum_unit(id: u32, kind: ObjKind, values: &[(i32, &str)], extends: Option<u32>) -> Unit {
        let vals: Vec<(i32, String)> = values.iter().map(|(v, n)| (*v, n.to_string())).collect();
        let mut s = SymObject {
            key: ObjKey {
                app: "app".into(),
                kind,
                name: ObjName::Id(id),
            },
            object: Object {
                kind,
                id: Some(id),
                ..Default::default()
            },
            extends: extends.map(|b| ObjKey {
                app: "app".into(),
                kind: ObjKind::Enum,
                name: ObjName::Id(b),
            }),
            ..Default::default()
        };
        if extends.is_some() {
            s.contribution.enum_values = vals;
        } else {
            s.object.enum_values = vals;
        }
        Unit::Sym(s)
    }

    #[test]
    fn extensions_add_their_enum_values_to_the_base_in_program_order() {
        let units = vec![
            enum_unit(10, ObjKind::EnumExtension, &[(5, "Five")], Some(1)),
            enum_unit(1, ObjKind::Enum, &[(0, "Zero"), (1, "One")], None),
            enum_unit(11, ObjKind::EnumExtension, &[(6, "Six")], Some(1)),
        ];
        let linked = link_all(units, 1).unwrap();
        let names: Vec<&str> = linked.program.objects[1]
            .enum_values
            .iter()
            .map(|(_, n)| n.as_str())
            .collect();
        assert_eq!(names, ["Zero", "One", "Five", "Six"]);
        assert!(linked.program.objects[0].enum_values.is_empty());
    }

    fn field(id: u32, name: &str) -> al_ir::Field {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "ty": "Integer", "stored": true, "init": null, "init_text": null,
            "relation_table": null, "relation_tables": [], "auto_increment": false, "relation": false,
            "on_validate": null, "ext_before_validate": [], "ext_after_validate": [], "on_lookup": null,
            "properties": [["Caption", "'Old'"]], "relation_arms": [], "relation_meta": [], "relation_runtime": [],
            "flow_filter": false, "calc": null, "calc_refusal": null, "declared_in": null
        }))
        .unwrap()
    }

    #[test]
    fn a_table_extension_adds_fields_keys_and_changes_in_program_order() {
        use al_ir::sym::{FieldModify, KeyDecl, TableContribution};
        let key = |id, kind| ObjKey {
            app: "app".into(),
            kind,
            name: ObjName::Id(id),
        };
        let table = SymObject {
            key: key(1, ObjKind::Table),
            object: Object {
                kind: ObjKind::Table,
                id: Some(1),
                table: Some(al_ir::Table {
                    fields: vec![field(1, "A")],
                    keys: vec![vec![0]],
                    unique_keys: vec![true],
                    key_props: vec![vec![]],
                    per_company: true,
                    extensions: vec![],
                }),
                ..Default::default()
            },
            refs: Refs {
                fields: vec![al_ir::sym::FieldKey {
                    table: key(1, ObjKind::Table),
                    no: 1,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let ext = |id: u32, field_id: u32, name: &str, key_of: Option<u32>| {
            let mut s = SymObject {
                key: key(id, ObjKind::TableExtension),
                extends: Some(key(1, ObjKind::Table)),
                object: Object {
                    kind: ObjKind::TableExtension,
                    id: Some(id),
                    ..Default::default()
                },
                refs: Refs {
                    fields: vec![
                        al_ir::sym::FieldKey {
                            table: key(1, ObjKind::Table),
                            no: field_id,
                        },
                        al_ir::sym::FieldKey {
                            table: key(1, ObjKind::Table),
                            no: 1,
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            };
            s.contribution.table = Some(TableContribution {
                fields: vec![field(field_id, name)],
                keys: key_of
                    .map(|_| KeyDecl {
                        fields: vec![0, 1],
                        unique: false,
                        props: vec![],
                    })
                    .into_iter()
                    .collect(),
                modifies: vec![FieldModify {
                    field: "a".into(),
                    properties: vec![("Caption".into(), "'New'".into())],
                    before_validate: vec![],
                    after_validate: vec![],
                }],
            });
            Unit::Sym(s)
        };
        let units = vec![ext(11, 20, "B", None), Unit::Sym(table), ext(12, 30, "C", Some(1))];
        let linked = link_all(units, 1).unwrap();
        let t = linked.program.objects[1].table.as_ref().unwrap();
        let ids: Vec<u32> = t.fields.iter().map(|f| f.id).collect();
        assert_eq!(ids, [1, 20, 30]);
        assert_eq!(t.fields[0].properties, [("Caption".to_string(), "'New'".to_string())]);
        assert_eq!(t.keys, [vec![0], vec![2, 0]]);
        assert_eq!(t.unique_keys, [true, false]);
    }

    #[test]
    fn par_map_keeps_the_order_of_the_items_on_any_number_of_workers() {
        let items: Vec<usize> = (0..1000).collect();
        for workers in [1, 2, 7] {
            let out = par_map(items.clone(), workers, |i, x| (i, x * 2));
            assert!(
                out.iter().enumerate().all(|(i, &(j, v))| i == j && v == i * 2),
                "{workers}"
            );
        }
        assert!(par_map(Vec::<u8>::new(), 4, |_, x| x).is_empty());
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
        let linked = link_all(vec![Unit::Sym(a), Unit::Sym(b)], 1).unwrap();
        assert_eq!(linked.pos_ranges, vec![(0, 2), (2, 1)]);
        assert_eq!(linked.program.objects[1].routines[1].pos, 2);
        assert_eq!(linked.program.positions[2].line, 7);
        assert_eq!(linked.program.positions[2].object, 1);
    }
}
