use crate::{
    IntentState, IntentStore, InvalidTransition, PaymentIntent, PaymentKind, ReconcileResult, RecoveryOutcome,
    StoreError,
};
use std::future::Future;
use thiserror::Error;

/// Outcome of ONE dispatch attempt. `Sent` and `Timeout` are deliberately
/// ambiguous: the intent stays `InFlight` and recovery reconciles it later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Settled { backend_ref: String },
    Sent,
    Timeout,
    DefinitiveFailure { reason: String },
}

/// Backend port. `dispatch` is called at most once per backend ref;
/// `lookup` is the ONLY method recovery may call.
pub trait Dispatcher {
    /// Compute the backend reference (payment hash, txid) *before* the network
    /// send: decoding the invoice, or building and signing the transaction.
    fn prepare(&self, intent: &PaymentIntent) -> impl Future<Output = Result<String, String>>;
    fn dispatch(&self, intent: &PaymentIntent, backend_ref: &str) -> impl Future<Output = DispatchOutcome>;
    /// Look up by payment hash / txid. Never sends anything.
    fn lookup(&self, intent: &PaymentIntent) -> impl Future<Output = ReconcileResult>;
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Transition(#[from] InvalidTransition),
    #[error("no persisted payment intent with id {0:?}")]
    UnknownIntent(String),
    #[error("payment {intent_id} rejected by spend policy: {reason}")]
    Rejected { intent_id: String, reason: String },
    #[error("payment {intent_id} could not be prepared: {reason}")]
    PrepareFailed { intent_id: String, reason: String },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Interrupted intents reconciled to a definite state (incl. never-sent → failed).
    pub reconciled: Vec<String>,
    /// Interrupted intents whose fate is unknown: they need a human.
    pub needs_recovery: Vec<String>,
}

type Policy = Box<dyn Fn(&PaymentIntent) -> Result<(), String> + Send + Sync>;
type Clock = Box<dyn Fn() -> String + Send + Sync>;
type IdGen = Box<dyn Fn() -> String + Send + Sync>;

/// Orchestrates the state machine over a durable store.
pub struct PaymentEngine<S, D> {
    store: S,
    dispatcher: D,
    new_id: IdGen,
    now: Clock,
    policy: Option<Policy>,
}

impl<S: IntentStore, D: Dispatcher> PaymentEngine<S, D> {
    /// `new_id` and `now` are injected (UUIDv4 and RFC 3339 in production):
    /// no hidden globals, so tests are deterministic.
    pub fn new(
        store: S,
        dispatcher: D,
        new_id: impl Fn() -> String + Send + Sync + 'static,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self { store, dispatcher, new_id: Box::new(new_id), now: Box::new(now), policy: None }
    }

    /// Spend/fee policy, enforced at authorization: rejecting here fails the
    /// intent before anything is signed or sent.
    pub fn with_policy(mut self, p: impl Fn(&PaymentIntent) -> Result<(), String> + Send + Sync + 'static) -> Self {
        self.policy = Some(Box::new(p));
        self
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn dispatcher(&self) -> &D {
        &self.dispatcher
    }

    /// Create → authorize. Both states are persisted.
    pub fn submit(
        &self,
        kind: PaymentKind,
        destination: &str,
        amount_sat: Option<u64>,
        max_fee_sat: Option<u64>,
    ) -> Result<PaymentIntent, EngineError> {
        self.submit_with_rate(kind, destination, amount_sat, max_fee_sat, None)
    }

    /// [`Self::submit`] with an explicit on-chain fee rate.
    pub fn submit_with_rate(
        &self,
        kind: PaymentKind,
        destination: &str,
        amount_sat: Option<u64>,
        max_fee_sat: Option<u64>,
        sat_per_vbyte: Option<u64>,
    ) -> Result<PaymentIntent, EngineError> {
        let mut draft = PaymentIntent::new((self.new_id)(), kind, destination, amount_sat, max_fee_sat);
        draft.sat_per_vbyte = sat_per_vbyte;
        self.store.save(&draft)?;
        if let Some(policy) = &self.policy {
            if let Err(reason) = policy(&draft) {
                self.store.save(&draft.fail(reason.clone())?)?;
                return Err(EngineError::Rejected { intent_id: draft.intent_id, reason });
            }
        }
        let authorized = draft.authorize()?;
        self.store.save(&authorized)?;
        Ok(authorized)
    }

    /// Sign → persist `InFlight` → dispatch exactly once → record the outcome.
    pub async fn execute(&self, intent_id: &str) -> Result<PaymentIntent, EngineError> {
        let signing = self.get(intent_id)?.begin_signing()?;
        self.store.save(&signing)?;

        let backend_ref = match self.dispatcher.prepare(&signing).await {
            Ok(r) => r,
            Err(reason) => {
                // Nothing left the device: a local, definitive failure.
                self.store.save(&signing.fail(reason.clone())?)?;
                return Err(EngineError::PrepareFailed { intent_id: intent_id.into(), reason });
            }
        };

        // PERSIST BEFORE DISPATCH. From here on a crash is reconciled on
        // startup, never blindly retried.
        let in_flight = signing.dispatch(&backend_ref, &(self.now)())?;
        self.store.save(&in_flight)?;

        let next = match self.dispatcher.dispatch(&in_flight, &backend_ref).await {
            DispatchOutcome::Settled { backend_ref } => {
                in_flight.reconcile(&ReconcileResult::Settled { backend_ref }, &(self.now)())?
            }
            DispatchOutcome::DefinitiveFailure { reason } => {
                in_flight.reconcile(&ReconcileResult::Failed { reason: Some(reason) }, &(self.now)())?
            }
            DispatchOutcome::Sent | DispatchOutcome::Timeout => return Ok(in_flight),
        };
        self.store.save(&next)?;
        Ok(next)
    }

    /// Startup crash recovery: every `Signing`/`InFlight` intent is looked up
    /// and reconciled. This method never calls `Dispatcher::dispatch`.
    pub async fn recover_interrupted(&self) -> Result<RecoveryReport, EngineError> {
        let mut report = RecoveryReport::default();
        for intent in self.store.list()? {
            if !intent.state.is_interrupted() {
                continue;
            }
            // A `Signing` intent never got a backend ref persisted, so nothing
            // can have been sent for it: treat as not-found without asking.
            let result = if intent.state == IntentState::Signing {
                ReconcileResult::NotFound
            } else {
                self.dispatcher.lookup(&intent).await
            };
            let next = intent.reconcile(&result, &(self.now)())?;
            self.store.save(&next)?;
            if next.state == IntentState::NeedsRecovery {
                report.needs_recovery.push(next.intent_id);
            } else {
                report.reconciled.push(next.intent_id);
            }
        }
        Ok(report)
    }

    pub fn resolve_needs_recovery(
        &self,
        intent_id: &str,
        outcome: &RecoveryOutcome,
    ) -> Result<PaymentIntent, EngineError> {
        let next = self.get(intent_id)?.resolve(outcome, &(self.now)())?;
        self.store.save(&next)?;
        Ok(next)
    }

    pub fn get(&self, intent_id: &str) -> Result<PaymentIntent, EngineError> {
        self.store.get(intent_id)?.ok_or_else(|| EngineError::UnknownIntent(intent_id.into()))
    }

    pub fn list(&self) -> Result<Vec<PaymentIntent>, EngineError> {
        Ok(self.store.list()?)
    }
}
