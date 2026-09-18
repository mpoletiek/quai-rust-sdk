//! Runtime-independent bounded local event delivery. Transport polling is explicit.
use std::collections::VecDeque;
use thiserror::Error;

/// Limits count queued items, not their heap byte sizes. Feed bounded transport
/// payloads or application-owned event values and account for their sizes separately.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EventHubConfig {
    /// Maximum live registrations, including one-shot listeners with an unread event.
    pub max_listeners: usize,
    /// Maximum events queued for any one listener.
    pub capacity_per_listener: usize,
    /// Maximum events queued across all listeners.
    pub max_queued_items: usize,
}
impl EventHubConfig {
    /// Replace `max_listeners`.
    pub const fn with_max_listeners(mut self, max_listeners: usize) -> Self {
        self.max_listeners = max_listeners;
        self
    }
    /// Replace `capacity_per_listener`.
    pub const fn with_capacity_per_listener(mut self, capacity_per_listener: usize) -> Self {
        self.capacity_per_listener = capacity_per_listener;
        self
    }
    /// Replace `max_queued_items`.
    pub const fn with_max_queued_items(mut self, max_queued_items: usize) -> Self {
        self.max_queued_items = max_queued_items;
        self
    }
}
impl Default for EventHubConfig {
    fn default() -> Self {
        Self {
            max_listeners: 64,
            capacity_per_listener: 64,
            max_queued_items: 4096,
        }
    }
}
/// Pause delivery while retaining bounded events, or explicitly discard new events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventPause {
    /// Existing and incoming events remain queued within normal limits.
    Buffer,
    /// Discard incoming events; previously queued events remain available on resume.
    Drop,
}
/// Monotonic registration identity; removed IDs are never reused by this hub.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ListenerId(u64);
impl ListenerId {
    /// Numeric identity for application bookkeeping; not portable across hubs.
    pub fn get(self) -> u64 {
        self.0
    }
}
/// Result of polling one local registration, without waiting or executor assumptions.
#[derive(Debug, Eq, PartialEq)]
pub enum EventPoll<E> {
    /// No event currently queued; listener remains active.
    Pending,
    /// Delivery is paused regardless of whether an event is queued.
    Paused,
    /// One event. A one-shot listener closes after delivering this final event.
    Event {
        /// Application event value.
        value: E,
        /// True when this registration was removed after delivery.
        closed: bool,
    },
}
/// Local configuration/lifecycle failures never hide dropped or partially delivered events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum EventHubError {
    /// Limits are zero or exceed the supported item-count bounds.
    #[error("invalid event hub limits")]
    InvalidConfig,
    /// Delivery or registration would exceed a configured limit; no item was queued.
    #[error("event hub capacity exceeded")]
    AtCapacity,
    /// Registration is absent or was removed after one-shot delivery.
    #[error("event listener unavailable")]
    UnknownListener,
    /// Hub has been explicitly closed and cannot be reopened.
    #[error("event hub is closed")]
    Closed,
    /// Pause mode cannot be changed without resuming first.
    #[error("resume event hub before changing pause mode")]
    PauseConflict,
    /// Registration identity space is exhausted; IDs never wrap.
    #[error("event listener identity exhausted")]
    IdExhausted,
}
struct Listener<K, E> {
    id: ListenerId,
    key: K,
    once: bool,
    active: bool,
    queue: VecDeque<E>,
}
/// Application-owned local fan-out for typed event keys and values, on native and
/// Wasm. Feed notifications or canonical replay updates explicitly. No global
/// callbacks, hidden task spawning, automatic network subscriptions or retries.
/// Wrap it in the application's chosen synchronization/executor when needed.
pub struct EventHub<K, E> {
    config: EventHubConfig,
    listeners: Vec<Listener<K, E>>,
    next_id: u64,
    queued: usize,
    paused: Option<EventPause>,
    closed: bool,
}
impl<K, E> std::fmt::Debug for EventHub<K, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub")
            .field("registrations", &self.listeners.len())
            .field("queued", &self.queued)
            .field("paused", &self.paused)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}
impl<K: Eq, E> EventHub<K, E> {
    /// Construct an empty local hub without network access or tasks.
    pub fn new(config: EventHubConfig) -> Result<Self, EventHubError> {
        if !(1..=1024).contains(&config.max_listeners)
            || !(1..=1024).contains(&config.capacity_per_listener)
            || !(1..=65536).contains(&config.max_queued_items)
        {
            return Err(EventHubError::InvalidConfig);
        }
        Ok(Self {
            config,
            listeners: vec![],
            next_id: 1,
            queued: 0,
            paused: None,
            closed: false,
        })
    }
    /// Register a persistent listener for one exact key.
    pub fn on(&mut self, key: K) -> Result<ListenerId, EventHubError> {
        self.register(key, false)
    }
    /// Register one event; registration remains allocated until its event is read or removed.
    pub fn once(&mut self, key: K) -> Result<ListenerId, EventHubError> {
        self.register(key, true)
    }
    fn register(&mut self, key: K, once: bool) -> Result<ListenerId, EventHubError> {
        if self.closed {
            return Err(EventHubError::Closed);
        }
        if self.listeners.len() >= self.config.max_listeners {
            return Err(EventHubError::AtCapacity);
        }
        let id = ListenerId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(EventHubError::IdExhausted)?;
        self.listeners.push(Listener {
            id,
            key,
            once,
            active: true,
            queue: VecDeque::new(),
        });
        Ok(id)
    }
    /// Active listener identities in registration order, optionally matching one key.
    /// A one-shot listener with its event already queued is no longer active.
    pub fn listeners<'a>(&'a self, key: Option<&'a K>) -> impl Iterator<Item = ListenerId> + 'a {
        self.listeners
            .iter()
            .filter(move |l| l.active && key.is_none_or(|key| key == &l.key))
            .map(|l| l.id)
    }
    /// Number of active listeners, optionally matching one exact key.
    pub fn listener_count(&self, key: Option<&K>) -> usize {
        self.listeners(key).count()
    }
    /// Number of event items retained across all registrations.
    pub fn queued_items(&self) -> usize {
        self.queued
    }
    /// Remove one registration and its queued events. Returns false if already absent.
    pub fn off(&mut self, id: ListenerId) -> bool {
        if let Some(i) = self.listeners.iter().position(|l| l.id == id) {
            let l = self.listeners.remove(i);
            self.queued -= l.queue.len();
            true
        } else {
            false
        }
    }
    /// Remove every matching registration, including queued one-shot listeners.
    /// None clears all keys. Returns the number of removed registrations.
    pub fn remove_all(&mut self, key: Option<&K>) -> usize {
        let before = self.listeners.len();
        self.listeners.retain(|l| {
            let remove = key.is_none_or(|key| key == &l.key);
            if remove {
                self.queued -= l.queue.len();
            }
            !remove
        });
        before - self.listeners.len()
    }
    /// Poll without waiting. Receiving the one-shot event removes its registration.
    pub fn poll(&mut self, id: ListenerId) -> Result<EventPoll<E>, EventHubError> {
        if self.closed {
            return Err(EventHubError::Closed);
        }
        let index = self
            .listeners
            .iter()
            .position(|l| l.id == id)
            .ok_or(EventHubError::UnknownListener)?;
        if self.paused.is_some() {
            return Ok(EventPoll::Paused);
        }
        let listener = &mut self.listeners[index];
        let Some(value) = listener.queue.pop_front() else {
            return Ok(EventPoll::Pending);
        };
        self.queued -= 1;
        let closed = !listener.active && listener.queue.is_empty();
        if closed {
            self.listeners.remove(index);
        }
        Ok(EventPoll::Event { value, closed })
    }
    /// Pause delivery. Repeating the same mode is idempotent; switching mode requires resume.
    pub fn pause(&mut self, mode: EventPause) -> Result<(), EventHubError> {
        if self.closed {
            return Err(EventHubError::Closed);
        }
        if self.paused.is_some_and(|old| old != mode) {
            return Err(EventHubError::PauseConflict);
        }
        self.paused = Some(mode);
        Ok(())
    }
    /// Resume local delivery; no network state is inferred or changed.
    pub fn resume(&mut self) -> Result<(), EventHubError> {
        if self.closed {
            return Err(EventHubError::Closed);
        }
        self.paused = None;
        Ok(())
    }
    /// Current explicit pause policy.
    pub fn paused(&self) -> Option<EventPause> {
        self.paused
    }
    /// Irreversibly close local delivery and discard all queued values/registrations.
    pub fn close(&mut self) {
        self.listeners.clear();
        self.queued = 0;
        self.closed = true;
        self.paused = None;
    }
    /// Whether local delivery has been explicitly closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
impl<K: Eq, E: Clone> EventHub<K, E> {
    /// Queue one event for all active matching listeners. Capacity checks occur
    /// before any queue changes; on error the caller retains the supplied event
    /// and decides whether to retry after draining or perform gap recovery.
    /// Returns deliveries queued; paused Drop mode and unmatched keys return zero.
    pub fn emit(&mut self, key: &K, event: &E) -> Result<usize, EventHubError> {
        if self.closed {
            return Err(EventHubError::Closed);
        }
        if self.paused == Some(EventPause::Drop) {
            return Ok(0);
        }
        let count = self
            .listeners
            .iter()
            .filter(|l| l.active && &l.key == key)
            .count();
        if self
            .queued
            .checked_add(count)
            .is_none_or(|n| n > self.config.max_queued_items)
            || self.listeners.iter().any(|l| {
                l.active && &l.key == key && l.queue.len() >= self.config.capacity_per_listener
            })
        {
            return Err(EventHubError::AtCapacity);
        }
        for listener in self
            .listeners
            .iter_mut()
            .filter(|l| l.active && &l.key == key)
        {
            listener.queue.push_back(event.clone());
            if listener.once {
                listener.active = false;
            }
        }
        self.queued += count;
        Ok(count)
    }
}
