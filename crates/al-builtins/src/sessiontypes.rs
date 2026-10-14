//! `SessionSettings` and `Notification`: in-memory value objects (corpus session/TestSessionSettings.al,
//! handlers/TestNotificationHandler.al, session/TestCalcDateNotificationContracts.al,
//! session/TestMiscTypes.al).

use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, NotificationState, SessionSettingsState, Value};
use std::rc::Rc;

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn with_settings<R>(r: Option<&mut Value>, f: impl FnOnce(&mut SessionSettingsState) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("SessionSettings method without a receiver");
    };
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::SessionSettings(SessionSettingsState::default());
    }
    match &mut *lib {
        LibraryValue::SessionSettings(s) => Ok(f(s)),
        other => unsupported(format!("SessionSettings method on {other:?}")),
    }
}

fn with_notification<R>(r: Option<&mut Value>, f: impl FnOnce(&mut NotificationState) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("Notification method without a receiver");
    };
    let mut lib = cell.borrow_mut();
    if matches!(&*lib, LibraryValue::Empty) {
        *lib = LibraryValue::Notification(NotificationState::default());
    }
    match &mut *lib {
        LibraryValue::Notification(s) => Ok(f(s)),
        other => unsupported(format!("Notification method on {other:?}")),
    }
}

fn text(a: &[Value], i: usize) -> Result<String, Signal> {
    match a.get(i).and_then(crate::native::text_of) {
        Some(t) => Ok(t),
        None => unsupported(format!("a text argument, got {:?}", a.get(i))),
    }
}

fn int(a: &[Value], i: usize) -> Result<i32, Signal> {
    match a.get(i) {
        Some(Value::Integer(n)) => Ok(*n),
        other => unsupported(format!("an Integer argument, got {other:?}")),
    }
}

/// Property accessors: with an argument they set, without one they get. A property nobody wrote
/// is refused on read (see [`SessionSettingsState`]).
fn language_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| s.language_id)?.map(Value::Integer).ok_or_else(|| {
            Signal::Unsupported("LanguageId of settings nobody set (the default is not pinned)".into())
        });
    }
    let n = int(a, 0)?;
    with_settings(r, |s| s.language_id = Some(n)).map(|()| Value::Null)
}

fn locale_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| s.locale_id)?
            .map(Value::Integer)
            .ok_or_else(|| Signal::Unsupported("LocaleId of settings nobody set (the default is not pinned)".into()));
    }
    let n = int(a, 0)?;
    with_settings(r, |s| s.locale_id = Some(n)).map(|()| Value::Null)
}

fn time_zone(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| s.time_zone.clone())?
            .map(Value::Text)
            .ok_or_else(|| Signal::Unsupported("TimeZone of settings nobody set (the default is not pinned)".into()));
    }
    let t = text(a, 0)?;
    with_settings(r, |s| s.time_zone = Some(t)).map(|()| Value::Null)
}

fn profile_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| s.profile_id.clone())?
            .map(Value::Text)
            .ok_or_else(|| Signal::Unsupported("ProfileId of settings nobody set (the default is not pinned)".into()));
    }
    let t = text(a, 0)?;
    with_settings(r, |s| s.profile_id = Some(t)).map(|()| Value::Null)
}

fn profile_app_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| s.profile_app_id)?.map(Value::Guid).ok_or_else(|| {
            Signal::Unsupported("ProfileAppId of settings nobody set (the default is not pinned)".into())
        });
    }
    let Value::Guid(g) = a[0] else {
        return unsupported(format!("ProfileAppId with {:?}", a[0]));
    };
    with_settings(r, |s| s.profile_app_id = Some(g)).map(|()| Value::Null)
}

/// An untouched instance is the empty company (session/TestSessionSettings.al:575-595).
fn company(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_settings(r, |s| Value::Text(s.company.clone()));
    }
    let t = text(a, 0)?;
    with_settings(r, |s| s.company = t).map(|()| Value::Null)
}

/// System profiles are deprecated: the setter discards its argument and the property stays false
/// (session/TestSessionSettings.al:172-215).
fn profile_system_scope(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        Ok(Value::Boolean(false))
    } else {
        Ok(Value::Null)
    }
}

/// `Init()` loads the language, locale and time zone of the running session and replaces what
/// the instance held (session/TestSessionSettings.al:371-455). The company stays empty, as BC 27.5
/// leaves it (the same test, "MEASURED"). The profile properties are not pinned: they go back to
/// "nobody set", which a read refuses.
fn init(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let Some((language, locale, zone)) = c.session_locale() else {
        return unsupported(
            "SessionSettings.Init reads the language, locale and time zone of the running session, which this context does not have (host value, ADR-013)",
        );
    };
    with_settings(r, |s| {
        *s = SessionSettingsState {
            language_id: Some(language),
            locale_id: Some(locale),
            time_zone: Some(zone),
            ..SessionSettingsState::default()
        }
    })
    .map(|()| Value::Null)
}

/// `RequestSessionUpdate(false)` asks the client for a new session and is routed to the test's
/// `[SessionSettingsHandler]`, which gets the assigned settings; the instance stays as it was and
/// so does the running session (session/TestSessionSettings.al:464-545). `true` also writes the
/// settings to "User Personalization", which the corpus does not exercise: refused.
fn request_update(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        Some(Value::Boolean(false)) => {}
        Some(Value::Boolean(true)) => {
            return unsupported(
                "SessionSettings.RequestSessionUpdate(true) writes User Personalization (not pinned by the corpus)",
            );
        }
        other => return unsupported(format!("SessionSettings.RequestSessionUpdate with {other:?}")),
    }
    let state = with_settings(r, |s| s.clone())?;
    let copy = Value::Library(
        "SessionSettings".into(),
        std::rc::Rc::new(std::cell::RefCell::new(LibraryValue::SessionSettings(state))),
    );
    c.ui_session_settings(copy).map(|()| Value::Null)
}

// ---- Notification ------------------------------------------------------------------------

/// `Message(text)` sets, `Message()` reads (handlers/TestNotificationHandler.al:28-35). Reading a
/// message nobody set is not pinned.
fn n_message(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_notification(r, |n| n.message.clone())?
            .map(Value::Text)
            .ok_or_else(|| {
                Signal::Unsupported("Notification.Message of a notification nobody gave one (not pinned)".into())
            });
    }
    let t = text(a, 0)?;
    with_notification(r, |n| n.message = Some(t)).map(|()| Value::Null)
}

/// `Id := guid` sets, `Id` reads (handlers/TestNotificationHandler.al:55-56). Reading the id of a
/// notification nobody gave one is not pinned.
fn n_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        Some(Value::Guid(g)) => {
            let g = *g;
            with_notification(r, |n| n.id = Some(g)).map(|()| Value::Null)
        }
        None => with_notification(r, |n| n.id)?.map(Value::Guid).ok_or_else(|| {
            Signal::Unsupported("Notification.Id of a notification nobody gave one (not pinned)".into())
        }),
        Some(other) => unsupported(format!("Notification.Id with {other:?}")),
    }
}

/// Setting a scope must not throw (session/TestCalcDateNotificationContracts.al:182-188); reading one
/// back is not pinned.
fn n_scope(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    match a.first() {
        Some(Value::Option(o, _)) => {
            let o = *o;
            with_notification(r, |n| n.scope = Some(o)).map(|()| Value::Null)
        }
        None => with_notification(r, |n| n.scope)?
            .map(|o| {
                let members: Rc<[String]> = crate::sysenum::option_members("NotificationScope")
                    .expect("a system option")
                    .into();
                Value::Option(o, members)
            })
            .ok_or_else(|| {
                Signal::Unsupported("Notification.Scope of a notification nobody set one on (not pinned)".into())
            }),
        Some(other) => unsupported(format!("Notification.Scope with {other:?}")),
    }
}

/// Data is a text map: a second `SetData` of a name replaces its value
/// (session/TestCalcDateNotificationContracts.al:142-180).
fn n_set_data(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (k, v) = (text(a, 0)?, text(a, 1)?);
    with_notification(r, |n| match n.data.iter_mut().find(|(name, _)| *name == k) {
        Some(slot) => slot.1 = v,
        None => n.data.push((k, v)),
    })
    .map(|()| Value::Null)
}

fn n_has_data(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let k = text(a, 0)?;
    with_notification(r, |n| Value::Boolean(n.data.iter().any(|(name, _)| *name == k)))
}

/// Reading a name nobody set is not pinned.
fn n_get_data(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let k = text(a, 0)?;
    with_notification(r, |n| {
        n.data.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone())
    })?
    .map(Value::Text)
    .ok_or_else(|| Signal::Unsupported("Notification.GetData of a name nobody set (not pinned)".into()))
}

/// Stored, never run: an action only matters when the notification is sent to a client
/// (session/TestSystemExtended.al:553-575, TestMiscComplete.al:106-119 pin that it does not throw).
fn n_add_action(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (caption, codeunit, method) = (text(a, 0)?, int(a, 1)?, text(a, 2)?);
    let description = if a.len() > 3 { Some(text(a, 3)?) } else { None };
    with_notification(r, |n| n.actions.push((caption, codeunit, method, description))).map(|()| Value::Null)
}

/// A copy of the notification for a handler, which gets its own (handlers/TestNotificationHandler.al:
/// 74-79 read the message from the parameter).
fn notification_copy(r: Option<&mut Value>) -> Result<(Value, Option<[u8; 16]>), Signal> {
    let state = with_notification(r, |n| n.clone())?;
    let id = state.id;
    Ok((
        Value::Library(
            "Notification".into(),
            std::rc::Rc::new(std::cell::RefCell::new(LibraryValue::Notification(state))),
        ),
        id,
    ))
}

/// `Send()` goes to the test's `[SendNotificationHandler]` (handlers/TestNotificationHandler.al:15-26).
fn n_send(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let (copy, id) = notification_copy(r)?;
    c.ui_send_notification(copy, id).map(|()| Value::Null)
}

/// `Recall()` after a handled `Send` does not throw with only a send handler bound
/// (handlers/TestNotificationHandler.al:60-72).
fn n_recall(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let id = with_notification(r, |n| n.id)?;
    c.ui_recall_notification(id).map(|()| Value::Null)
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    Some(match (sig.receiver.as_str(), sig.name.as_str()) {
        ("SessionSettings", "LanguageId") => language_id,
        ("SessionSettings", "LocaleId") => locale_id,
        ("SessionSettings", "TimeZone") => time_zone,
        ("SessionSettings", "ProfileId") => profile_id,
        ("SessionSettings", "ProfileAppId") => profile_app_id,
        ("SessionSettings", "Company") => company,
        ("SessionSettings", "ProfileSystemScope") => profile_system_scope,
        ("SessionSettings", "Init") => init,
        ("SessionSettings", "RequestSessionUpdate") => request_update,
        ("Notification", "Message") => n_message,
        ("Notification", "Scope") => n_scope,
        ("Notification", "Id") => n_id,
        ("Notification", "SetData") => n_set_data,
        ("Notification", "HasData") => n_has_data,
        ("Notification", "GetData") => n_get_data,
        ("Notification", "AddAction") => n_add_action,
        ("Notification", "Send") => n_send,
        ("Notification", "Recall") => n_recall,
        _ => return None,
    })
}
