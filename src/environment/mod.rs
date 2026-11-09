//! Private deterministic simulation; time advances only at explicit boundaries.
mod simulation;
mod tools;

pub use simulation::{
    Clock, EnvironmentError, Furnace, Job, JobStatus, Simulation, VersionedResource,
};
pub use tools::simulation_tools;

#[cfg(test)]
mod tests;
