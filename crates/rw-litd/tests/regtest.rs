//! Live tests against the Docker regtest harness (harness/regtest).
//!
//! ```sh
//! harness/regtest/scripts/bootstrap.sh && source harness/regtest/.env.regtest
//! cargo test -p rw-litd --features regtest -- --test-threads=1
//! ```
//!
//! REGTEST ONLY. These pay real (regtest) invoices between two LNDs and
//! prove the payment engine never pays twice, including across a "crash".

#![cfg(feature = "regtest")]

use rw_core::views::*;
use rw_core::{ApiError, MainnetPolicy, Network, NodeApi};
use rw_litd::{ConnectConfig, LitdBackend, LndDispatcher};
use rw_payments::{
    DispatchOutcome, Dispatcher, IntentState, IntentStore, PaymentEngine, PaymentIntent, PaymentKind, ReconcileResult,
    SqliteStore,
};
use std::sync::Arc;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set: source harness/regtest/.env.regtest"))
}

fn cfg(who: &str) -> ConnectConfig {
    ConnectConfig {
        endpoint: env(&format!("RW_{who}_ENDPOINT")),
        tls_cert: env(&format!("RW_{who}_CERT")).into(),
        macaroon: env(&format!("RW_{who}_MACAROON")).into(),
        mainnet: MainnetPolicy::Refuse,
    }
}

async fn open(who: &str, store: Arc<dyn IntentStore>) -> LitdBackend {
    LitdBackend::open(&cfg(who), store).await.unwrap_or_else(|e| panic!("open {who}: {e}")).0
}

fn mem() -> Arc<dyn IntentStore> {
    Arc::new(rw_payments::MemoryStore::default())
}

async fn invoice(bob: &LitdBackend, sats: u64, memo: &str) -> String {
    bob.create_invoice(&CreateInvoiceRequest { amount_sat: sats, memo: memo.into(), expiry_secs: None })
        .await
        .unwrap()
        .payment_request
}

async fn payments_for(alice: &LitdBackend, hash: &str) -> Vec<PaymentView> {
    alice.payments().await.unwrap().payments.into_iter().filter(|p| p.payment_hash == hash).collect()
}

#[tokio::test]
async fn state_is_ready_on_regtest() {
    let alice = open("ALICE", mem()).await;
    let s = alice.state().await.unwrap();
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.info.unwrap().network(), Some(Network::Regtest));
    assert!(alice.channels().await.unwrap().open.iter().any(|c| c.remote_pubkey == env("RW_BOB_PUBKEY")));
}

#[tokio::test]
async fn pay_settles_and_a_second_pay_never_pays_twice() {
    let alice = open("ALICE", mem()).await;
    let bob = open("BOB", mem()).await;
    let pr = invoice(&bob, 12_000, "rw regtest pay").await;

    let first = alice
        .pay_invoice(&PayRequest { payment_request: pr.clone(), amount_sat: None, max_fee_sat: Some(50) })
        .await
        .unwrap();
    assert_eq!(first.status, PaymentOutcomeStatus::Succeeded, "{first:?}");

    // Same invoice again: LND refuses the duplicate, which the dispatcher
    // treats as ambiguous (never "failed"); recovery then reconciles it to
    // settled by looking the hash up. One payment, never two.
    let second =
        alice.pay_invoice(&PayRequest { payment_request: pr, amount_sat: None, max_fee_sat: Some(50) }).await.unwrap();
    assert_eq!(second.status, PaymentOutcomeStatus::NoResult, "{second:?}");
    let report = alice.engine().recover_interrupted().await.unwrap();
    assert_eq!(report.reconciled.len(), 1);
    let id = second.intent_id.unwrap();
    assert_eq!(alice.engine().get(&id).unwrap().state, IntentState::Settled);
    assert_eq!(payments_for(&alice, &first.payment_hash).await.iter().filter(|p| p.status == "succeeded").count(), 1);
}

/// Wraps the real dispatcher and pretends the process died right after
/// sending: the payment really goes out, but the engine never hears back.
struct CrashAfterSend(LndDispatcher);

impl Dispatcher for CrashAfterSend {
    async fn prepare(&self, i: &PaymentIntent) -> Result<String, String> {
        self.0.prepare(i).await
    }
    async fn dispatch(&self, i: &PaymentIntent, r: &str) -> DispatchOutcome {
        let _ = self.0.dispatch(i, r).await;
        DispatchOutcome::Timeout
    }
    async fn lookup(&self, i: &PaymentIntent) -> ReconcileResult {
        self.0.lookup(i).await
    }
}

#[tokio::test]
async fn crash_after_send_is_reconciled_on_restart_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("intents.db");
    let bob = open("BOB", mem()).await;
    let pr = invoice(&bob, 7_000, "rw crash drill").await;
    let client = rw_litd::connect(&cfg("ALICE")).await.unwrap();

    let id = {
        let engine = PaymentEngine::new(
            SqliteStore::open(&db).unwrap(),
            CrashAfterSend(LndDispatcher::new(client)),
            || "crash-drill-1".into(),
            || "2026-11-16T00:00:00Z".into(),
        );
        let i = engine.submit(PaymentKind::LightningInvoice, &pr, None, Some(50)).unwrap();
        assert_eq!(engine.execute(&i.intent_id).await.unwrap().state, IntentState::InFlight);
        i.intent_id
    }; // "crash"

    // Restart: open() reconciles interrupted intents before anything else.
    let (alice, report) = LitdBackend::open(&cfg("ALICE"), Arc::new(SqliteStore::open(&db).unwrap())).await.unwrap();
    assert_eq!(report.reconciled, vec![id.clone()]);
    let intent = alice.engine().get(&id).unwrap();
    assert_eq!(intent.state, IntentState::Settled);
    let hash = intent.backend_ref.unwrap();
    assert_eq!(payments_for(&alice, &hash).await.len(), 1, "exactly one payment attempt for this hash");
}

#[tokio::test]
async fn onchain_send_is_reconcilable_by_label() {
    let alice = open("ALICE", mem()).await;
    let bob = open("BOB", mem()).await;
    let addr = bob.new_address().await.unwrap().address;
    let req = SendOnchainRequest { address: addr, amount_sat: 50_000, sat_per_vbyte: Some(2) };
    let fee = alice.estimate_fee(&req).await.unwrap();
    assert!(fee.fee_sat > 0);
    let tx = alice.send_onchain(&req).await.unwrap();
    let intent = alice.engine().list().unwrap().into_iter().find(|i| i.kind == PaymentKind::Onchain).unwrap();
    assert_eq!(intent.state, IntentState::Settled);
    assert_eq!(intent.backend_ref.as_deref(), Some(tx.txid.as_str()));
    // The label lookup recovery would use finds the same txid.
    let probe = PaymentIntent { backend_ref: None, ..intent };
    let d = LndDispatcher::new(rw_litd::connect(&cfg("ALICE")).await.unwrap());
    assert_eq!(d.lookup(&probe).await, ReconcileResult::Settled { backend_ref: tx.txid });
}

#[tokio::test]
async fn wrong_network_invoice_is_refused_before_authorization() {
    let alice = open("ALICE", mem()).await;
    const MAINNET: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";
    let e = alice
        .pay_invoice(&PayRequest { payment_request: MAINNET.into(), amount_sat: None, max_fee_sat: None })
        .await
        .unwrap_err();
    assert!(matches!(e, ApiError::Invalid(_)), "{e:?}");
    assert!(alice.engine().list().unwrap().is_empty(), "nothing may be persisted for a refused destination");
}

#[tokio::test]
async fn read_surfaces_answer() {
    let alice = open("ALICE", mem()).await;
    alice.wallet_balance().await.unwrap();
    alice.transactions().await.unwrap();
    alice.peers().await.unwrap();
    alice.invoices().await.unwrap();
    alice.diagnostics().await.unwrap();
    assert!(!alice.channel_backup().await.unwrap().multi_chan_backup_hex.is_empty());
}

/// litd's embedded LND speaks the same API; the backend works unchanged.
#[tokio::test]
async fn litd_embedded_lnd_is_ready() {
    if std::env::var("RW_LITD_CERT").map(|p| !std::path::Path::new(&p).exists()).unwrap_or(true) {
        eprintln!("litd credentials not exported; skipping");
        return;
    }
    let litd = open("LITD", mem()).await;
    assert_eq!(litd.state().await.unwrap().phase, Phase::Ready);
}
