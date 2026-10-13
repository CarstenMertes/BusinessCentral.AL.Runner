//! `RecordRef`, `FieldRef` and `KeyRef` methods. They need the table schema
//! and the store, so every implementation forwards its overload id to the VM
//! through [`Ctx::ref_op`]; the VM resolves the id to the method through the
//! catalog. Overloads not listed here are not implemented: the lowerer
//! refuses a call to them (hard rule 4).

use crate::native::{Ctx, NativeFn};

macro_rules! forward {
    ($($id:literal),* $(,)?) => {
        &[$(($id, (|c: &mut dyn Ctx, r, a| c.ref_op($id, r, a)) as NativeFn)),*]
    };
}

pub const REGISTRY: &[(&str, NativeFn)] = forward![
    // RecordRef: the id numbers follow the catalog order, see `tests` below.
    "RecordRef.Open#2",
    "RecordRef.Close#1",
    "RecordRef.Number#1",
    "RecordRef.Name#1",
    "RecordRef.IsEmpty#1",
    "RecordRef.Count#1",
    "RecordRef.Field#2",
    "RecordRef.FieldCount#1",
    "RecordRef.FieldExist#2",
    "RecordRef.FieldIndex#1",
    "RecordRef.Init#1",
    "RecordRef.Insert#1",
    "RecordRef.Insert#2",
    "RecordRef.Modify#1",
    "RecordRef.Delete#1",
    "RecordRef.DeleteAll#1",
    "RecordRef.Find#1",
    "RecordRef.FindFirst#1",
    "RecordRef.FindLast#1",
    "RecordRef.FindSet#1",
    "RecordRef.FindSet#2",
    "RecordRef.Next#1",
    "RecordRef.Reset#1",
    "RecordRef.Rename#1",
    "RecordRef.GetTable#1",
    "RecordRef.SetTable#1",
    "RecordRef.KeyCount#1",
    "RecordRef.KeyIndex#1",
    // KeyRef
    "KeyRef.Active#1",
    "KeyRef.FieldCount#1",
    "KeyRef.FieldIndex#1",
    // FieldRef
    "FieldRef.Active#1",
    "FieldRef.Length#1",
    "FieldRef.Name#1",
    "FieldRef.Number#1",
    "FieldRef.SetRange#1",
    "FieldRef.SetFilter#1",
    "FieldRef.Value#1",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    /// An id that drifts from the catalog (a corpus bump reorders overloads)
    /// must not silently bind to another method: each id's method name is
    /// the text in the id, and its first parameter is what this table says.
    #[test]
    fn ids_name_the_methods_and_overloads_they_claim() {
        let cat = Catalog::load();
        let params = |id: &str| {
            let s = cat.by_id(id).unwrap_or_else(|| panic!("{id} is not in the catalog"));
            let (recv, name) = id.rsplit_once('#').unwrap().0.split_once('.').unwrap();
            assert!(
                s.receiver == recv && s.name == name,
                "{id} is {}.{}",
                s.receiver,
                s.name
            );
            s.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>()
        };
        assert_eq!(params("RecordRef.Open#2")[0], "Integer");
        assert_eq!(params("RecordRef.Field#2")[0], "Integer");
        assert_eq!(params("RecordRef.FieldExist#2")[0], "Integer");
        assert!(params("RecordRef.Insert#1").is_empty());
        assert_eq!(params("RecordRef.Insert#2"), ["Boolean"]);
        assert!(params("RecordRef.FindSet#1").len() == 1 && params("RecordRef.FindSet#2").len() == 2);
        assert_eq!(params("RecordRef.GetTable#1"), ["Record"]);
        assert_eq!(params("RecordRef.SetTable#1"), ["Record"]);
        for (id, _) in REGISTRY {
            params(id);
        }
    }
}
