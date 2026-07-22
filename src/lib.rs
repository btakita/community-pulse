//! Community Pulse: community signals and agent research under a user-owned attention budget.

pub mod app;
pub mod chat;
pub mod domain;
pub mod engine;
pub mod ingest;
pub mod live;
pub mod mcp;
pub mod reactive;
pub mod research;
pub mod setup;
pub mod tools;

pub use domain::{ChatMessage, ChatRole, DigestCard, InterestModel, TrendEvidence};
pub use engine::PulseEngine;
pub use reactive::{PulseState, UiSnapshot};
pub use tools::ToolBridge;
