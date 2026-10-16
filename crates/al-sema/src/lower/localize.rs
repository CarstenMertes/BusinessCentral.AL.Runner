//! From the object lowering wrote (references are positions in the program) to the symbolic
//! object link takes (references are indices into the object's own tables of stable keys,
//! ADR-033).
//!
//! The keys of an object's routines come from its syntax, not from its lowered form: a routine
//! that nothing reached is lowered to a stub without parameters, and its key must not change
//! because of that.

use al_ir as ir;
use al_ir::sym::{ControlKey, FieldKey, LocalKey, ObjKey, Refs, RoutineKey, SymObject};
use al_ir::walk::{RefVisitor, TableOf, walk_object};
use al_syntax::ast;
use std::collections::HashMap;

/// The routine keys of an object, one per routine node in the order the object numbers them.
pub(crate) fn routine_keys(nodes: &[ast::Routine]) -> Vec<LocalKey> {
    let mut seen: HashMap<(String, String, String), u32> = HashMap::new();
    nodes
        .iter()
        .map(|r| {
            let name = r.name().to_lowercase();
            let section = section_of(r);
            let sig = signature_of(r);
            let slot = seen.entry((name.clone(), section.clone(), sig.clone())).or_insert(0);
            let ordinal = *slot;
            *slot += 1;
            LocalKey {
                name,
                section,
                sig,
                ordinal,
            }
        })
        .collect()
}

/// The routine keys of the objects of a program, made from the syntax of an object when another
/// object first asks for them. An object a compile cache supplies has its keys in the cache.
pub(crate) struct RoutineKeys<'a> {
    ws: &'a crate::workspace::Workspace,
    /// Per object: the keys the cache supplies.
    supplied: Vec<Option<&'a [LocalKey]>>,
    /// Per object: whether it is compiled (a shell has no routines).
    compiled: Vec<bool>,
    cells: Vec<std::sync::OnceLock<Vec<LocalKey>>>,
}

impl<'a> RoutineKeys<'a> {
    pub(crate) fn new(
        ws: &'a crate::workspace::Workspace,
        supplied: Vec<Option<&'a [LocalKey]>>,
        compiled: Vec<bool>,
    ) -> RoutineKeys<'a> {
        let n = compiled.len();
        RoutineKeys {
            ws,
            supplied,
            compiled,
            cells: (0..n).map(|_| std::sync::OnceLock::new()).collect(),
        }
    }

    /// The keys of the routines of object `i`, in the order the object numbers them.
    pub(crate) fn of(&self, i: usize) -> &[LocalKey] {
        if let Some(k) = self.supplied[i] {
            return k;
        }
        self.cells[i].get_or_init(|| {
            if self.compiled[i] {
                routine_keys(&ast::routines_in(self.ws.object_ast(i).syntax()))
            } else {
                Vec::new()
            }
        })
    }
}

/// The nearest enclosing section of a trigger (`field 7`, `action Post`), as
/// `ir::Routine::container` holds it, in lower case. Empty for a procedure or a trigger of the
/// object itself.
fn section_of(r: &ast::Routine) -> String {
    match r.syntax().ancestors().find_map(ast::Section::cast) {
        Some(sec) => {
            let arg = sec.args().first().cloned().unwrap_or_default();
            format!(
                "{} {}",
                sec.name().to_ascii_lowercase(),
                arg.trim().trim_matches('"').to_lowercase()
            )
        }
        None => String::new(),
    }
}

/// The parameter types as written, lower case, without spaces, joined by `,`.
fn signature_of(r: &ast::Routine) -> String {
    let mut out = String::new();
    for (i, p) in r.params().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if let Some(t) = p.type_ref() {
            t.syntax().text().for_each_chunk(|chunk| {
                for c in chunk.chars() {
                    if !c.is_whitespace() {
                        if c.is_ascii() {
                            out.push(c.to_ascii_lowercase());
                        } else {
                            out.extend(c.to_lowercase());
                        }
                    }
                }
            });
        }
    }
    out
}

/// The fields the expressions of an object name, as (table, field number), in the order lowering
/// first named them. An expression holds the index into this list.
#[derive(Default)]
pub(crate) struct FieldRefs {
    pub(crate) list: Vec<(usize, u32)>,
    ix: FxMap<(usize, u32), usize>,
}

impl FieldRefs {
    pub(crate) fn intern(&mut self, table: usize, id: u32) -> usize {
        let list = &mut self.list;
        *self.ix.entry((table, id)).or_insert_with(|| {
            list.push((table, id));
            list.len() - 1
        })
    }
}

/// The controls the expressions of an object name: (page, control name in lower case, the control's
/// position in the page's layout, which the check against the old numbering reads). An expression
/// holds the index into this list.
#[derive(Default)]
pub(crate) struct ControlRefs {
    pub(crate) list: Vec<(usize, String, usize)>,
    ix: FxMap<(usize, String), usize>,
}

impl ControlRefs {
    pub(crate) fn intern(&mut self, page: usize, name: &str, pos: usize) -> usize {
        let name = name.to_ascii_lowercase();
        let list = &mut self.list;
        *self.ix.entry((page, name.clone())).or_insert_with(|| {
            list.push((page, name, pos));
            list.len() - 1
        })
    }
}

use al_ir::sym::FxMap;

/// What the symbolic form of each object is made from: the key of every object and of its routines,
/// in program order, and what the lowering knew about tables.
pub(crate) struct Keys<'a> {
    pub objects: &'a [ObjKey],
    /// The key of each routine of object `i`.
    pub routines: &'a (dyn Fn(usize) -> &'a [LocalKey] + Sync),
    /// Per object: the table whose record it works on (`Shared::table_of`).
    pub table_of: &'a [Option<usize>],
    /// The number of field `pos` of table `table`.
    pub field_no: &'a (dyn Fn(usize, usize) -> Option<u32> + Sync),
    /// The table (object) of data item `item` of report `report`.
    pub report_item_table: &'a (dyn Fn(usize, usize) -> Option<usize> + Sync),
}

/// Makes the symbolic object of a lowered one. `Err` names a reference that points outside the
/// program (a bug of the lowering).
pub(crate) fn localize(
    keys: &Keys<'_>,
    own: usize,
    mut object: ir::Object,
    positions: Vec<ir::SrcPos>,
    field_refs: &[(usize, u32)],
    control_refs: &[(usize, String, usize)],
) -> Result<SymObject, String> {
    // The expressions' own references come first: they are already indices into this list.
    let mut refs = Refs::default();
    let mut fields: FxMap<(usize, u32), usize> = FxMap::default();
    for &(table, id) in field_refs {
        fields.entry((table, id)).or_insert(refs.fields.len());
        refs.fields.push(FieldKey {
            table: keys.objects[table].clone(),
            no: id,
        });
    }
    for (page, name, _) in control_refs {
        refs.controls.push(ControlKey {
            page: keys.objects[*page].clone(),
            name: name.clone(),
        });
    }
    let mut v = Localize {
        keys,
        own,
        refs,
        objects: FxMap::default(),
        routines: FxMap::default(),
        fields,
        bad: None,
    };
    walk_object(&mut object, &mut v);
    if let Some(b) = v.bad {
        return Err(format!("{}: {b}", keys.objects[own]));
    }
    Ok(SymObject {
        key: keys.objects[own].clone(),
        routines: Vec::new(),
        refs: v.refs,
        field_numbers: Vec::new(),
        object,
        positions,
    })
}

struct Localize<'a, 'k> {
    keys: &'a Keys<'k>,
    own: usize,
    refs: Refs,
    objects: FxMap<usize, usize>,
    routines: FxMap<(usize, usize), usize>,
    fields: FxMap<(usize, u32), usize>,
    bad: Option<String>,
}

impl Localize<'_, '_> {
    /// The table object a field reference is a field of, in workspace numbering.
    fn table_object(&self, table: TableOf) -> Option<usize> {
        match table {
            TableOf::Own => Some(self.own),
            TableOf::Trigger | TableOf::PageSource => self.keys.table_of.get(self.own).copied().flatten(),
            TableOf::Object(t) => Some(t),
            TableOf::SourceOf(p) => self.keys.table_of.get(p).copied().flatten(),
            TableOf::BaseItem { report, item } => (self.keys.report_item_table)(report, item),
            TableOf::Embedded | TableOf::Unknown => None,
        }
    }
}

impl RefVisitor for Localize<'_, '_> {
    fn object(&mut self, r: &mut usize) {
        let at = *r;
        let Some(key) = self.keys.objects.get(at) else {
            self.bad
                .get_or_insert_with(|| format!("object reference {at} is outside the program"));
            return;
        };
        let refs = &mut self.refs;
        *r = *self.objects.entry(at).or_insert_with(|| {
            refs.objects.push(key.clone());
            refs.objects.len() - 1
        });
    }

    fn routine(&mut self, r: &mut usize, object: usize) {
        let at = (object, *r);
        let known = object < self.keys.objects.len();
        let Some(local) = known.then(|| (self.keys.routines)(object)).and_then(|t| t.get(*r)) else {
            self.bad
                .get_or_insert_with(|| format!("routine reference {} of object {object} does not exist", at.1));
            return;
        };
        let refs = &mut self.refs;
        let obj = &self.keys.objects[object];
        *r = *self.routines.entry(at).or_insert_with(|| {
            refs.routines.push(RoutineKey {
                obj: obj.clone(),
                local: local.clone(),
            });
            refs.routines.len() - 1
        });
    }

    fn control(&mut self, r: &mut usize) {
        if *r >= self.refs.controls.len() {
            self.bad
                .get_or_insert_with(|| format!("control reference {} is outside the object's list", *r));
        }
    }

    fn field(&mut self, r: &mut usize, table: TableOf) {
        if table == TableOf::Embedded {
            if *r >= self.refs.fields.len() {
                self.bad
                    .get_or_insert_with(|| format!("field reference {} is outside the object's list", *r));
            }
            return;
        }
        let Some(t) = self.table_object(table) else {
            self.bad
                .get_or_insert_with(|| format!("field position {} has no table ({})", *r, table.describe()));
            return;
        };
        let Some(no) = (self.keys.field_no)(t, *r) else {
            self.bad
                .get_or_insert_with(|| format!("field position {} is not a field of object {t}", *r));
            return;
        };
        let refs = &mut self.refs;
        let keys = self.keys;
        *r = *self.fields.entry((t, no)).or_insert_with(|| {
            refs.fields.push(FieldKey {
                table: keys.objects[t].clone(),
                no,
            });
            refs.fields.len() - 1
        });
    }
}
