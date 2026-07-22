//! Community Pulse: community signals ranked under a fixed attention budget.

pub mod app;
pub mod chat;
pub mod domain;
pub mod engine;
pub mod ingest;
pub mod reactive;
pub mod tools;

pub use domain::{ChatMessage, ChatRole, DigestCard, InterestModel, TrendEvidence};
pub use engine::PulseEngine;
pub use reactive::{PulseState, UiSnapshot};
pub use tools::ToolBridge;
