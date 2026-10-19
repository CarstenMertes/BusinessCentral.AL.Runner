//! `Engine::compiled_codeunit` and `Engine::codeunit_object`: the lookups of a codeunit by id that `Codeunit.Run(Id)` and
//! the test runner's steps around each test method make, answered from an index built with the engine instead of a scan
//! of every object of the program on each call. They give what the scan gave: the first object in program order.

use al_ir::{ObjKind, Object, Program};
use al_vm::Engine;

fn object(kind: ObjKind, id: Option<u32>, uncompiled: bool) -> Object {
    Object {
        kind,
        id,
        uncompiled,
        ..Default::default()
    }
}

#[test]
fn a_codeunit_is_found_by_id_as_the_first_object_in_program_order() {
    let objects = vec![
        object(ObjKind::Codeunit, Some(7), true),
        object(ObjKind::Codeunit, Some(7), false),
        object(ObjKind::Codeunit, Some(7), false),
        object(ObjKind::Table, Some(8), false),
        object(ObjKind::Codeunit, Some(9), true),
        object(ObjKind::Page, Some(10), false),
        object(ObjKind::Codeunit, Some(10), false),
        object(ObjKind::Codeunit, None, false),
    ];
    let scan = |objects: &[Object], id: Option<u32>, shells: bool| {
        objects
            .iter()
            .position(|o| (o.is_kind(ObjKind::Codeunit) || shells && o.is_shell_of(ObjKind::Codeunit)) && o.id == id)
    };
    let n = objects.len();
    let engine = Engine::new(
        Program {
            objects: objects.clone(),
            ..Default::default()
        },
        vec![Vec::new(); n],
    );
    for id in [Some(7), Some(8), Some(9), Some(10), Some(11), None] {
        assert_eq!(
            engine.compiled_codeunit(id),
            scan(&objects, id, false),
            "compiled {id:?}"
        );
        assert_eq!(engine.codeunit_object(id), scan(&objects, id, true), "any {id:?}");
    }
    // A shell first: the compiled codeunit after it is the one that runs.
    assert_eq!(engine.compiled_codeunit(Some(7)), Some(1));
    assert_eq!(engine.codeunit_object(Some(7)), Some(0));
    // Only a shell: no compiled codeunit, but the object exists.
    assert_eq!(engine.compiled_codeunit(Some(9)), None);
    assert_eq!(engine.codeunit_object(Some(9)), Some(4));
    // Another kind with the id is not a codeunit.
    assert_eq!(engine.compiled_codeunit(Some(8)), None);
    assert_eq!(engine.compiled_codeunit(Some(10)), Some(6));
}
