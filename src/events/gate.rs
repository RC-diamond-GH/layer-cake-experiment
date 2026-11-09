use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The exact mechanism mirrored by `AgentTheory/MVP.lean`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryGate {
    pub active: bool,
    pub seen: BTreeSet<String>,
}

impl Default for DeliveryGate {
    fn default() -> Self {
        Self {
            active: true,
            seen: BTreeSet::new(),
        }
    }
}

impl DeliveryGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn admit(&mut self, event_id: &str) -> bool {
        if !self.active || self.seen.contains(event_id) {
            return false;
        }
        self.seen.insert(event_id.to_owned());
        true
    }

    pub fn cancel(&mut self) {
        self.active = false;
    }
}
