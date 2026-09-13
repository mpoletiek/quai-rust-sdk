//! Portable local event registration, ordering and bounded fan-out tests.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::provider::event_hub::*;
fn config() -> EventHubConfig {
    EventHubConfig {
        max_listeners: 4,
        capacity_per_listener: 2,
        max_queued_items: 4,
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn fanout_is_ordered_keyed_and_once_listener_is_removed_after_its_first_delivery() {
    let mut hub = EventHub::new(config()).unwrap();
    let a = hub.on("block").unwrap();
    let b = hub.once("block").unwrap();
    let other = hub.on("log").unwrap();
    assert_eq!(hub.listeners(Some(&"block")).collect::<Vec<_>>(), [a, b]);
    assert_eq!(hub.emit(&"block", &10), Ok(2));
    assert_eq!(hub.listener_count(Some(&"block")), 1);
    assert_eq!(hub.emit(&"block", &11), Ok(1));
    assert_eq!(hub.queued_items(), 3);
    assert_eq!(hub.poll(other), Ok(EventPoll::Pending));
    assert_eq!(
        hub.poll(b),
        Ok(EventPoll::Event {
            value: 10,
            closed: true
        })
    );
    assert_eq!(hub.poll(b), Err(EventHubError::UnknownListener));
    for n in [10, 11] {
        assert_eq!(
            hub.poll(a),
            Ok(EventPoll::Event {
                value: n,
                closed: false
            })
        );
    }
    assert_eq!(hub.poll(a), Ok(EventPoll::Pending));
    assert_eq!(hub.queued_items(), 0);
    let next = hub.once("block").unwrap();
    assert!(next.get() > b.get());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn capacity_failure_is_atomic_across_listeners_and_event_can_be_explicitly_retried() {
    let mut hub = EventHub::new(config()).unwrap();
    let a = hub.on(1).unwrap();
    let b = hub.on(1).unwrap();
    hub.emit(&1, &"first").unwrap();
    hub.emit(&1, &"second").unwrap();
    assert_eq!(hub.emit(&1, &"third"), Err(EventHubError::AtCapacity));
    assert_eq!(hub.queued_items(), 4);
    assert_eq!(
        hub.poll(a),
        Ok(EventPoll::Event {
            value: "first",
            closed: false
        })
    );
    assert_eq!(hub.emit(&1, &"third"), Err(EventHubError::AtCapacity));
    assert_eq!(hub.queued_items(), 3);
    assert_eq!(
        hub.poll(b),
        Ok(EventPoll::Event {
            value: "first",
            closed: false
        })
    );
    assert_eq!(hub.emit(&1, &"third"), Ok(2));
    for id in [a, b] {
        for value in ["second", "third"] {
            assert_eq!(
                hub.poll(id),
                Ok(EventPoll::Event {
                    value,
                    closed: false
                })
            );
        }
    }
    let mut hub = EventHub::new(EventHubConfig {
        max_queued_items: 1,
        ..config()
    })
    .unwrap();
    let a = hub.once(1).unwrap();
    let b = hub.on(1).unwrap();
    assert_eq!(hub.emit(&1, &5), Err(EventHubError::AtCapacity));
    assert_eq!(hub.listener_count(None), 2);
    assert_eq!(hub.queued_items(), 0);
    assert!(hub.off(b));
    assert_eq!(hub.emit(&1, &5), Ok(1));
    assert_eq!(
        hub.poll(a),
        Ok(EventPoll::Event {
            value: 5,
            closed: true
        })
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn pause_buffer_and_drop_have_explicit_distinct_delivery_semantics() {
    let mut hub = EventHub::new(config()).unwrap();
    let a = hub.on(1).unwrap();
    hub.emit(&1, &1).unwrap();
    hub.pause(EventPause::Buffer).unwrap();
    hub.emit(&1, &2).unwrap();
    assert_eq!(hub.poll(a), Ok(EventPoll::Paused));
    assert_eq!(hub.queued_items(), 2);
    assert_eq!(
        hub.pause(EventPause::Drop),
        Err(EventHubError::PauseConflict)
    );
    assert_eq!(hub.paused(), Some(EventPause::Buffer));
    hub.resume().unwrap();
    assert_eq!(
        hub.poll(a),
        Ok(EventPoll::Event {
            value: 1,
            closed: false
        })
    );
    hub.pause(EventPause::Drop).unwrap();
    assert_eq!(hub.emit(&1, &3), Ok(0));
    assert_eq!(hub.queued_items(), 1);
    assert_eq!(hub.poll(a), Ok(EventPoll::Paused));
    hub.resume().unwrap();
    assert_eq!(
        hub.poll(a),
        Ok(EventPoll::Event {
            value: 2,
            closed: false
        })
    );
    assert_eq!(hub.poll(a), Ok(EventPoll::Pending));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn removal_clears_queued_once_registrations_and_close_never_reopens() {
    let mut hub = EventHub::new(config()).unwrap();
    let a = hub.on(1).unwrap();
    let b = hub.once(1).unwrap();
    let c = hub.on(2).unwrap();
    hub.emit(&1, &"PRIVATE_EVENT").unwrap();
    hub.emit(&2, &"PRIVATE_EVENT").unwrap();
    assert!(!format!("{hub:?}").contains("PRIVATE_EVENT"));
    assert_eq!(hub.remove_all(Some(&1)), 2);
    assert_eq!(hub.queued_items(), 1);
    assert!(!hub.off(a));
    assert!(!hub.off(b));
    assert_eq!(hub.listeners(None).collect::<Vec<_>>(), [c]);
    assert_eq!(hub.remove_all(None), 1);
    assert_eq!(hub.queued_items(), 0);
    hub.close();
    hub.close();
    assert!(hub.is_closed());
    assert_eq!(hub.on(3), Err(EventHubError::Closed));
    assert_eq!(hub.emit(&2, &"PRIVATE_EVENT"), Err(EventHubError::Closed));
    assert_eq!(hub.poll(c), Err(EventHubError::Closed));
    assert_eq!(hub.resume(), Err(EventHubError::Closed));
    assert_eq!(hub.pause(EventPause::Buffer), Err(EventHubError::Closed));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn invalid_limits_and_registration_capacity_fail_without_reusing_ids() {
    for c in [
        EventHubConfig {
            max_listeners: 0,
            ..config()
        },
        EventHubConfig {
            max_listeners: 1025,
            ..config()
        },
        EventHubConfig {
            capacity_per_listener: 0,
            ..config()
        },
        EventHubConfig {
            capacity_per_listener: 1025,
            ..config()
        },
        EventHubConfig {
            max_queued_items: 0,
            ..config()
        },
        EventHubConfig {
            max_queued_items: 65537,
            ..config()
        },
    ] {
        assert!(EventHub::<u8, u8>::new(c).is_err());
    }
    let mut hub = EventHub::<u8, u8>::new(EventHubConfig {
        max_listeners: 1,
        ..config()
    })
    .unwrap();
    let a = hub.once(1).unwrap();
    hub.emit(&1, &7).unwrap();
    assert_eq!(hub.on(2), Err(EventHubError::AtCapacity));
    hub.poll(a).unwrap();
    let b = hub.on(2).unwrap();
    assert!(b.get() > a.get());
    assert_eq!(hub.listener_count(None), 1);
}
