//! `Media` and `MediaSet` fields (corpus media/).
//!
//! A Media field holds the id of its media object; a MediaSet field holds the id of its set, and
//! the set holds the ids of its media objects. Both are stored as a Guid, null for an empty field
//! (media/TestMediaEmptyMediaId.al). The media objects and the sets live in the session's
//! `MediaStore`, the stand-in for the platform tables "Tenant Media" and "Tenant Media Set"
//! (ADR-013 gives those tables rows). The field only holds the id once the record is written,
//! so an import that was not followed by `Modify` is not seen by the next `Get`.

use crate::native::{Ctx, NativeFn, Signal};
use crate::value::{LibraryValue, Value};
use std::collections::HashMap;

pub type Id = [u8; 16];

/// One imported media object.
#[derive(Debug, Clone)]
pub struct MediaObject {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub description: String,
}

/// The session's media: objects by id and the members of each set by set id.
#[derive(Debug, Default)]
pub struct MediaStore {
    pub objects: HashMap<Id, MediaObject>,
    pub sets: HashMap<Id, Vec<Id>>,
}

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn id_of(r: &Option<&mut Value>) -> Result<Id, Signal> {
    match r {
        Some(Value::Guid(g)) => Ok(*g),
        other => unsupported(format!("Media method on {other:?}")),
    }
}

fn store(c: &mut dyn Ctx) -> Result<&mut MediaStore, Signal> {
    match c.media_store() {
        Some(s) => Ok(s),
        None => unsupported("media in a context without a media store"),
    }
}

fn text_arg(a: &[Value], i: usize) -> Result<String, Signal> {
    match a.get(i).and_then(crate::native::text_of) {
        Some(t) => Ok(t),
        None => unsupported(format!("a text argument, got {:?}", a.get(i))),
    }
}

/// Reads the rest of an `InStream` (the bytes it has not handed out yet).
fn drain(v: &Value) -> Result<Vec<u8>, Signal> {
    let Value::Library(_, h) = v else {
        return unsupported(format!("an InStream was expected, got {v:?}"));
    };
    match &mut *h.borrow_mut() {
        LibraryValue::InStream(s) => {
            let rest = s.data[s.pos.min(s.data.len())..].to_vec();
            s.pos = s.data.len();
            Ok(rest)
        }
        other => unsupported(format!("an InStream was expected, got {other:?}")),
    }
}

fn import_object(c: &mut dyn Ctx, a: &[Value]) -> Result<Id, Signal> {
    // ImportStream(Stream, Description[, MimeType]).
    let bytes = drain(
        a.first()
            .ok_or_else(|| Signal::Unsupported("ImportStream without a stream".into()))?,
    )?;
    let description = text_arg(a, 1)?;
    let mime = match a.get(2) {
        None | Some(Value::Null) => String::new(),
        Some(_) => text_arg(a, 2)?,
    };
    let id = c.new_guid();
    store(c)?.objects.insert(
        id,
        MediaObject {
            bytes,
            mime,
            description,
        },
    );
    Ok(id)
}

fn has_value(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Boolean(id_of(&r)? != [0; 16]))
}

fn media_id(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Guid(id_of(&r)?))
}

/// The importing methods fill an empty field only. Replacing or extending what a field already
/// holds is not pinned, so it is refused.
fn media_import(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if id_of(&r)? != [0; 16] {
        return unsupported("Media.ImportStream into a field that already has a value (not pinned)");
    }
    let id = import_object(c, a)?;
    if let Some(slot) = r {
        *slot = Value::Guid(id);
    }
    Ok(Value::Guid(id))
}

fn set_import(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if id_of(&r)? != [0; 16] {
        return unsupported("MediaSet.ImportStream into a set that already has members (not pinned)");
    }
    // The set's own id is what the call returns and what MediaId answers; the member has another
    // (media/TestMediaSetPersistence.al:127-155).
    let member = import_object(c, a)?;
    let set = c.new_guid();
    store(c)?.sets.insert(set, vec![member]);
    if let Some(slot) = r {
        *slot = Value::Guid(set);
    }
    Ok(Value::Guid(set))
}

fn media_export(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let id = id_of(&r)?;
    if id == [0; 16] {
        return unsupported("Media.ExportStream of an empty field (not pinned)");
    }
    let Some(obj) = store(c)?.objects.get(&id).cloned() else {
        return unsupported("Media.ExportStream of a media the session does not hold");
    };
    let Some(Value::Library(_, h)) = a.first() else {
        return unsupported("Media.ExportStream without an OutStream");
    };
    match &*h.borrow() {
        LibraryValue::OutStream(blob, _) => {
            let mut b = blob.borrow_mut();
            b.bytes.extend_from_slice(&obj.bytes);
            b.loaded = true;
        }
        other => return unsupported(format!("an OutStream was expected, got {other:?}")),
    }
    // The bytes are the ones imported. BC re-encodes an image, so the length differs from BC's;
    // the corpus asserts only what any encoder preserves (media/TestMediaPngImport.al:99-168).
    Ok(Value::Boolean(true))
}

fn set_count(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let id = id_of(&r)?;
    if id == [0; 16] {
        return Ok(Value::Integer(0));
    }
    let n = store(c)?.sets.get(&id).map_or(0, Vec::len);
    Ok(Value::Integer(n as i32))
}

fn set_item(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let id = id_of(&r)?;
    let Some(Value::Integer(i)) = a.first() else {
        return unsupported("MediaSet.Item with this index");
    };
    let item = usize::try_from(*i - 1)
        .ok()
        .and_then(|i| store(c).ok()?.sets.get(&id)?.get(i).copied());
    match item {
        Some(m) => Ok(Value::Guid(m)),
        None => unsupported(format!("MediaSet.Item({i}) outside the set (error text not pinned)")),
    }
}

fn set_remove(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let id = id_of(&r)?;
    let Some(Value::Guid(m)) = a.first() else {
        return unsupported("MediaSet.Remove with this argument");
    };
    let Some(members) = store(c)?.sets.get_mut(&id) else {
        return Ok(Value::Boolean(false));
    };
    let before = members.len();
    members.retain(|x| x != m);
    Ok(Value::Boolean(members.len() != before))
}

pub const REGISTRY: &[(&str, NativeFn)] = &[
    ("Media.HasValue#1", has_value),
    ("Media.MediaId#1", media_id),
    ("Media.ImportStream#1", media_import),
    ("Media.ExportStream#1", media_export),
    ("MediaSet.MediaId#1", media_id),
    ("MediaSet.Count#1", set_count),
    ("MediaSet.Item#1", set_item),
    ("MediaSet.ImportStream#1", set_import),
    ("MediaSet.Remove#1", set_remove),
];
