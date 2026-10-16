//! From the object lowering wrote (references are positions in the program) to the symbolic
//! object link takes (references are indices into the object's own tables of stable keys,
//! ADR-033).
//!
//! The keys of an object's routines come from its syntax, not from its lowered form: a routine
//! that nothing reached is lowered to a stub without parameters, and its key must not change
//! because of that.

use al_ir as ir;
use al_ir::sym::{FieldKey, LocalKey, ObjKey, Refs, RoutineKey, SymObject};
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
            for c in t.syntax().text().to_string().chars() {
                if !c.is_whitespace() {
                    out.extend(c.to_lowercase());
                }
            }
        }
    }
    out
}

/// The fields the expressions of an object name, as (table, field number), in the order lowering
/// first named them. An expression holds the index into this list.
#[derive(Default)]
pub(crate) struct FieldRefs {
    pub(crate) list: Vec<(usize, u32)>,
    ix: HashMap<(usize, u32), usize>,
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

/// What the symbolic form of each object is made from: the key of every object and of its routines,
/// in program order, and what the lowering knew about tables.
pub(crate) struct Keys<'a> {
    pub objects: &'a [ObjKey],
    pub routines: &'a [Vec<LocalKey>],
    /// Per object: the table whose record it works on (`Shared::table_of`).
    pub table_of: &'a [Option<usize>],
    /// The number of field `pos` of table `table`.
    pub field_no: &'a dyn Fn(usize, usize) -> Option<u32>,
    /// The table (object) of data item `item` of report `report`.
    pub report_item_table: &'a dyn Fn(usize, usize) -> Option<usize>,
}

/// Makes the symbolic object of a lowered one. `Err` names a reference that points outside the
/// program (a bug of the lowering).
pub(crate) fn localize(
    keys: &Keys<'_>,
    own: usize,
    mut object: ir::Object,
    positions: Vec<ir::SrcPos>,
    field_refs: &[(usize, u32)],
) -> Result<SymObject, String> {
    // The expressions' own references come first: they are already indices into this list.
    let mut refs = Refs::default();
    let mut fields: HashMap<FieldKey, usize> = HashMap::new();
    for &(table, id) in field_refs {
        let key = FieldKey {
            table: keys.objects[table].clone(),
            no: id,
        };
        fields.entry(key.clone()).or_insert(refs.fields.len());
        refs.fields.push(key);
    }
    let mut v = Localize {
        keys,
        own,
        refs,
        objects: HashMap::new(),
        routines: HashMap::new(),
        fields,
        bad: None,
    };
    walk_object(&mut object, &mut v);
    if let Some(b) = v.bad {
        return Err(format!("{}: {b}", keys.objects[own]));
    }
    Ok(SymObject {
        key: keys.objects[own].clone(),
        routines: keys.routines[own].clone(),
        refs: v.refs,
        object,
        positions,
    })
}

struct Localize<'a> {
    keys: &'a Keys<'a>,
    own: usize,
    refs: Refs,
    objects: HashMap<usize, usize>,
    routines: HashMap<(usize, usize), usize>,
    fields: HashMap<FieldKey, usize>,
    bad: Option<String>,
}

impl Localize<'_> {
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

impl RefVisitor for Localize<'_> {
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
        let Some(local) = self.keys.routines.get(object).and_then(|t| t.get(*r)) else {
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
                .get_or_insert_with(|| format!("field position {} has no table ({table:?})", *r));
            return;
        };
        let Some(no) = (self.keys.field_no)(t, *r) else {
            self.bad
                .get_or_insert_with(|| format!("field position {} is not a field of object {t}", *r));
            return;
        };
        let key = FieldKey {
            table: self.keys.objects[t].clone(),
            no,
        };
        let refs = &mut self.refs;
        *r = *self.fields.entry(key.clone()).or_insert_with(|| {
            refs.fields.push(key);
            refs.fields.len() - 1
        });
    }
}
