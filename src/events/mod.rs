//! Retained events and deterministic FIFO subscription delivery.
pub mod gate;
mod hub;

pub use hub::{Delivery, Event, EventError, EventHub, Publication, Subscription};

#[cfg(test)]
mod tests;
