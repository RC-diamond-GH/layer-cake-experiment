//! Self-contained run inspection and structural run comparisons.
mod comparison;
mod html;
#[cfg(test)]
mod tests;

use thiserror::Error;

pub use comparison::compare;
pub use html::html;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("could not serialize report data: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid call record {record_id}: {message}")]
    InvalidCall { record_id: String, message: String },
}
