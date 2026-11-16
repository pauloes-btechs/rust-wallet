//! # rw-replay
//!
//! A [`NodeApi`] that answers from real litd v0.17.8 responses recorded on
//! regtest (`fixtures/litd-regtest.json`, recorded for Sovereign Wallet's
//! desktop demo; see `docs/UPSTREAM_WATCH.md`). It powers:
//!
//! * the hosted web demo (the Leptos UI compiled to WASM, served by Vercel),
//!   which therefore makes **no network calls and holds no keys**;
//! * the contract test: every recorded route must deserialize into the
//!   `rw-core` views, so the view contract cannot silently drift from what
//!   litd actually returns.
//!
//! Writes are simulated: they return the recorded response for that route
//! and change nothing.

#![forbid(unsafe_code)]

use rw_core::views::*;
use rw_core::{ApiError, ApiResult, Capability, CapabilitySet, NodeApi};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The raw recording, embedded at compile time.
pub const FIXTURE_JSON: &str = include_str!("../fixtures/litd-regtest.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMeta {
    pub recorded_at: String,
    pub network: String,
    pub source: String,
    pub litd: String,
    pub lnd: String,
    pub alias: String,
    pub exchanges: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct Route {
    status: u16,
    body: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct Recording {
    meta: RecordingMeta,
    routes: BTreeMap<String, Route>,
}

fn recording() -> &'static Recording {
    static R: OnceLock<Recording> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(FIXTURE_JSON).expect("embedded fixture is valid JSON"))
}

/// Every route in the recording, e.g. `"GET /channels"`.
pub fn routes() -> impl Iterator<Item = &'static str> {
    recording().routes.keys().map(String::as_str)
}

pub fn meta() -> &'static RecordingMeta {
    &recording().meta
}

/// Typed body of one recorded route.
pub fn body<T: DeserializeOwned>(route: &str) -> ApiResult<T> {
    let r = recording()
        .routes
        .get(route)
        .ok_or_else(|| ApiError::Unsupported(format!("{route} is not in the recording")))?;
    if r.status != 200 {
        return Err(ApiError::Rejected(format!("{route} recorded HTTP {}", r.status)));
    }
    serde_json::from_value(r.body.clone()).map_err(|e| ApiError::Other(format!("{route}: {e}")))
}

/// Replay backend. Cheap to construct; all instances share the parsed recording.
#[derive(Clone)]
pub struct ReplayBackend {
    caps: CapabilitySet,
}

impl Default for ReplayBackend {
    fn default() -> Self {
        use Capability::*;
        Self {
            caps: CapabilitySet::of(&[
                Read,
                OnchainReceive,
                OnchainSend,
                LightningReceive,
                LightningSend,
                Channels,
                ChannelBackups,
                Services,
                TaprootAssets,
                Loop,
                Faraday,
            ]),
        }
    }
}

impl NodeApi for ReplayBackend {
    fn capabilities(&self) -> &CapabilitySet {
        &self.caps
    }
    async fn state(&self) -> ApiResult<StateView> {
        body("GET /state")
    }
    async fn wallet_balance(&self) -> ApiResult<WalletBalanceView> {
        body("GET /onchain/balance")
    }
    async fn transactions(&self) -> ApiResult<TransactionsView> {
        body("GET /lnd/transactions")
    }
    async fn new_address(&self) -> ApiResult<AddressView> {
        body("POST /onchain/address")
    }
    async fn estimate_fee(&self, _r: &SendOnchainRequest) -> ApiResult<FeeEstimateView> {
        body("POST /onchain/estimate-fee")
    }
    async fn send_onchain(&self, _r: &SendOnchainRequest) -> ApiResult<TxidView> {
        body("POST /onchain/send")
    }
    async fn channels(&self) -> ApiResult<ChannelsView> {
        body("GET /channels")
    }
    async fn open_channel(&self, _r: &OpenChannelRequest) -> ApiResult<OpenChannelResult> {
        body("POST /channels")
    }
    async fn channel_backup(&self) -> ApiResult<ChannelBackupView> {
        body("GET /channel-backups")
    }
    async fn peers(&self) -> ApiResult<PeersView> {
        body("GET /lnd/peers")
    }
    async fn payments(&self) -> ApiResult<PaymentsView> {
        body("GET /payments")
    }
    async fn invoices(&self) -> ApiResult<InvoicesView> {
        body("GET /lnd/invoices")
    }
    async fn create_invoice(&self, _r: &CreateInvoiceRequest) -> ApiResult<CreatedInvoiceView> {
        body("POST /invoices")
    }
    async fn decode_invoice(&self, _pr: &str) -> ApiResult<DecodedInvoiceView> {
        body("POST /invoices/decode")
    }
    async fn pay_invoice(&self, _r: &PayRequest) -> ApiResult<PaymentOutcomeView> {
        body("POST /payments")
    }
    async fn diagnostics(&self) -> ApiResult<DiagnosticsView> {
        body("GET /diagnostics")
    }
    async fn services(&self) -> ApiResult<ServicesView> {
        body("GET /services")
    }
    async fn assets(&self) -> ApiResult<AssetsView> {
        body("GET /assets")
    }
    async fn swaps(&self) -> ApiResult<SwapsView> {
        body("GET /loop/swaps")
    }
}

/// Simulated dispatcher so the demo runs the *real* payment engine:
/// prepare = the recorded decode's payment hash, dispatch = the recorded
/// payment outcome, lookup = the recorded payments list.
#[derive(Clone, Default)]
pub struct ReplayDispatcher;

impl rw_payments::Dispatcher for ReplayDispatcher {
    async fn prepare(&self, i: &rw_payments::PaymentIntent) -> Result<String, String> {
        if i.kind == rw_payments::PaymentKind::Onchain {
            return Ok(format!("rw-intent:{}", i.intent_id));
        }
        body::<DecodedInvoiceView>("POST /invoices/decode").map(|d| d.payment_hash_hex).map_err(|e| e.to_string())
    }

    async fn dispatch(&self, i: &rw_payments::PaymentIntent, backend_ref: &str) -> rw_payments::DispatchOutcome {
        if i.kind == rw_payments::PaymentKind::Onchain {
            return match body::<TxidView>("POST /onchain/send") {
                Ok(t) => rw_payments::DispatchOutcome::Settled { backend_ref: t.txid },
                Err(e) => rw_payments::DispatchOutcome::DefinitiveFailure { reason: e.to_string() },
            };
        }
        match body::<PaymentOutcomeView>("POST /payments") {
            Ok(o) if o.status == PaymentOutcomeStatus::Succeeded => {
                rw_payments::DispatchOutcome::Settled { backend_ref: backend_ref.to_string() }
            }
            Ok(o) if o.status == PaymentOutcomeStatus::Failed => rw_payments::DispatchOutcome::DefinitiveFailure {
                reason: o.failure_reason.unwrap_or_else(|| "failed".into()),
            },
            _ => rw_payments::DispatchOutcome::Sent,
        }
    }

    async fn lookup(&self, i: &rw_payments::PaymentIntent) -> rw_payments::ReconcileResult {
        let Some(hash) = i.backend_ref.as_deref() else { return rw_payments::ReconcileResult::NotFound };
        match body::<PaymentsView>("GET /payments") {
            Ok(p) => match p.payments.iter().find(|p| p.payment_hash == hash) {
                Some(p) if p.status == "succeeded" => {
                    rw_payments::ReconcileResult::Settled { backend_ref: hash.to_string() }
                }
                Some(p) if p.status == "failed" => {
                    rw_payments::ReconcileResult::Failed { reason: p.failure_reason.clone() }
                }
                Some(_) => rw_payments::ReconcileResult::Unknown,
                None => rw_payments::ReconcileResult::NotFound,
            },
            Err(_) => rw_payments::ReconcileResult::Unknown,
        }
    }
}
