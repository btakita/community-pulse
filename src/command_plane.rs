//! `#lzdurablesink` Phase 4 second consumer — community-pulse adopts the lazily
//! command-plane durable-sink shape for the attention-budget transition.
//!
//! This mirrors agent-doc's closeout authority shape — `CommandSubmit` →
//! authority decides from live state → durable sink (`PulseEngine` SQLite) →
//! terminal [`lazily::CausalReceipt`] — in an unrelated application, proving the
//! shape replicates outside agent-doc. That unblocks the plan's Phase 4
//! promotion review (extract a shared lazily helper only once two consumers
//! share the shape).
//!
//! The budget transition is small and self-contained: the operator sets the
//! attention budget; the authority validates/clamps it against the live engine
//! state, sinks it as an idempotent `settings` upsert, and acknowledges with a
//! terminal receipt — never a transport ACK. `community-pulse` is a single
//! process, so this is an in-process authority (no socket); the contract is the
//! shape, not the transport.

use anyhow::{Context, Result};
use lazily::{CausalReceipt, CommandPolicy, CommandSubmit, DedupePolicy, IpcValue};
use serde::{Deserialize, Serialize};

use crate::engine::PulseEngine;

/// Domain namespace owning community-pulse command payloads.
pub const NAMESPACE: &str = "community-pulse";
/// Authority target identity (the engine).
pub const ENGINE_TARGET: &str = "community-pulse-engine";
/// Command name for an attention-budget set.
pub const SET_BUDGET_NAME: &str = "set_budget";
/// Fully-qualified payload schema id for the budget set.
pub const SET_BUDGET_PAYLOAD_TYPE: &str = "community-pulse.set_budget.v1";

/// Payload body for `community-pulse.set_budget.v1`. The operator asks the
/// engine authority to set the attention budget; the authority clamps it to the
/// valid range and persists it as an idempotent projection upsert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetBudgetPayload {
    pub budget: usize,
}

/// Build the `CommandSubmit` for an attention-budget set. `command_id` must be
/// stable + replay-safe; the idempotency key dedupes concurrent sets of the same
/// budget so a duplicate fold is exactly the sink's idempotent re-delivery.
pub fn build_set_budget_submit(command_id: impl Into<String>, budget: usize) -> Result<CommandSubmit> {
    let command_id = command_id.into();
    let bytes = serde_json::to_vec(&SetBudgetPayload { budget }).context("encode set_budget payload")?;
    Ok(CommandSubmit {
        causation_id: command_id.clone(),
        command_id,
        source: "community-pulse-ui".to_string(),
        target: ENGINE_TARGET.to_string(),
        namespace: NAMESPACE.to_string(),
        name: SET_BUDGET_NAME.to_string(),
        authority_generation: 1,
        idempotency_key: format!("set_budget:{budget}"),
        deadline_ms: 0,
        policy: CommandPolicy {
            // Two sets of the same budget fold onto one command; a newer budget
            // value supersedes a pending older one.
            dedupe: DedupePolicy::SameIdempotencyKey,
            supersede: true,
            cancel_on_preempt: false,
        },
        payload_type: SET_BUDGET_PAYLOAD_TYPE.to_string(),
        payload_hash: payload_hash(&bytes),
        payload: IpcValue::Inline(bytes),
        required_features: Vec::new(),
    })
}

/// Authority-side service for a `set_budget` command (`#lzdurablesink`): decode
/// the payload, clamp the budget against the engine's valid range, sink it as an
/// idempotent `settings` upsert, and return the terminal [`CausalReceipt`]. A
/// decode/sink failure is a `rejected` receipt (fail closed); an applied set is
/// `applied`. This is the durable-sink rule: the engine is the authority, the
/// SQLite upsert is the write-only sink, and the receipt is the monotone ack.
pub fn service_set_budget(engine: &PulseEngine, submit: &CommandSubmit) -> CausalReceipt {
    let outcome = set_budget_outcome(engine, submit);
    let receipt_id = format!("{}:rcpt", submit.command_id);
    match outcome {
        Ok(applied_budget) => CausalReceipt::applied(
            receipt_id,
            &submit.command_id,
            ENGINE_TARGET,
            submit.authority_generation,
        )
        .with_payload_hash(format!("budget:{applied_budget}")),
        Err(reason) => CausalReceipt::rejected(
            receipt_id,
            &submit.command_id,
            ENGINE_TARGET,
            submit.authority_generation,
        )
        .with_reason(reason),
    }
}

fn set_budget_outcome(engine: &PulseEngine, submit: &CommandSubmit) -> Result<usize, String> {
    if submit.payload_type != SET_BUDGET_PAYLOAD_TYPE {
        return Err(format!(
            "unexpected payload_type {:?} (want {SET_BUDGET_PAYLOAD_TYPE})",
            submit.payload_type,
        ));
    }
    let IpcValue::Inline(bytes) = &submit.payload else {
        return Err("set_budget payload must be inline bytes".to_string());
    };
    let payload: SetBudgetPayload =
        serde_json::from_slice(bytes).map_err(|e| format!("decode set_budget payload: {e:#}"))?;
    engine
        .set_budget(payload.budget)
        .map_err(|e| format!("sink set_budget: {e:#}"))
}

fn payload_hash(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("fnv1a:{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazily::ReceiptOutcome;

    /// The durable-sink round-trip for the budget transition: build a submit,
    /// service it through the authority, the terminal receipt is `applied`, and
    /// the sink fired (the engine's budget reflects the clamped value). Proves
    /// the lazily command-plane durable-sink shape works in a second app.
    #[test]
    fn set_budget_round_trips_through_command_plane_authority() {
        let engine = PulseEngine::in_memory().unwrap();
        let submit = build_set_budget_submit("cmd-budget-1", 7).unwrap();

        let receipt = service_set_budget(&engine, &submit);
        assert_eq!(receipt.outcome, ReceiptOutcome::Applied);
        assert_eq!(receipt.causation_id, submit.command_id);
        assert_eq!(receipt.payload_hash.as_deref(), Some("budget:7"));

        // The durable sink fired: the engine's live budget reflects the set.
        assert_eq!(engine.budget().unwrap(), 7);
    }

    /// The authority clamps the budget to the valid range (the engine's invariant
    /// holds through the command plane, not just at the UI seam).
    #[test]
    fn set_budget_authority_clamps_to_valid_range() {
        let engine = PulseEngine::in_memory().unwrap();
        let submit = build_set_budget_submit("cmd-budget-2", usize::MAX).unwrap();

        let receipt = service_set_budget(&engine, &submit);
        assert_eq!(receipt.outcome, ReceiptOutcome::Applied);
        // Clamped to MAX_BUDGET (whatever the engine's ceiling is) — not usize::MAX.
        let applied = engine.budget().unwrap();
        assert!(applied < usize::MAX, "authority must clamp the budget");
        assert_eq!(receipt.payload_hash.as_deref(), Some(format!("budget:{applied}")).as_deref());
    }

    /// A malformed payload (wrong payload_type) fails closed as a `rejected`
    /// receipt with the command id, so the client resolves rather than hanging.
    #[test]
    fn set_budget_unknown_payload_type_fails_closed() {
        let engine = PulseEngine::in_memory().unwrap();
        let mut submit = build_set_budget_submit("cmd-budget-3", 3).unwrap();
        submit.payload_type = "community-pulse.something_else.v1".to_string();

        let receipt = service_set_budget(&engine, &submit);
        assert_eq!(receipt.outcome, ReceiptOutcome::Rejected);
        assert_eq!(receipt.causation_id, submit.command_id);
        assert!(receipt.reason.as_deref().unwrap().contains("payload_type"));
    }
}
