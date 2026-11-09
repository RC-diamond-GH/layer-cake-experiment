use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::gate::DeliveryGate;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub source: String,
    pub job_id: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub source: String,
    pub job_id: String,
    pub receiver: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub subscription_id: String,
    pub receiver: String,
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Publication {
    pub duplicate: bool,
    pub deliveries: Vec<Delivery>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum EventError {
    #[error("subscription already exists: {0}")]
    ExistingSubscription(String),
    #[error("subscription does not exist: {0}")]
    UnknownSubscription(String),
    #[error("event identity reused with different contents: {0}")]
    ConflictingEvent(String),
}

/// History uses publication order, subscriptions use identity order, and each
/// receiver queue preserves actual enqueue order. Cancelled IDs cannot be reused.
#[derive(Clone, Debug, Default)]
pub struct EventHub {
    retained: Vec<Event>,
    event_index: BTreeMap<String, usize>,
    subscriptions: BTreeMap<String, Subscription>,
    gates: BTreeMap<String, DeliveryGate>,
    queues: BTreeMap<String, VecDeque<Delivery>>,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn retained(&self) -> &[Event] {
        &self.retained
    }
    pub fn subscriptions(&self) -> &BTreeMap<String, Subscription> {
        &self.subscriptions
    }
    pub fn pending(&self, receiver: &str) -> usize {
        self.queues.get(receiver).map_or(0, VecDeque::len)
    }

    pub fn publish(&mut self, event: Event) -> Result<Publication, EventError> {
        if let Some(index) = self.event_index.get(&event.id) {
            if self.retained[*index] != event {
                return Err(EventError::ConflictingEvent(event.id));
            }
            return Ok(Publication {
                duplicate: true,
                deliveries: Vec::new(),
            });
        }
        let mut deliveries = Vec::new();
        for (id, subscription) in &self.subscriptions {
            if subscription.active
                && subscription.source == event.source
                && subscription.job_id == event.job_id
            {
                if let Some(gate) = self.gates.get_mut(id) {
                    if gate.admit(&event.id) {
                        let delivery = Delivery {
                            subscription_id: id.clone(),
                            receiver: subscription.receiver.clone(),
                            event: event.clone(),
                        };
                        self.queues
                            .entry(subscription.receiver.clone())
                            .or_default()
                            .push_back(delivery.clone());
                        deliveries.push(delivery);
                    }
                }
            }
        }
        self.event_index
            .insert(event.id.clone(), self.retained.len());
        self.retained.push(event);
        Ok(Publication {
            duplicate: false,
            deliveries,
        })
    }

    pub fn subscribe(&mut self, subscription: Subscription) -> Result<Vec<Delivery>, EventError> {
        if self.subscriptions.contains_key(&subscription.id) {
            return Err(EventError::ExistingSubscription(subscription.id));
        }
        let mut gate = DeliveryGate::new();
        if !subscription.active {
            gate.cancel();
        }
        let mut deliveries = Vec::new();
        for event in &self.retained {
            if event.source == subscription.source
                && event.job_id == subscription.job_id
                && gate.admit(&event.id)
            {
                let delivery = Delivery {
                    subscription_id: subscription.id.clone(),
                    receiver: subscription.receiver.clone(),
                    event: event.clone(),
                };
                self.queues
                    .entry(subscription.receiver.clone())
                    .or_default()
                    .push_back(delivery.clone());
                deliveries.push(delivery);
            }
        }
        self.gates.insert(subscription.id.clone(), gate);
        self.subscriptions
            .insert(subscription.id.clone(), subscription);
        Ok(deliveries)
    }

    pub fn cancel(&mut self, id: &str) -> Result<usize, EventError> {
        let subscription = self
            .subscriptions
            .get_mut(id)
            .ok_or_else(|| EventError::UnknownSubscription(id.to_owned()))?;
        subscription.active = false;
        if let Some(gate) = self.gates.get_mut(id) {
            gate.cancel();
        }
        let mut purged = 0;
        if let Some(queue) = self.queues.get_mut(&subscription.receiver) {
            let before = queue.len();
            queue.retain(|delivery| delivery.subscription_id != id);
            purged = before - queue.len();
        }
        Ok(purged)
    }

    pub fn pop(&mut self, receiver: &str) -> Option<Delivery> {
        self.queues.get_mut(receiver)?.pop_front()
    }
}
