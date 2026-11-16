//! One API, two transports.
//!
//! * **Desktop** (inside Tauri): every call is a Tauri IPC `invoke` to the Rust
//!   shell, which talks gRPC to litd. The UI never sees a macaroon or a socket.
//! * **Demo** (any browser, e.g. the Vercel deployment): calls are answered by
//!   `rw-replay` from real litd regtest responses compiled into the WASM, and
//!   sends run through the real `rw-payments` engine over a replay
//!   dispatcher. No network, no keys, no real bitcoin.
//!
//! The mode is detected once at startup from `window.__TAURI__`.

use rw_core::views::*;
use rw_core::{ApiError, ApiResult, CapabilitySet, NodeApi};
use rw_payments::{MemoryStore, PaymentEngine, PaymentIntent, PaymentKind};
use rw_replay::{ReplayBackend, ReplayDispatcher};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Demo,
    Desktop,
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

thread_local! {
    static MODE: OnceCell<Mode> = const { OnceCell::new() };
    static DEMO_ENGINE: Rc<DemoEngine> = Rc::new(PaymentEngine::new(
        MemoryStore::default(),
        ReplayDispatcher,
        || {
            thread_local!(static N: Cell<u32> = const { Cell::new(0) });
            N.with(|n| { n.set(n.get() + 1); format!("demo-intent-{}", n.get()) })
        },
        now_iso,
    ));
}

type DemoEngine = PaymentEngine<MemoryStore, ReplayDispatcher>;

/// The demo's engine (single-threaded WASM: an `Rc` is enough).
fn demo_engine() -> Rc<DemoEngine> {
    DEMO_ENGINE.with(Rc::clone)
}

fn now_iso() -> String {
    #[cfg(target_family = "wasm")]
    {
        js_sys::Date::new_0().to_iso_string().into()
    }
    #[cfg(not(target_family = "wasm"))]
    {
        "1970-01-01T00:00:00Z".into()
    }
}

pub fn mode() -> Mode {
    MODE.with(|m| {
        *m.get_or_init(|| {
            let has_tauri = web_sys::window()
                .map(|w| js_sys::Reflect::has(&w, &JsValue::from_str("__TAURI__")).unwrap_or(false))
                .unwrap_or(false);
            if has_tauri {
                Mode::Desktop
            } else {
                Mode::Demo
            }
        })
    })
}

fn replay() -> ReplayBackend {
    ReplayBackend::default()
}

async fn invoke<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> ApiResult<T> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|e| ApiError::Other(e.to_string()))?;
    match tauri_invoke(cmd, args).await {
        Ok(v) => serde_wasm_bindgen::from_value(v).map_err(|e| ApiError::Other(format!("{cmd}: {e}"))),
        Err(e) => Err(serde_wasm_bindgen::from_value::<ApiError>(e.clone())
            .unwrap_or_else(|_| ApiError::Other(e.as_string().unwrap_or_else(|| format!("{cmd} failed"))))),
    }
}

#[derive(Serialize)]
struct NoArgs {}

#[derive(Serialize)]
struct Req<'a, T: Serialize> {
    req: &'a T,
}

macro_rules! read {
    ($name:ident, $cmd:literal, $ty:ty) => {
        pub async fn $name() -> ApiResult<$ty> {
            match mode() {
                Mode::Demo => replay().$name().await,
                Mode::Desktop => invoke($cmd, NoArgs {}).await,
            }
        }
    };
}

read!(state, "state", StateView);
read!(wallet_balance, "wallet_balance", WalletBalanceView);
read!(transactions, "transactions", TransactionsView);
read!(new_address, "new_address", AddressView);
read!(channels, "channels", ChannelsView);
read!(channel_backup, "channel_backup", ChannelBackupView);
read!(peers, "peers", PeersView);
read!(payments, "payments", PaymentsView);
read!(invoices, "invoices", InvoicesView);
read!(diagnostics, "diagnostics", DiagnosticsView);
read!(services, "services", ServicesView);
read!(assets, "assets", AssetsView);
read!(swaps, "swaps", SwapsView);

pub async fn capabilities() -> ApiResult<CapabilitySet> {
    match mode() {
        Mode::Demo => Ok(replay().capabilities().clone()),
        Mode::Desktop => invoke("capabilities", NoArgs {}).await,
    }
}

pub async fn estimate_fee(req: &SendOnchainRequest) -> ApiResult<FeeEstimateView> {
    match mode() {
        Mode::Demo => replay().estimate_fee(req).await,
        Mode::Desktop => invoke("estimate_fee", Req { req }).await,
    }
}

pub async fn create_invoice(req: &CreateInvoiceRequest) -> ApiResult<CreatedInvoiceView> {
    match mode() {
        Mode::Demo => replay().create_invoice(req).await,
        Mode::Desktop => invoke("create_invoice", Req { req }).await,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodeArgs<'a> {
    payment_request: &'a str,
}

pub async fn decode_invoice(payment_request: &str) -> ApiResult<DecodedInvoiceView> {
    match mode() {
        Mode::Demo => replay().decode_invoice(payment_request).await,
        Mode::Desktop => invoke("decode_invoice", DecodeArgs { payment_request }).await,
    }
}

pub async fn open_channel(req: &OpenChannelRequest) -> ApiResult<OpenChannelResult> {
    match mode() {
        Mode::Demo => replay().open_channel(req).await,
        Mode::Desktop => invoke("open_channel", Req { req }).await,
    }
}

/// Lightning send. In the demo this runs the real payment engine over the
/// replay dispatcher, so the intent journal shows genuine state transitions.
pub async fn pay_invoice(req: &PayRequest) -> ApiResult<PaymentOutcomeView> {
    match mode() {
        Mode::Desktop => invoke("pay_invoice", Req { req }).await,
        Mode::Demo => {
            let decoded = replay().decode_invoice(&req.payment_request).await?;
            let engine = demo_engine();
            let intent = engine
                .submit(PaymentKind::LightningInvoice, &req.payment_request, req.amount_sat, req.max_fee_sat)
                .map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
            let id = intent.intent_id.clone();
            let done = engine.execute(&id).await.map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
            Ok(PaymentOutcomeView {
                status: match done.state {
                    rw_payments::IntentState::Settled => PaymentOutcomeStatus::Succeeded,
                    rw_payments::IntentState::Failed => PaymentOutcomeStatus::Failed,
                    _ => PaymentOutcomeStatus::NoResult,
                },
                payment_hash: decoded.payment_hash_hex,
                preimage: None,
                failure_reason: done.failure_reason,
                intent_id: Some(id),
            })
        }
    }
}

pub async fn send_onchain(req: &SendOnchainRequest) -> ApiResult<TxidView> {
    match mode() {
        Mode::Demo => {
            // Demo on-chain: the real engine over the replay dispatcher.
            let engine = demo_engine();
            let intent = engine
                .submit_with_rate(PaymentKind::Onchain, &req.address, Some(req.amount_sat), None, req.sat_per_vbyte)
                .map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
            let done = engine.execute(&intent.intent_id).await.map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
            match done.backend_ref {
                Some(txid) if done.state == rw_payments::IntentState::Settled => Ok(TxidView { txid }),
                _ => Err(ApiError::Rejected(done.failure_reason.unwrap_or_else(|| "send failed".into()))),
            }
        }
        Mode::Desktop => invoke("send_onchain", Req { req }).await,
    }
}

/// The payment engine's journal.
pub async fn intents() -> ApiResult<Vec<PaymentIntent>> {
    match mode() {
        Mode::Demo => demo_engine().list().map_err(|e| ApiError::Other(e.to_string())),
        Mode::Desktop => invoke("intents", NoArgs {}).await,
    }
}

// ---------------------------------------------------------------- desktop only

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    /// "none" | "remote" | "sidecar"
    pub kind: String,
    pub endpoint: Option<String>,
    pub network: Option<String>,
    pub sidecar_available: bool,
}

pub async fn connection_info() -> ApiResult<ConnectionInfo> {
    match mode() {
        Mode::Demo => Ok(ConnectionInfo { kind: "demo".into(), network: Some("regtest".into()), ..Default::default() }),
        Mode::Desktop => invoke("connection_info", NoArgs {}).await,
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteNode {
    pub endpoint: String,
    pub tls_cert_path: String,
    pub macaroon_path: String,
}

pub async fn connect_remote(req: &RemoteNode) -> ApiResult<ConnectionInfo> {
    invoke("connect_remote", Req { req }).await
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarRequest {
    pub network: String,
    pub neutrino_peers: Vec<String>,
}

pub async fn start_sidecar(req: &SidecarRequest) -> ApiResult<ConnectionInfo> {
    invoke("start_sidecar", Req { req }).await
}

pub async fn gen_seed() -> ApiResult<Vec<String>> {
    invoke("gen_seed", NoArgs {}).await
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitWalletRequest {
    pub password: String,
    pub mnemonic: Vec<String>,
}

pub async fn init_wallet(req: &InitWalletRequest) -> ApiResult<()> {
    invoke("init_wallet", Req { req }).await
}

#[derive(Serialize)]
struct Password<'a> {
    password: &'a str,
}

pub async fn unlock_wallet(password: &str) -> ApiResult<()> {
    invoke("unlock_wallet", Password { password }).await
}

pub async fn disconnect() -> ApiResult<()> {
    invoke("disconnect", NoArgs {}).await
}
