//! A typed event bus.
//!
//! Engine systems and mods subscribe through the same API, so a mod's handler
//! is never a second-class citizen. Handlers run in descending priority order;
//! ties break by registration order, which keeps behaviour deterministic.
//!
//! Handlers receive `&mut E`, so an event can carry a decision back out — see
//! [`Cancellable`] for the common "a handler vetoed this" case.
//!
//! Handlers are `Fn`, not `FnMut`, and [`EventBus::emit`] takes `&self`. That
//! keeps emitting re-entrant (a handler may emit further events) at the cost of
//! requiring interior mutability in handlers that accumulate state. Pushing
//! into a `RefCell<Vec<_>>` command queue is the intended pattern.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Marker for anything that can travel on the bus.
///
/// Blanket-implemented; there is nothing to write by hand.
pub trait Event: Any {}
impl<T: Any> Event for T {}

/// Events whose handlers may veto the action that triggered them.
pub trait Cancellable {
    fn is_cancelled(&self) -> bool;
    fn cancel(&mut self);
}

/// Ordering hint for handlers. Higher runs first.
pub type Priority = i32;

pub const PRIORITY_HIGH: Priority = 100;
pub const PRIORITY_NORMAL: Priority = 0;
pub const PRIORITY_LOW: Priority = -100;

struct Handler<E> {
    priority: Priority,
    /// Who registered this — `"engine"` or a mod id. Used for diagnostics and
    /// for unsubscribing a mod wholesale when it is unloaded.
    source: String,
    callback: Box<dyn Fn(&mut E)>,
}

/// One event type's handlers, type-erased.
struct HandlerList {
    /// Really a `Vec<Handler<E>>` for the `E` this entry is keyed by.
    handlers: Box<dyn Any>,
    /// Monomorphised for `E` at insertion time, so we can filter the list
    /// without knowing `E` at the call site.
    remove_source: fn(&mut dyn Any, &str),
}

fn remove_source_impl<E: Event>(handlers: &mut dyn Any, source: &str) {
    if let Some(list) = handlers.downcast_mut::<Vec<Handler<E>>>() {
        list.retain(|handler| handler.source != source);
    }
}

/// Dispatches events to registered handlers.
#[derive(Default)]
pub struct EventBus {
    /// `TypeId::of::<E>()` -> that type's handler list, erased so one map can
    /// hold every event type. Downcast is infallible because the key is
    /// derived from the same type as the value.
    handlers: HashMap<TypeId, HandlerList>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler at normal priority.
    pub fn subscribe<E: Event>(&mut self, source: impl Into<String>, callback: impl Fn(&mut E) + 'static) {
        self.subscribe_with_priority(source, PRIORITY_NORMAL, callback);
    }

    /// Register a handler, controlling where it sits in the running order.
    pub fn subscribe_with_priority<E: Event>(
        &mut self,
        source: impl Into<String>,
        priority: Priority,
        callback: impl Fn(&mut E) + 'static,
    ) {
        let handler = Handler {
            priority,
            source: source.into(),
            callback: Box::new(callback),
        };
        let list = self
            .handlers
            .entry(TypeId::of::<E>())
            .or_insert_with(|| HandlerList {
                handlers: Box::new(Vec::<Handler<E>>::new()),
                remove_source: remove_source_impl::<E>,
            })
            .handlers
            .downcast_mut::<Vec<Handler<E>>>()
            .expect("handler list type matches its TypeId key");

        // Insert at the end of the run of equal-or-higher priorities, so equal
        // priorities keep registration order.
        let at = list
            .iter()
            .position(|existing| existing.priority < priority)
            .unwrap_or(list.len());
        list.insert(at, handler);
    }

    /// Run every handler for `E` against `event`.
    pub fn emit<E: Event>(&self, event: &mut E) {
        let Some(entry) = self.handlers.get(&TypeId::of::<E>()) else {
            return;
        };
        let list = entry
            .handlers
            .downcast_ref::<Vec<Handler<E>>>()
            .expect("handler list type matches its TypeId key");
        for handler in list {
            (handler.callback)(event);
        }
    }

    /// Run handlers for `E`, stopping as soon as one cancels the event.
    /// Returns `true` if the event survived uncancelled.
    pub fn emit_cancellable<E: Event + Cancellable>(&self, event: &mut E) -> bool {
        let Some(entry) = self.handlers.get(&TypeId::of::<E>()) else {
            return !event.is_cancelled();
        };
        let list = entry
            .handlers
            .downcast_ref::<Vec<Handler<E>>>()
            .expect("handler list type matches its TypeId key");
        for handler in list {
            if event.is_cancelled() {
                break;
            }
            (handler.callback)(event);
        }
        !event.is_cancelled()
    }

    /// Drop every handler registered under `source`, across all event types.
    /// Called when a mod unloads.
    pub fn unsubscribe_source(&mut self, source: &str) {
        for entry in self.handlers.values_mut() {
            (entry.remove_source)(entry.handlers.as_mut(), source);
        }
    }

    /// Number of handlers registered for `E`.
    pub fn handler_count<E: Event>(&self) -> usize {
        self.handlers
            .get(&TypeId::of::<E>())
            .and_then(|entry| entry.handlers.downcast_ref::<Vec<Handler<E>>>())
            .map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Tick {
        count: u32,
    }

    struct BlockBreak {
        hardness: f32,
        cancelled: bool,
    }

    impl Cancellable for BlockBreak {
        fn is_cancelled(&self) -> bool {
            self.cancelled
        }
        fn cancel(&mut self) {
            self.cancelled = true;
        }
    }

    #[test]
    fn emitting_with_no_handlers_is_a_no_op() {
        let bus = EventBus::new();
        let mut tick = Tick { count: 0 };
        bus.emit(&mut tick);
        assert_eq!(tick.count, 0);
    }

    #[test]
    fn handlers_observe_and_mutate_the_event() {
        let mut bus = EventBus::new();
        bus.subscribe("engine", |tick: &mut Tick| tick.count += 1);
        bus.subscribe("mymod", |tick: &mut Tick| tick.count += 10);

        let mut tick = Tick { count: 0 };
        bus.emit(&mut tick);
        assert_eq!(tick.count, 11);
        assert_eq!(bus.handler_count::<Tick>(), 2);
    }

    #[test]
    fn events_are_routed_only_to_their_own_type() {
        let mut bus = EventBus::new();
        bus.subscribe("engine", |tick: &mut Tick| tick.count += 1);

        let mut event = BlockBreak { hardness: 1.0, cancelled: false };
        bus.emit(&mut event);
        assert_eq!(event.hardness, 1.0);
        assert_eq!(bus.handler_count::<BlockBreak>(), 0);
    }

    #[test]
    fn higher_priority_runs_first_and_ties_keep_registration_order() {
        let order = Rc::new(RefCell::new(Vec::new()));

        let mut bus = EventBus::new();
        for (label, priority) in [
            ("normal_first", PRIORITY_NORMAL),
            ("low", PRIORITY_LOW),
            ("high", PRIORITY_HIGH),
            ("normal_second", PRIORITY_NORMAL),
        ] {
            let order = Rc::clone(&order);
            bus.subscribe_with_priority("test", priority, move |_: &mut Tick| {
                order.borrow_mut().push(label);
            });
        }

        bus.emit(&mut Tick { count: 0 });
        assert_eq!(
            *order.borrow(),
            vec!["high", "normal_first", "normal_second", "low"]
        );
    }

    #[test]
    fn cancelling_stops_later_handlers_and_reports_the_veto() {
        let reached_last = Rc::new(RefCell::new(false));
        let reached_last_inner = Rc::clone(&reached_last);

        let mut bus = EventBus::new();
        bus.subscribe_with_priority("vetoer", PRIORITY_HIGH, |event: &mut BlockBreak| {
            event.cancel();
        });
        bus.subscribe_with_priority("late", PRIORITY_LOW, move |_: &mut BlockBreak| {
            *reached_last_inner.borrow_mut() = true;
        });

        let mut event = BlockBreak { hardness: 1.0, cancelled: false };
        let survived = bus.emit_cancellable(&mut event);

        assert!(!survived);
        assert!(event.is_cancelled());
        assert!(!*reached_last.borrow(), "handlers after a cancel must not run");
    }

    #[test]
    fn an_uncancelled_event_reaches_every_handler() {
        let mut bus = EventBus::new();
        bus.subscribe("a", |event: &mut BlockBreak| event.hardness *= 2.0);
        bus.subscribe("b", |event: &mut BlockBreak| event.hardness += 1.0);

        let mut event = BlockBreak { hardness: 3.0, cancelled: false };
        assert!(bus.emit_cancellable(&mut event));
        assert_eq!(event.hardness, 7.0);
    }

    #[test]
    fn unsubscribing_a_source_drops_its_handlers_across_every_event_type() {
        let mut bus = EventBus::new();
        bus.subscribe("engine", |tick: &mut Tick| tick.count += 1);
        bus.subscribe("mymod", |tick: &mut Tick| tick.count += 10);
        bus.subscribe("mymod", |event: &mut BlockBreak| event.hardness += 5.0);

        assert_eq!(bus.handler_count::<Tick>(), 2);
        assert_eq!(bus.handler_count::<BlockBreak>(), 1);

        bus.unsubscribe_source("mymod");

        assert_eq!(bus.handler_count::<Tick>(), 1);
        assert_eq!(bus.handler_count::<BlockBreak>(), 0);

        // The surviving engine handler still runs, and the mod's does not.
        let mut tick = Tick { count: 0 };
        bus.emit(&mut tick);
        assert_eq!(tick.count, 1);
    }

    #[test]
    fn unsubscribing_an_unknown_source_changes_nothing() {
        let mut bus = EventBus::new();
        bus.subscribe("engine", |tick: &mut Tick| tick.count += 1);
        bus.unsubscribe_source("never_registered");
        assert_eq!(bus.handler_count::<Tick>(), 1);
    }

    #[test]
    fn handlers_accumulate_state_through_interior_mutability() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_inner = Rc::clone(&seen);

        let mut bus = EventBus::new();
        bus.subscribe("collector", move |tick: &mut Tick| {
            seen_inner.borrow_mut().push(tick.count);
        });

        for count in [1, 2, 3] {
            bus.emit(&mut Tick { count });
        }
        assert_eq!(*seen.borrow(), vec![1, 2, 3]);
    }
}
