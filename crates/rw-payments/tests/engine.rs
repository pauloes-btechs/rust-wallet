//! Engine-level safety tests. The crash-recovery block is release-blocking:
//! a failure here means the wallet could pay the same invoice twice.

use rw_payments::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    dispatches: AtomicU32,
    lookups: AtomicU32,
    outcome: Mutex<Option<DispatchOutcome>>,
    lookup_result: Mutex<Option<ReconcileResult>>,
    /// Store snapshot taken *inside* dispatch: proves persist-before-dispatch.
    seen_state_at_dispatch: Mutex<Vec<Option<IntentState>>>,
    store_probe: Mutex<Option<Arc<dyn IntentStore>>>,
}

struct FakeRef(Arc<Fake>);

impl Dispatcher for FakeRef {
    async fn prepare(&self, intent: &PaymentIntent) -> Result<String, String> {
        if intent.destination == "unpreparable" {
            return Err("cannot decode".into());
        }
        Ok(format!("hash-{}", intent.intent_id))
    }

    async fn dispatch(&self, intent: &PaymentIntent, _r: &str) -> DispatchOutcome {
        self.0.dispatches.fetch_add(1, Ordering::SeqCst);
        if let Some(store) = self.0.store_probe.lock().unwrap().as_ref() {
            let s = store.get(&intent.intent_id).unwrap().map(|i| i.state);
            self.0.seen_state_at_dispatch.lock().unwrap().push(s);
        }
        self.0.outcome.lock().unwrap().clone().unwrap_or(DispatchOutcome::Sent)
    }

    async fn lookup(&self, _i: &PaymentIntent) -> ReconcileResult {
        self.0.lookups.fetch_add(1, Ordering::SeqCst);
        self.0.lookup_result.lock().unwrap().clone().unwrap_or(ReconcileResult::Unknown)
    }
}

/// Shares one store between the engine and the probe.
struct Shared(Arc<dyn IntentStore>);
impl IntentStore for Shared {
    fn save(&self, i: &PaymentIntent) -> Result<(), StoreError> {
        self.0.save(i)
    }
    fn get(&self, id: &str) -> Result<Option<PaymentIntent>, StoreError> {
        self.0.get(id)
    }
    fn list(&self) -> Result<Vec<PaymentIntent>, StoreError> {
        self.0.list()
    }
}

fn ids() -> impl Fn() -> String + Send + Sync {
    let n = AtomicU32::new(0);
    move || format!("intent-{}", n.fetch_add(1, Ordering::SeqCst))
}

fn clock() -> impl Fn() -> String + Send + Sync {
    || "2026-11-16T00:00:00Z".to_string()
}

fn engine(store: Arc<dyn IntentStore>, fake: Arc<Fake>) -> PaymentEngine<Shared, FakeRef> {
    *fake.store_probe.lock().unwrap() = Some(store.clone());
    PaymentEngine::new(Shared(store), FakeRef(fake), ids(), clock())
}

#[tokio::test]
async fn persists_in_flight_before_dispatch() {
    let fake = Arc::new(Fake::default());
    *fake.outcome.lock().unwrap() = Some(DispatchOutcome::Settled { backend_ref: "hash-intent-0".into() });
    let e = engine(Arc::new(MemoryStore::default()), fake.clone());
    let i = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, Some(10)).unwrap();
    assert_eq!(e.get(&i.intent_id).unwrap().state, IntentState::Authorized);
    let done = e.execute(&i.intent_id).await.unwrap();
    assert_eq!(done.state, IntentState::Settled);
    assert_eq!(*fake.seen_state_at_dispatch.lock().unwrap(), vec![Some(IntentState::InFlight)]);
}

#[tokio::test]
async fn execute_twice_never_dispatches_twice() {
    let fake = Arc::new(Fake::default());
    let e = engine(Arc::new(MemoryStore::default()), fake.clone());
    let i = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
    assert_eq!(e.execute(&i.intent_id).await.unwrap().state, IntentState::InFlight);
    assert!(e.execute(&i.intent_id).await.is_err(), "second execute must be refused");
    assert_eq!(fake.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn policy_rejection_fails_before_signing() {
    let fake = Arc::new(Fake::default());
    let e = engine(Arc::new(MemoryStore::default()), fake.clone()).with_policy(|i| match i.max_fee_sat {
        Some(f) if f <= 100 => Ok(()),
        _ => Err("fee cap required (≤100 sats)".into()),
    });
    let err = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap_err();
    assert!(matches!(err, EngineError::Rejected { .. }));
    let all = e.list().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, IntentState::Failed);
    assert_eq!(fake.dispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepare_failure_is_local_and_terminal() {
    let fake = Arc::new(Fake::default());
    let e = engine(Arc::new(MemoryStore::default()), fake.clone());
    let i = e.submit(PaymentKind::LightningInvoice, "unpreparable", None, None).unwrap();
    assert!(matches!(e.execute(&i.intent_id).await, Err(EngineError::PrepareFailed { .. })));
    assert_eq!(e.get(&i.intent_id).unwrap().state, IntentState::Failed);
    assert_eq!(fake.dispatches.load(Ordering::SeqCst), 0);
}

/// The guard on-chain sends use: an ambiguous send blocks the next one
/// until it is reconciled, and settled or other nodes' intents do not.
#[tokio::test]
async fn unresolved_finds_only_open_intents_of_this_node_and_kind() {
    let store: Arc<dyn IntentStore> = Arc::new(MemoryStore::default());
    let fake = Arc::new(Fake::default());
    let e = engine(store.clone(), fake.clone());
    e.set_node("node-a");
    *fake.outcome.lock().unwrap() = Some(DispatchOutcome::Timeout);
    let i = e.submit(PaymentKind::Onchain, "bcrt1…", Some(1000), None).unwrap();
    e.execute(&i.intent_id).await.unwrap();
    assert_eq!(e.unresolved(PaymentKind::Onchain).unwrap().map(|u| u.intent_id), Some(i.intent_id.clone()));
    assert_eq!(e.unresolved(PaymentKind::LightningInvoice).unwrap(), None);

    let other = engine(store.clone(), fake.clone());
    other.set_node("node-b");
    assert_eq!(other.unresolved(PaymentKind::Onchain).unwrap(), None);

    // Unknown lookup → needs-recovery still blocks; resolving it unblocks.
    e.recover_interrupted().await.unwrap();
    assert!(e.unresolved(PaymentKind::Onchain).unwrap().is_some());
    e.resolve_needs_recovery(&i.intent_id, &RecoveryOutcome::Settled { backend_ref: "tx".into() }).unwrap();
    assert_eq!(e.unresolved(PaymentKind::Onchain).unwrap(), None);
}

mod crash_recovery {
    use super::*;

    /// Crash after the send: the restarted engine looks up, never re-sends.
    #[tokio::test]
    async fn in_flight_is_reconciled_not_redispatched() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("intents.db");
        let fake = Arc::new(Fake::default());
        let id = {
            let e = engine(Arc::new(SqliteStore::open(&db).unwrap()), fake.clone());
            let i = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
            *fake.outcome.lock().unwrap() = Some(DispatchOutcome::Timeout);
            e.execute(&i.intent_id).await.unwrap();
            i.intent_id
        }; // engine and connection dropped: the "crash"

        *fake.lookup_result.lock().unwrap() = Some(ReconcileResult::Settled { backend_ref: "hash-x".into() });
        let e = engine(Arc::new(SqliteStore::open(&db).unwrap()), fake.clone());
        let report = e.recover_interrupted().await.unwrap();
        assert_eq!(report.reconciled, vec![id.clone()]);
        assert_eq!(e.get(&id).unwrap().state, IntentState::Settled);
        assert_eq!(fake.dispatches.load(Ordering::SeqCst), 1, "recovery must never dispatch");
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_becomes_needs_recovery_then_manual_resolution() {
        let fake = Arc::new(Fake::default());
        let e = engine(Arc::new(MemoryStore::default()), fake.clone());
        let i = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
        e.execute(&i.intent_id).await.unwrap();
        let report = e.recover_interrupted().await.unwrap();
        assert_eq!(report.needs_recovery, vec![i.intent_id.clone()]);
        // Running recovery again does nothing to a needs-recovery intent.
        assert_eq!(e.recover_interrupted().await.unwrap(), RecoveryReport::default());
        let r = e
            .resolve_needs_recovery(&i.intent_id, &RecoveryOutcome::Failed { reason: Some("expired".into()) })
            .unwrap();
        assert_eq!(r.state, IntentState::Failed);
        assert_eq!(fake.dispatches.load(Ordering::SeqCst), 1);
    }

    /// Crash between "persist in-flight" and the network call: the backend has
    /// never seen it. The intent fails instead of sitting in `Authorized`
    /// with nothing to re-execute it.
    #[tokio::test]
    async fn not_found_fails_as_never_reached_node() {
        let fake = Arc::new(Fake::default());
        let store: Arc<dyn IntentStore> = Arc::new(MemoryStore::default());
        let e = engine(store.clone(), fake.clone());
        let i = e.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
        let in_flight = i.begin_signing().unwrap().dispatch("hash-intent-0", "t").unwrap();
        store.save(&in_flight).unwrap();

        *fake.lookup_result.lock().unwrap() = Some(ReconcileResult::NotFound);
        let report = e.recover_interrupted().await.unwrap();
        assert_eq!(report.reconciled, vec![i.intent_id.clone()]);
        let failed = e.get(&i.intent_id).unwrap();
        assert_eq!((failed.state, failed.failure_reason.as_deref()), (IntentState::Failed, Some(NEVER_REACHED_NODE)));
        assert!(e.execute(&i.intent_id).await.is_err(), "a failed intent is never executed again");
        assert_eq!(fake.dispatches.load(Ordering::SeqCst), 0);
    }

    /// Crash while signing: nothing had a backend ref, so no lookup is needed.
    #[tokio::test]
    async fn signing_fails_without_lookup() {
        let fake = Arc::new(Fake::default());
        let store: Arc<dyn IntentStore> = Arc::new(MemoryStore::default());
        let e = engine(store.clone(), fake.clone());
        let i = e.submit(PaymentKind::Onchain, "bcrt1…", Some(1000), None).unwrap();
        store.save(&i.begin_signing().unwrap()).unwrap();
        e.recover_interrupted().await.unwrap();
        assert_eq!(e.get(&i.intent_id).unwrap().state, IntentState::Failed);
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 0);
    }

    /// One journal, two nodes: recovering against node B must not touch
    /// node A's in-flight intent (B's lookup would say not-found).
    #[tokio::test]
    async fn recovery_skips_other_nodes_intents() {
        let store: Arc<dyn IntentStore> = Arc::new(MemoryStore::default());
        let fake = Arc::new(Fake::default());
        let a = engine(store.clone(), fake.clone());
        a.set_node("node-a");
        let i = a.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
        assert_eq!(i.node.as_deref(), Some("node-a"));
        a.execute(&i.intent_id).await.unwrap();

        *fake.lookup_result.lock().unwrap() = Some(ReconcileResult::NotFound);
        let b = engine(store.clone(), fake.clone());
        b.set_node("node-b");
        assert_eq!(b.recover_interrupted().await.unwrap(), RecoveryReport::default());
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 0);
        assert_eq!(store.get(&i.intent_id).unwrap().unwrap().state, IntentState::InFlight);

        *fake.lookup_result.lock().unwrap() = Some(ReconcileResult::Settled { backend_ref: "h".into() });
        assert_eq!(a.recover_interrupted().await.unwrap().reconciled, vec![i.intent_id.clone()]);
        assert_eq!(store.get(&i.intent_id).unwrap().unwrap().state, IntentState::Settled);
    }

    /// Intents journaled before nodes were recorded are still recovered.
    #[tokio::test]
    async fn legacy_intent_without_node_is_still_recovered() {
        let store: Arc<dyn IntentStore> = Arc::new(MemoryStore::default());
        let fake = Arc::new(Fake::default());
        let legacy = engine(store.clone(), fake.clone());
        let i = legacy.submit(PaymentKind::LightningInvoice, "lnbcrt…", None, None).unwrap();
        legacy.execute(&i.intent_id).await.unwrap();
        assert_eq!(store.get(&i.intent_id).unwrap().unwrap().node, None);

        *fake.lookup_result.lock().unwrap() = Some(ReconcileResult::Settled { backend_ref: "h".into() });
        let e = engine(store.clone(), fake.clone());
        e.set_node("node-a");
        assert_eq!(e.recover_interrupted().await.unwrap().reconciled, vec![i.intent_id]);
    }

    #[tokio::test]
    async fn sqlite_preserves_order_and_body() {
        let s = SqliteStore::in_memory().unwrap();
        for n in 0..5 {
            s.save(&PaymentIntent::new(format!("z{n}"), PaymentKind::Onchain, "a", Some(n), None)).unwrap();
        }
        let mut i = s.get("z2").unwrap().unwrap();
        i = i.authorize().unwrap();
        s.save(&i).unwrap();
        let ids: Vec<_> = s.list().unwrap().into_iter().map(|i| i.intent_id).collect();
        assert_eq!(ids, ["z0", "z1", "z2", "z3", "z4"]);
        assert_eq!(s.get("z2").unwrap().unwrap().state, IntentState::Authorized);
    }
}
