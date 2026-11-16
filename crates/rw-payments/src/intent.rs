use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaymentKind {
    LightningInvoice,
    Onchain,
}

/// `Draft | Authorized | Signing` are local and reversible.
/// `InFlight | Settled` have been dispatched: reconcile, never re-dispatch.
/// `Failed | NeedsRecovery` are terminal or manual.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntentState {
    Draft,
    Authorized,
    Signing,
    InFlight,
    Settled,
    Failed,
    NeedsRecovery,
}

impl IntentState {
    /// States recovery must reconcile on startup.
    pub fn is_interrupted(self) -> bool {
        matches!(self, IntentState::Signing | IntentState::InFlight)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, IntentState::Settled | IntentState::Failed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptResult {
    Dispatched,
    Timeout,
    Rejected,
    Resolved,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    /// RFC 3339 timestamp from the engine's clock.
    pub at: String,
    pub result: AttemptResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentIntent {
    /// Generated once, persisted in `Authorized` before dispatch.
    pub intent_id: String,
    pub kind: PaymentKind,
    /// BOLT-11 invoice or on-chain address, validated before authorize.
    pub destination: String,
    /// Amount for zero-amount invoices and on-chain sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_sat: Option<u64>,
    /// Spend policy: hard cap on fees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fee_sat: Option<u64>,
    /// On-chain fee rate chosen by the user (None = node's estimate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sat_per_vbyte: Option<u64>,
    pub state: IntentState,
    /// Payment hash / txid: the reconciliation key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    pub attempts: Vec<Attempt>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid payment-intent transition: cannot {action} from {from:?}")]
pub struct InvalidTransition {
    pub from: IntentState,
    pub action: &'static str,
}

/// What a backend *lookup* says about an interrupted intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileResult {
    Settled {
        backend_ref: String,
    },
    Failed {
        reason: Option<String>,
    },
    /// The backend knows the payment but it has not resolved, or the lookup itself failed.
    Unknown,
    /// Nothing ever reached the backend: one re-signed retry is safe.
    NotFound,
}

/// Manual resolution of a `NeedsRecovery` intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Settled { backend_ref: String },
    Failed { reason: Option<String> },
}

type T = Result<PaymentIntent, InvalidTransition>;

impl PaymentIntent {
    pub fn new(
        intent_id: impl Into<String>,
        kind: PaymentKind,
        destination: impl Into<String>,
        amount_sat: Option<u64>,
        max_fee_sat: Option<u64>,
    ) -> Self {
        Self {
            intent_id: intent_id.into(),
            kind,
            destination: destination.into(),
            amount_sat,
            max_fee_sat,
            sat_per_vbyte: None,
            state: IntentState::Draft,
            backend_ref: None,
            failure_reason: None,
            attempts: Vec::new(),
        }
    }

    fn guard(&self, ok: bool, action: &'static str) -> Result<(), InvalidTransition> {
        if ok {
            Ok(())
        } else {
            Err(InvalidTransition { from: self.state, action })
        }
    }

    fn to(&self, state: IntentState) -> PaymentIntent {
        PaymentIntent { state, ..self.clone() }
    }

    pub fn authorize(&self) -> T {
        self.guard(self.state == IntentState::Draft, "authorize")?;
        Ok(self.to(IntentState::Authorized))
    }

    pub fn begin_signing(&self) -> T {
        self.guard(self.state == IntentState::Authorized, "sign")?;
        Ok(self.to(IntentState::Signing))
    }

    /// Calling this on an intent that is already in flight is the
    /// blind-re-dispatch defect: it is an error.
    pub fn dispatch(&self, backend_ref: &str, at: &str) -> T {
        self.guard(self.state == IntentState::Signing, "dispatch")?;
        let mut next = self.to(IntentState::InFlight);
        next.backend_ref = Some(backend_ref.to_string());
        next.attempts.push(Attempt {
            at: at.to_string(),
            result: AttemptResult::Dispatched,
            backend_ref: Some(backend_ref.to_string()),
        });
        Ok(next)
    }

    /// Recovery rule: reconcile, never re-dispatch.
    pub fn reconcile(&self, result: &ReconcileResult, at: &str) -> T {
        self.guard(self.state.is_interrupted(), "reconcile")?;
        let (state, attempt) = match result {
            ReconcileResult::Settled { .. } => (IntentState::Settled, AttemptResult::Resolved),
            ReconcileResult::Failed { .. } => (IntentState::Failed, AttemptResult::Rejected),
            ReconcileResult::NotFound => (IntentState::Authorized, AttemptResult::Unknown),
            ReconcileResult::Unknown => (IntentState::NeedsRecovery, AttemptResult::Unknown),
        };
        let mut next = self.to(state);
        next.attempts.push(Attempt { at: at.to_string(), result: attempt, backend_ref: None });
        match result {
            ReconcileResult::Settled { backend_ref } => next.backend_ref = Some(backend_ref.clone()),
            ReconcileResult::Failed { reason } => next.failure_reason = reason.clone(),
            _ => {}
        }
        Ok(next)
    }

    /// Terminal local failure (policy rejection, validation failure).
    pub fn fail(&self, reason: impl Into<String>) -> T {
        self.guard(matches!(self.state, IntentState::Draft | IntentState::Authorized | IntentState::Signing), "fail")?;
        let mut next = self.to(IntentState::Failed);
        next.failure_reason = Some(reason.into());
        Ok(next)
    }

    /// The only way out of `NeedsRecovery`.
    pub fn resolve(&self, outcome: &RecoveryOutcome, at: &str) -> T {
        self.guard(self.state == IntentState::NeedsRecovery, "resolve")?;
        let mut next = match outcome {
            RecoveryOutcome::Settled { backend_ref } => {
                let mut n = self.to(IntentState::Settled);
                n.backend_ref = Some(backend_ref.clone());
                n
            }
            RecoveryOutcome::Failed { reason } => {
                let mut n = self.to(IntentState::Failed);
                n.failure_reason = reason.clone();
                n
            }
        };
        next.attempts.push(Attempt { at: at.to_string(), result: AttemptResult::Resolved, backend_ref: None });
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use IntentState::*;

    fn draft() -> PaymentIntent {
        PaymentIntent::new("i1", PaymentKind::LightningInvoice, "lnbcrt1…", None, Some(10))
    }

    fn in_flight() -> PaymentIntent {
        draft().authorize().unwrap().begin_signing().unwrap().dispatch("hash", "t1").unwrap()
    }

    #[test]
    fn happy_path() {
        let i = in_flight();
        assert_eq!(i.state, InFlight);
        assert_eq!(i.backend_ref.as_deref(), Some("hash"));
        let s = i.reconcile(&ReconcileResult::Settled { backend_ref: "hash".into() }, "t2").unwrap();
        assert_eq!(s.state, Settled);
        assert_eq!(s.attempts.len(), 2);
    }

    #[test]
    fn re_dispatch_is_an_error() {
        let e = in_flight().dispatch("hash", "t2").unwrap_err();
        assert_eq!(e, InvalidTransition { from: InFlight, action: "dispatch" });
    }

    #[test]
    fn every_illegal_transition_is_rejected() {
        let all = [Draft, Authorized, Signing, InFlight, Settled, Failed, NeedsRecovery];
        for s in all {
            let i = PaymentIntent { state: s, ..draft() };
            assert_eq!(i.authorize().is_ok(), s == Draft, "authorize from {s:?}");
            assert_eq!(i.begin_signing().is_ok(), s == Authorized, "sign from {s:?}");
            assert_eq!(i.dispatch("h", "t").is_ok(), s == Signing, "dispatch from {s:?}");
            assert_eq!(i.reconcile(&ReconcileResult::Unknown, "t").is_ok(), s.is_interrupted(), "reconcile from {s:?}");
            assert_eq!(i.fail("x").is_ok(), matches!(s, Draft | Authorized | Signing), "fail from {s:?}");
            assert_eq!(
                i.resolve(&RecoveryOutcome::Failed { reason: None }, "t").is_ok(),
                s == NeedsRecovery,
                "resolve from {s:?}"
            );
        }
    }

    #[test]
    fn reconcile_outcomes() {
        let i = in_flight();
        assert_eq!(i.reconcile(&ReconcileResult::NotFound, "t").unwrap().state, Authorized);
        assert_eq!(i.reconcile(&ReconcileResult::Unknown, "t").unwrap().state, NeedsRecovery);
        let f = i.reconcile(&ReconcileResult::Failed { reason: Some("no route".into()) }, "t").unwrap();
        assert_eq!((f.state, f.failure_reason.as_deref()), (Failed, Some("no route")));
    }

    #[test]
    fn terminal_states_are_absorbing() {
        let settled = in_flight().reconcile(&ReconcileResult::Settled { backend_ref: "h".into() }, "t").unwrap();
        assert!(settled.state.is_terminal());
        assert!(settled.dispatch("h", "t").is_err());
        assert!(settled.fail("x").is_err());
    }

    #[test]
    fn json_round_trip() {
        let i = in_flight();
        let j = serde_json::to_string(&i).unwrap();
        assert!(j.contains(r#""state":"in-flight""#));
        assert_eq!(serde_json::from_str::<PaymentIntent>(&j).unwrap(), i);
    }
}
