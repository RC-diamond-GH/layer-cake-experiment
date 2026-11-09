use serde_json::json;

use super::{Event, EventError, EventHub, Subscription, gate::DeliveryGate};

fn event(id: &str, source: &str, job: &str) -> Event {
    Event {
        id: id.into(),
        source: source.into(),
        job_id: job.into(),
        payload: json!({"result":id}),
    }
}
fn subscription(id: &str, source: &str, job: &str, receiver: &str) -> Subscription {
    Subscription {
        id: id.into(),
        source: source.into(),
        job_id: job.into(),
        receiver: receiver.into(),
        active: true,
    }
}

#[test]
fn delivery_gate_matches_formal_transition() {
    let mut gate = DeliveryGate::new();
    assert!(gate.admit("e1"));
    let after_first = gate.clone();
    assert!(!gate.admit("e1"));
    assert_eq!(gate, after_first);
    gate.cancel();
    let cancelled = gate.clone();
    assert!(!gate.admit("e2"));
    assert!(!gate.admit("e1"));
    assert_eq!(gate, cancelled);
}

#[test]
fn publications_match_source_and_job_and_queue_fifo() -> Result<(), EventError> {
    let mut hub = EventHub::new();
    hub.subscribe(subscription("b", "furnace", "job", "receiver"))?;
    hub.subscribe(subscription("a", "furnace", "job", "receiver"))?;
    hub.publish(event("unmatched-source", "timer", "job"))?;
    hub.publish(event("unmatched-job", "furnace", "other"))?;
    let publication = hub.publish(event("second-id", "furnace", "job"))?;
    assert_eq!(
        publication
            .deliveries
            .iter()
            .map(|delivery| delivery.subscription_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    hub.publish(event("first-id", "furnace", "job"))?;
    let deliveries: Vec<_> = std::iter::from_fn(|| hub.pop("receiver"))
        .map(|delivery| (delivery.event.id, delivery.subscription_id))
        .collect();
    assert_eq!(
        deliveries,
        vec![
            ("second-id".into(), "a".into()),
            ("second-id".into(), "b".into()),
            ("first-id".into(), "a".into()),
            ("first-id".into(), "b".into())
        ]
    );
    assert_eq!(hub.retained().len(), 4);
    assert_eq!(hub.pending("receiver"), 0);
    Ok(())
}

#[test]
fn late_registration_replays_history_once_in_publication_order() -> Result<(), EventError> {
    let mut hub = EventHub::new();
    let first = event("z", "furnace", "job");
    hub.publish(first.clone())?;
    hub.publish(event("a", "furnace", "job"))?;
    let deliveries = hub.subscribe(subscription("late", "furnace", "job", "receiver"))?;
    assert_eq!(
        deliveries
            .iter()
            .map(|delivery| delivery.event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["z", "a"]
    );
    assert!(hub.publish(first.clone())?.duplicate);
    assert_eq!(hub.pending("receiver"), 2);
    assert_eq!(
        hub.pop("receiver").map(|delivery| delivery.event.id),
        Some("z".into())
    );
    assert_eq!(
        hub.pop("receiver").map(|delivery| delivery.event.id),
        Some("a".into())
    );
    assert!(hub.publish(first)?.deliveries.is_empty());
    assert!(hub.pop("receiver").is_none());
    Ok(())
}

#[test]
fn cancellation_purges_only_its_subscription_and_prevents_future_admission()
-> Result<(), EventError> {
    let mut hub = EventHub::new();
    hub.subscribe(subscription("cancel", "source", "job", "receiver"))?;
    hub.subscribe(subscription("keep", "source", "job", "receiver"))?;
    hub.publish(event("e1", "source", "job"))?;
    assert_eq!(hub.cancel("cancel")?, 1);
    assert_eq!(hub.cancel("cancel")?, 0);
    hub.publish(event("e2", "source", "job"))?;
    let deliveries: Vec<_> = std::iter::from_fn(|| hub.pop("receiver")).collect();
    assert_eq!(deliveries.len(), 2);
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery.subscription_id == "keep")
    );
    assert!(!hub.subscriptions()["cancel"].active);
    assert!(matches!(
        hub.subscribe(subscription("cancel", "source", "job", "receiver")),
        Err(EventError::ExistingSubscription(_))
    ));
    assert!(matches!(
        hub.cancel("absent"),
        Err(EventError::UnknownSubscription(_))
    ));
    Ok(())
}

#[test]
fn event_identity_cannot_be_reused_with_new_contents() -> Result<(), EventError> {
    let mut hub = EventHub::new();
    hub.publish(event("e", "source", "job"))?;
    assert!(matches!(
        hub.publish(event("e", "different-source", "job")),
        Err(EventError::ConflictingEvent(_))
    ));
    assert_eq!(hub.retained().len(), 1);
    Ok(())
}
