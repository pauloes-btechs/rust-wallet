//! Contract test: the rw-core views must deserialize every recorded litd
//! response. If litd changes a shape (or a view is edited carelessly), this
//! fails before the demo or the desktop app does.

use rw_core::views::*;
use rw_core::{destination, NodeApi};
use rw_replay::*;

fn check<T: serde::de::DeserializeOwned + serde::Serialize>(route: &str) {
    let v: T = body(route).unwrap_or_else(|e| panic!("{route}: {e}"));
    // And it survives a round trip through the wire format.
    let j = serde_json::to_string(&v).unwrap();
    serde_json::from_str::<T>(&j).unwrap_or_else(|e| panic!("{route} round trip: {e}"));
}

#[test]
fn every_modelled_route_deserializes() {
    check::<StateView>("GET /state");
    check::<WalletBalanceView>("GET /onchain/balance");
    check::<TransactionsView>("GET /lnd/transactions");
    check::<AddressView>("POST /onchain/address");
    check::<FeeEstimateView>("POST /onchain/estimate-fee");
    check::<TxidView>("POST /onchain/send");
    check::<ChannelsView>("GET /channels");
    check::<OpenChannelResult>("POST /channels");
    check::<ChannelBackupView>("GET /channel-backups");
    check::<PeersView>("GET /lnd/peers");
    check::<PaymentsView>("GET /payments");
    check::<InvoicesView>("GET /lnd/invoices");
    check::<CreatedInvoiceView>("POST /invoices");
    check::<DecodedInvoiceView>("POST /invoices/decode");
    check::<PaymentOutcomeView>("POST /payments");
    check::<DiagnosticsView>("GET /diagnostics");
    check::<ServicesView>("GET /services");
    check::<AssetsView>("GET /assets");
    check::<SwapsView>("GET /loop/swaps");
}

#[test]
fn recording_is_regtest() {
    assert_eq!(meta().network, "regtest");
    assert!(routes().count() >= 20);
}

/// Real invoices and addresses from the recording pass the offline gate as regtest.
#[test]
fn recorded_destinations_validate_as_regtest() {
    let inv: InvoicesView = body("GET /lnd/invoices").unwrap();
    for i in inv.invoices.iter().filter(|i| !i.payment_request.is_empty()) {
        let d = destination::parse_destination(&i.payment_request, Some(rw_core::Network::Regtest))
            .unwrap_or_else(|e| panic!("{}: {e}", i.payment_request));
        assert!(d.is_lightning());
    }
    let addr: AddressView = body("POST /onchain/address").unwrap();
    destination::parse_destination(&addr.address, Some(rw_core::Network::Regtest)).unwrap();
    let txs: TransactionsView = body("GET /lnd/transactions").unwrap();
    for a in txs.transactions.iter().flat_map(|t| &t.addresses) {
        destination::parse_destination(a, Some(rw_core::Network::Regtest)).unwrap_or_else(|e| panic!("{a}: {e}"));
    }
}

#[tokio::test]
async fn backend_answers_and_engine_settles_over_replay() {
    let b = ReplayBackend::default();
    let s = b.state().await.unwrap();
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.info.unwrap().network(), Some(rw_core::Network::Regtest));
    let ch = b.channels().await.unwrap();
    assert!(!ch.open.is_empty());

    let engine = rw_payments::PaymentEngine::new(
        rw_payments::MemoryStore::default(),
        ReplayDispatcher,
        || "demo-1".to_string(),
        || "2026-11-16T00:00:00Z".to_string(),
    );
    let i = engine.submit(rw_payments::PaymentKind::LightningInvoice, "lnbcrt…", None, Some(50)).unwrap();
    let done = engine.execute(&i.intent_id).await.unwrap();
    assert_eq!(done.state, rw_payments::IntentState::Settled);
}
