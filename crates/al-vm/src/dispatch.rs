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
use al_builtins::describe::Describe;
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

/// The error of a failed `BindSubscription` statement, code `EventBinding`.
fn binding_error(id: u32, why: &str) -> Signal {
    Signal::Error(Box::new(AlError {
        message: format!("The binding of codeunit {id} was unsuccessful. {why}"),
        code: "EventBinding".into(),
        call_stack: String::new(),
    }))
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
            match (o.kind, o.rec_slot) {
                (al_ir::ObjKind::Codeunit, _) => Some(Value::Codeunit(instance)),
                // The sender of a page's event is the page (a `Page` variable), not its Rec: Item Tracking Lines
                // publishes with IncludeSender and Asm. Item Tracking Mgt. takes `var sender: Page "Item Tracking
                // Lines"`. Not pinned by the corpus (assumption page-event-sender).
                (al_ir::ObjKind::Page, _) => match o.globals.iter().position(|g| g.name == "CurrPage") {
                    Some(slot) => {
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::PageEventSender);
                        Some(instance.borrow().globals[slot].clone())
                    }
                    None => return unsupported(format!("{}: IncludeSender on a page without CurrPage", r.name)),
                },
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
        // The publisher's event frame stands below its subscribers' (corpus
        // session/TestSessionInformationCallstackContracts.al:187).
        self.frames.push(object);
        self.call_stack.push(crate::CallFrame {
            event: true,
            routine,
            pos: al_ir::NO_POS,
        });
        self.publish_frame = true;
        let raised = self.raise(&call, &mut values);
        self.publish_frame = false;
        let top = self.call_stack.pop();
        self.frames.pop();
        if let Err(e) = raised {
            // What earlier subscribers wrote back is what the publisher's caller sees.
            let mut e = e;
            if let (Signal::Error(err), Some(top)) = (&mut e, top) {
                self.add_frame(err, object, top);
            }
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
        let from_publisher = std::mem::take(&mut self.publish_frame);
        // A table event (OnAfterInsertEvent and the others the platform raises) puts no frame of its own
        // between the subscriber and the code that raised it (session/TestSessionCallstackSourcesContracts.al:
        // Callstack_FromTablePageReportAndExtensionCode, `table event`); page events and Global Triggers are
        // not measured.
        let unpinned = !from_publisher
            && self
                .engine
                .program
                .objects
                .get(ev.object)
                .is_none_or(|o| !o.is_kind(al_ir::ObjKind::Table));
        if unpinned {
            self.unpinned_event_depth += 1;
        }
        let r = self.raise_to_subscribers(ev, args);
        if unpinned {
            self.unpinned_event_depth -= 1;
        }
        r
    }

    fn raise_to_subscribers(&mut self, ev: &EventCall, args: &mut [Value]) -> Result<(), Signal> {
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
            // (codeunit/TestIsolatedEventSubscriberError.al). The swallowed error becomes the last
            // error, and a subscriber that succeeds has its writes committed (corpus
            // record/TestTableEventSequenceContracts.al:IsolatedEvent_SubscriberError_BecomesTheLastError_...).
            *self
                .event_stats
                .calls
                .borrow_mut()
                .entry((s.object, s.routine))
                .or_insert(0) += 1;
            let isolate = isolated && !self.store.has_uncommitted();
            let before = isolate.then(|| self.store.clone());
            let phase = self.tx_phase;
            let (_, finals) = match self.call(s.object, s.routine, inst, call_args) {
                Err(Signal::Error(e)) if isolate => {
                    self.store = before.expect("snapshot taken when isolating");
                    // The subscriber's own transaction is gone with its writes.
                    self.tx_phase = phase;
                    self.last_error = *e;
                    continue;
                }
                r => r?,
            };
            if isolate {
                self.commit_transaction();
            }
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
        // A subscriber BC would run and this program has no code for: running the event without it
        // would be a wrong answer (ADR-013 decision 6, hard rule 4).
        if let Some(m) = engine.events.missing_for(object, event, element) {
            return unsupported(format!(
                "event {event}: {} subscribes to it and was not compiled (no code the workspace reaches uses it)",
                m.subscriber
            ));
        }
        let Some(subs) = engine.events.subscribers.get(&(object, event.to_string())) else {
            return Ok(Vec::new());
        };
        let mut targets = Vec::new();
        for s in subs {
            if object == engine.events.global_key {
                if let Some(u) = engine.events.unavailable_global.get(&(s.object, s.routine)) {
                    // A subscriber that cannot run would receive this event. BC runs it, and what it
                    // does (change log, Dataverse, API webhooks) AL code can see: refuse. The setup
                    // events are the one exception: while no table the subscriber's decision reads
                    // holds a row, nothing is configured and it answers "no" for every table.
                    let setup = matches!(event, "getglobaltabletriggermask" | "getdatabasetabletriggersetup");
                    let configured = u.config.iter().find(|&&t| self.store.has_rows(t));
                    if !setup || configured.is_some() {
                        let o = &engine.program.objects[s.object];
                        return unsupported(format!(
                            "Global Triggers event {event}: subscriber {}.{} cannot run ({}){}",
                            o.name,
                            o.routines[s.routine].name,
                            u.why,
                            configured.map_or(String::new(), |&t| format!(
                                " and {} holds rows, which it reads",
                                engine.program.objects[t as usize].name
                            ))
                        ));
                    }
                    continue;
                }
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
        // Subscribers run in ascending codeunit id, a bound manual one among them (corpus
        // record/TestTableEventSequenceContracts.al:Subscribers_RunInAscendingCodeunitIdOrder_...);
        // the sort is stable, so one codeunit's subscribers keep their declaration order.
        let program = &engine.program;
        targets.sort_by_key(|(s, _)| program.objects[s.object].id.unwrap_or(0));
        Ok(targets)
    }

    /// Whether raising `event` of `object` now would reach any subscriber.
    pub(crate) fn has_listeners(&mut self, object: usize, event: &str, element: Option<&str>) -> Result<bool, Signal> {
        if !self.engine.events.is_subscribed(object, event) {
            return Ok(false);
        }
        Ok(!self.event_targets(object, event, element)?.is_empty())
    }

    /// The instance a static (automatic) subscriber runs on: the session's one instance for a
    /// `SingleInstance` codeunit, else a fresh one per event, so its globals start at their defaults
    /// every time (corpus record/TestTableEventSequenceContracts.al:Insert_EventsAndTrigger_WithAndWithoutRunTrigger,
    /// where the After event sees `hits=0` after the Before event counted 1).
    fn static_instance(&mut self, object: usize) -> Result<Inst, Signal> {
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

    /// The bound instances per manual subscriber codeunit, for the Event Subscription table.
    fn sync_active_instances(&mut self) {
        self.bindings.retain(|b| b.instance.strong_count() > 0);
        let mut active = std::collections::HashMap::new();
        for b in &self.bindings {
            if let Some(i) = b.instance.upgrade() {
                *active.entry(i.borrow().object as usize).or_insert(0) += 1;
            }
        }
        *self.event_stats.active.borrow_mut() = active;
    }

    /// `BindSubscription(Codeunit)` (`bind`) and `UnbindSubscription(Codeunit)`.
    pub(crate) fn subscription(&mut self, bind: bool, cu: Value, ret_used: bool) -> Result<Value, Signal> {
        let Value::Codeunit(rc) = cu else {
            return unsupported(format!("BindSubscription of {}", cu.describe()));
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
                    // corpus record/TestTableEventSequenceContracts.al:BindSubscription_Manual_Twice_And_Static_Errors
                    Err(binding_error(
                        self.engine.program.objects[object].id.unwrap_or(0),
                        "The codeunit does not use manual binding",
                    ))
                };
            }
            if at.is_some() {
                // Boolean form: false (Contract 4); statement form: an error (Contract 8).
                return if ret_used {
                    Ok(Value::Boolean(false))
                } else {
                    Err(binding_error(
                        self.engine.program.objects[object].id.unwrap_or(0),
                        "The codeunit has already been bound.",
                    ))
                };
            }
            self.ensure_initialized(&inst);
            self.bindings.push(Binding {
                instance: Rc::downgrade(&inst),
            });
            self.sync_active_instances();
            Ok(Value::Boolean(true))
        } else if manual {
            // True even when it was never bound (Contract 6, "container-verified").
            if let Some(i) = at {
                self.bindings.remove(i);
            }
            self.sync_active_instances();
            Ok(Value::Boolean(true))
        } else if ret_used {
            // The corpus pins only that it does not raise (TestEventContracts.al Contract 6).
            Ok(Value::Boolean(false))
        } else {
            Ok(Value::Null)
        }
    }
}
