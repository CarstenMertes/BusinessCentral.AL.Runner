//! Event dispatch: publisher calls reach subscribers; manual subscriptions
//! are bound to codeunit instances (`BindSubscription`).
//!
//! Semantics, with the corpus tests that pin them:
//! - Subscriber parameters are matched to the publisher's by name; a
//!   subscriber may declare any subset (codeunit/TestCodeunitEventSenderPosition.al:
//!   a trailing parameter may be omitted), and with `IncludeSender` the
//!   one parameter that matches no publisher parameter is the sender, at any
//!   position (same file, SenderFirst/Middle/Last).
//! - `var` parameters write back to the publisher (codeunit/TestEventContracts.al:12,
//!   record/TestTableEventByRefMutation.al:23).
//! - An error in a subscriber reaches the publisher's caller
//!   (codeunit/TestEventUnderscoreName.al, UnderscoredEvent_SubscriberError_Propagates).
//! - A `Manual` subscriber fires only on instances bound by `BindSubscription`
//!   (codeunit/TestEventManualBinding.al, Contracts 1-3), once per bound
//!   instance (Contract 7), on the bound instance itself (Contract 2). A
//!   binding lasts until `UnbindSubscription` or until the last reference to
//!   the instance is gone (TestEventManualBinding.al Contract 10,
//!   TestEventManualBindingByValue.al); a `SingleInstance` codeunit is its
//!   session-wide instance, so it is never gone.
//! - Isolated events: each subscriber is its own transaction when nothing is
//!   pending (codeunit/TestIsolatedEventSubscriberError.al).

use crate::Session;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, Value};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

type Inst = Rc<RefCell<Instance>>;

/// An event being raised: which one, and what the publisher offers its subscribers.
pub(crate) struct EventCall<'a> {
    /// The object subscribers name (the base table for events of a table extension).
    pub object: usize,
    /// Lower-case event name.
    pub event: &'a str,
    /// Filters field events by field name.
    pub element: Option<&'a str>,
    pub isolated: bool,
    /// The publisher's parameter names, matching `args` of `raise`.
    pub names: &'a [String],
    /// The publishing instance, for `IncludeSender` events.
    pub sender: Option<Value>,
}

/// One manual subscription: a codeunit instance whose manual subscribers receive events.
pub(crate) struct Binding {
    pub instance: Weak<RefCell<Instance>>,
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

impl Session {
    /// Runs a publisher routine: no body, only its subscribers. Returns the
    /// parameters as the subscribers left them (for `var` write-back).
    pub(crate) fn publish(
        &mut self,
        object: usize,
        routine: usize,
        instance: Inst,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        let engine = self.engine.clone();
        let p = &engine.events.publishers[&(object, routine)];
        let r = &engine.program.objects[object].routines[routine];
        let mut values = Vec::with_capacity(r.params);
        for (i, a) in args.into_iter().enumerate().take(r.params) {
            values.push(match (a, r.by_var.get(i).copied().unwrap_or(false)) {
                (Value::Record(rc), false) => self.record_by_value(&rc),
                (a, _) => ops::convert(a, &r.locals[i].ty)?,
            });
        }
        let sender = if p.include_sender {
            let o = &engine.program.objects[object];
            match (o.kind.as_str(), o.rec_slot) {
                ("codeunit", _) => Some(Value::Codeunit(instance)),
                (_, Some(slot)) => Some(instance.borrow().globals[slot].clone()),
                _ => return unsupported(format!("{}: IncludeSender on a {}", r.name, o.kind)),
            }
        } else {
            None
        };
        let names: Vec<String> = r.locals[..r.params].iter().map(|s| s.name.clone()).collect();
        let call = EventCall {
            object: p.object,
            event: &p.event,
            element: None,
            isolated: p.isolated,
            names: &names,
            sender,
        };
        if let Err(e) = self.raise(&call, &mut values) {
            // What earlier subscribers wrote back is what the publisher's caller sees.
            if matches!(e, Signal::Error(_)) {
                self.error_finals = Some(values);
            }
            return Err(e);
        }
        Ok((Value::Null, values))
    }

    /// Delivers the event to its subscribers. `var` parameters of subscribers are written
    /// back into `args`. Isolated events: see below.
    pub(crate) fn raise(&mut self, ev: &EventCall, args: &mut [Value]) -> Result<(), Signal> {
        let EventCall {
            object,
            event,
            element,
            isolated,
            names,
            sender,
        } = ev;
        let (object, event, element, isolated) = (*object, *event, *element, *isolated);
        let engine = self.engine.clone();
        for (s, inst) in self.event_targets(object, event, element)? {
            let r = &engine.program.objects[s.object].routines[s.routine];
            let mut map: Vec<Option<usize>> = Vec::with_capacity(r.params);
            let mut sender_taken = false;
            for slot in &r.locals[..r.params] {
                if let Some(j) = names.iter().position(|n| n.eq_ignore_ascii_case(&slot.name)) {
                    map.push(Some(j));
                } else if sender.is_some() && !sender_taken {
                    sender_taken = true;
                    map.push(None);
                } else {
                    return unsupported(format!(
                        "subscriber {}.{}: parameter `{}` matches no parameter of event `{event}`",
                        engine.program.objects[s.object].name, r.name, slot.name
                    ));
                }
            }
            let call_args = map
                .iter()
                .map(|m| match m {
                    Some(j) => args[*j].clone(),
                    None => sender.clone().expect("sender checked"),
                })
                .collect();
            // An Isolated event runs each subscriber in its own transaction: with nothing pending
            // in the caller's, a subscriber's error is swallowed and only its writes are rolled
            // back; with a write pending the event is not isolated and the error propagates
            // (codeunit/TestIsolatedEventSubscriberError.al). Whether the error text is kept as
            // the last error is not pinned.
            let isolate = isolated && !self.store.has_uncommitted();
            let before = isolate.then(|| self.store.clone());
            let (_, finals) = match self.call(s.object, s.routine, inst, call_args) {
                Err(Signal::Error(_)) if isolate => {
                    self.store = before.expect("snapshot taken when isolating");
                    continue;
                }
                r => r?,
            };
            for (i, m) in map.iter().enumerate() {
                if let (Some(j), true) = (m, r.by_var[i]) {
                    args[*j] = finals[i].clone();
                }
            }
        }
        Ok(())
    }

    /// The subscribers an event reaches right now, each with the instance it
    /// runs on: static subscribers always, manual ones once per bound instance.
    /// `element` filters field events by field name.
    fn event_targets(
        &mut self,
        object: usize,
        event: &str,
        element: Option<&str>,
    ) -> Result<Vec<(crate::events::Subscriber, Inst)>, Signal> {
        let engine = self.engine.clone();
        let Some(subs) = engine.events.subscribers.get(&(object, event.to_string())) else {
            return Ok(Vec::new());
        };
        let mut targets = Vec::new();
        for s in subs {
            if object == engine.events.global_key && engine.events.unavailable_global.contains(&(s.object, s.routine)) {
                continue;
            }
            if let Some(el) = element {
                if !s.element.eq_ignore_ascii_case(el) {
                    continue;
                }
            }
            if s.manual {
                for inst in self.bound_instances(s.object) {
                    targets.push((s.clone(), inst));
                }
            } else {
                targets.push((s.clone(), self.static_instance(s.object)?));
            }
        }
        Ok(targets)
    }

    /// Whether raising `event` of `object` now would reach any subscriber.
    pub(crate) fn has_listeners(&mut self, object: usize, event: &str, element: Option<&str>) -> Result<bool, Signal> {
        if !self.engine.events.is_subscribed(object, event) {
            return Ok(false);
        }
        Ok(!self.event_targets(object, event, element)?.is_empty())
    }

    /// The instance a static (automatic) subscriber runs on: the session's one
    /// instance for `SingleInstance` codeunits. Other codeunits would get a
    /// fresh instance per event, which only matters if they keep global state;
    /// that is not pinned by the corpus, so such subscribers are refused.
    fn static_instance(&mut self, object: usize) -> Result<Inst, Signal> {
        let o = &self.engine.program.objects[object];
        let single = o
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SingleInstance") && v.trim().eq_ignore_ascii_case("true"));
        if !single && !o.globals.is_empty() {
            return unsupported(format!(
                "subscriber codeunit {} keeps global variables: the lifetime of a static subscriber instance is not pinned",
                o.name
            ));
        }
        let fresh = Rc::new(RefCell::new(Instance {
            object: object as u32,
            ..Default::default()
        }));
        Ok(self.shared_instance(fresh))
    }

    /// Live bound instances of codeunit `object`, oldest binding first.
    fn bound_instances(&mut self, object: usize) -> Vec<Inst> {
        self.bindings.retain(|b| b.instance.strong_count() > 0);
        self.bindings
            .iter()
            .filter_map(|b| b.instance.upgrade())
            .filter(|i| i.borrow().object as usize == object)
            .collect()
    }

    /// `BindSubscription(Codeunit)` (`bind`) and `UnbindSubscription(Codeunit)`.
    pub(crate) fn subscription(&mut self, bind: bool, cu: Value, ret_used: bool) -> Result<Value, Signal> {
        let Value::Codeunit(rc) = cu else {
            return unsupported(format!("BindSubscription of {cu:?}"));
        };
        let inst = self.shared_instance(rc);
        let object = inst.borrow().object as usize;
        let manual = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("EventSubscriberInstance") && v.trim().eq_ignore_ascii_case("Manual"));
        self.bindings.retain(|b| b.instance.strong_count() > 0);
        let at = self
            .bindings
            .iter()
            .position(|b| b.instance.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)));
        if bind {
            // Not Manual: false, no error (codeunit/TestEventManualBinding.al Contract 5, the
            // Boolean form only; the statement form is not pinned).
            if !manual {
                return if ret_used {
                    Ok(Value::Boolean(false))
                } else {
                    unsupported(
                        "BindSubscription as a statement on a codeunit that is not EventSubscriberInstance = Manual",
                    )
                };
            }
            if at.is_some() {
                // Boolean form: false (Contract 4); statement form: an error (Contract 8).
                return if ret_used {
                    Ok(Value::Boolean(false))
                } else {
                    Err(Signal::Error(AlError {
                        message: format!(
                            "The instance of codeunit {} has already been bound.",
                            self.engine.program.objects[object].name
                        ),
                        code: String::new(),
                        call_stack: String::new(),
                    }))
                };
            }
            self.ensure_initialized(&inst);
            self.bindings.push(Binding {
                instance: Rc::downgrade(&inst),
            });
            Ok(Value::Boolean(true))
        } else if manual {
            // True even when it was never bound (Contract 6, "container-verified").
            if let Some(i) = at {
                self.bindings.remove(i);
            }
            Ok(Value::Boolean(true))
        } else if ret_used {
            // The corpus pins only that it does not raise (TestEventContracts.al Contract 6).
            Ok(Value::Boolean(false))
        } else {
            Ok(Value::Null)
        }
    }
}
