//! Reactive Gameplay Event Bus for autonomous quest tracking, achievements, and scripting.
//!
//! Provides a high-throughput, publish-subscribe event dispatch system with zero heap allocations
//! during runtime event emission and subscriber delivery.

/// Maximum events buffered in the event ring buffer.
pub const EVENT_QUEUE_CAPACITY: usize = 256;
/// Maximum registered listener callbacks.
pub const MAX_SUBSCRIBERS: usize = 32;

/// Strongly-typed game events emitted by gameplay subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEvent {
    /// An entity was killed in combat.
    EntityKilled {
        /// Attacker / killer entity ID.
        killer_id: u64,
        /// Defeated victim entity ID.
        victim_id: u64,
        /// Archetype / template ID of the victim.
        victim_type: u32,
        /// Server zone ID where the kill occurred.
        zone_id: u32,
        /// Server tick when the event occurred.
        tick: u64,
    },
    /// An item was acquired or crafted.
    ItemAcquired {
        /// Recipient player account ID.
        player_id: u64,
        /// Blueprint item ID.
        item_id: u32,
        /// Quantity acquired.
        quantity: u32,
        /// Server tick when the event occurred.
        tick: u64,
    },
    /// A player crossed into a spatial location or territory.
    LocationReached {
        /// Player account ID.
        player_id: u64,
        /// Zone ID.
        zone_id: u32,
        /// Sector X coordinate.
        sector_x: i32,
        /// Sector Z coordinate.
        sector_z: i32,
        /// Server tick when reached.
        tick: u64,
    },
    /// A player interacted with an in-world object (chest, NPC, node).
    ObjectInteracted {
        /// Interacting player account ID.
        player_id: u64,
        /// Target object / entity ID.
        object_id: u64,
        /// Interaction classification (e.g. 1 = Talk, 2 = Harvest, 3 = Open).
        interaction_type: u32,
        /// Server tick when interacted.
        tick: u64,
    },
    /// Custom script event for emergent game mechanics.
    Custom {
        /// Custom event opcode.
        event_id: u32,
        /// Actor account ID.
        actor_id: u64,
        /// Generic parameter 1.
        param1: u64,
        /// Generic parameter 2.
        param2: u64,
        /// Server tick.
        tick: u64,
    },
}

impl GameEvent {
    /// Returns the discriminator ID for this event type.
    pub const fn event_type_id(&self) -> u16 {
        match self {
            Self::EntityKilled { .. } => 1,
            Self::ItemAcquired { .. } => 2,
            Self::LocationReached { .. } => 3,
            Self::ObjectInteracted { .. } => 4,
            Self::Custom { .. } => 5,
        }
    }
}

/// Callback function signature for event listeners.
pub type EventCallback = fn(&GameEvent, &mut EventContext);

/// Context passed into subscriber callbacks for recording reactions.
#[derive(Debug, Default)]
pub struct EventContext {
    /// Count of handled actions.
    pub actions_triggered: u32,
}

/// A registered event subscriber.
#[derive(Clone, Copy)]
pub struct EventSubscriber {
    /// Unique subscriber ID.
    pub subscriber_id: u32,
    /// Bitmask of subscribed event type IDs (e.g. (1 << 1) for EntityKilled).
    pub filter_mask: u32,
    /// Function pointer callback.
    pub callback: EventCallback,
}

/// Zero-allocation publish-subscribe event bus.
pub struct EventBus {
    queue: [Option<GameEvent>; EVENT_QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    subscribers: [Option<EventSubscriber>; MAX_SUBSCRIBERS],
    subscriber_count: usize,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            queue: [None; EVENT_QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            subscribers: [None; MAX_SUBSCRIBERS],
            subscriber_count: 0,
        }
    }
}

impl EventBus {
    /// Creates a new event bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes a game event to the ring buffer.
    ///
    /// If the queue is saturated, the oldest un-dispatched event is dropped.
    pub fn publish(&mut self, event: GameEvent) {
        if self.count >= EVENT_QUEUE_CAPACITY {
            // Drop oldest
            self.head = (self.head + 1) % EVENT_QUEUE_CAPACITY;
            self.count -= 1;
        }

        self.queue[self.tail] = Some(event);
        self.tail = (self.tail + 1) % EVENT_QUEUE_CAPACITY;
        self.count += 1;
    }

    /// Subscribes a callback to receive events matching the filter mask.
    pub fn subscribe(
        &mut self,
        subscriber_id: u32,
        filter_mask: u32,
        callback: EventCallback,
    ) -> bool {
        if self.subscriber_count >= MAX_SUBSCRIBERS {
            return false;
        }

        for slot in self.subscribers.iter_mut() {
            if slot.is_none() {
                *slot = Some(EventSubscriber {
                    subscriber_id,
                    filter_mask,
                    callback,
                });
                self.subscriber_count += 1;
                return true;
            }
        }

        false
    }

    /// Unsubscribes a listener by subscriber ID.
    pub fn unsubscribe(&mut self, subscriber_id: u32) -> bool {
        for slot in self.subscribers.iter_mut() {
            if let Some(ref sub) = slot {
                if sub.subscriber_id == subscriber_id {
                    *slot = None;
                    self.subscriber_count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// Drains and dispatches all queued events to matching subscribers.
    ///
    /// Returns the total number of events dispatched.
    pub fn dispatch_all(&mut self, context: &mut EventContext) -> usize {
        let dispatched_count = self.count;

        while self.count > 0 {
            if let Some(event) = self.queue[self.head].take() {
                let type_bit = 1 << event.event_type_id();

                for sub in self.subscribers.iter().flatten() {
                    if (sub.filter_mask & type_bit) != 0 {
                        (sub.callback)(&event, context);
                    }
                }
            }

            self.head = (self.head + 1) % EVENT_QUEUE_CAPACITY;
            self.count -= 1;
        }

        dispatched_count
    }

    /// Returns the number of events currently queued.
    pub fn pending_count(&self) -> usize {
        self.count
    }

    /// Checks if the event queue is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_kill_event(event: &GameEvent, ctx: &mut EventContext) {
        if let GameEvent::EntityKilled { victim_type, .. } = event {
            if *victim_type == 42 {
                ctx.actions_triggered += 1;
            }
        }
    }

    #[test]
    fn test_event_bus_publish_and_dispatch() {
        let mut bus = EventBus::new();

        // Subscribe to EntityKilled (type_id = 1, bit = 1 << 1 = 2)
        assert!(bus.subscribe(1, 1 << 1, on_kill_event));

        // Publish kill event matching victim_type 42
        bus.publish(GameEvent::EntityKilled {
            killer_id: 100,
            victim_id: 200,
            victim_type: 42,
            zone_id: 1,
            tick: 10,
        });

        // Publish kill event with different victim_type
        bus.publish(GameEvent::EntityKilled {
            killer_id: 100,
            victim_id: 201,
            victim_type: 99,
            zone_id: 1,
            tick: 11,
        });

        // Publish item event (should not be delivered to subscriber due to filter mask)
        bus.publish(GameEvent::ItemAcquired {
            player_id: 100,
            item_id: 1001,
            quantity: 5,
            tick: 12,
        });

        let mut ctx = EventContext::default();
        let dispatched = bus.dispatch_all(&mut ctx);

        assert_eq!(dispatched, 3);
        assert_eq!(ctx.actions_triggered, 1);
        assert!(bus.is_empty());
    }
}
