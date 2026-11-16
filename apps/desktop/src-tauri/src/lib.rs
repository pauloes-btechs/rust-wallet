//! rust-wallet desktop shell (Tauri 2).
//!
//! The webview runs the Leptos UI. Every node operation is a Tauri command
//! implemented here in Rust; the UI never holds a macaroon, a password after
//! submit, or a socket. Two ways to have a node:
//!
//! * **Remote**: an existing LND/litd (endpoint + tls.cert + admin.macaroon
//!   paths). The certificate is pinned exactly.
//! * **Sidecar**: the bundled litd v0.17.8 (SHA-256 pinned in
//!   `sidecars.lock.json`) launched on 127.0.0.1 only, Neutrino backend,
//!   regtest/signet only in this build.
//!
//! Payments run through `rw-payments` with a SQLite journal in the app data
//! dir; interrupted payments are reconciled after connect/unlock.

mod profile;

use profile::Profile;
use rw_core::views::*;
use rw_core::{ApiError, ApiResult, CapabilitySet, MainnetPolicy, Network, NodeApi};
use rw_litd::sidecar::{SidecarConfig, SidecarPorts};
use rw_litd::{ConnectConfig, LitdBackend};
use rw_payments::{IntentStore, PaymentIntent, SqliteStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Manager, State};
use tokio::sync::Mutex;

#[derive(Default)]
struct Conn {
    profile: Option<Profile>,
    connect: Option<ConnectConfig>,
    backend: Option<Arc<LitdBackend>>,
    child: Option<tokio::process::Child>,
}

struct AppState {
    conn: Mutex<Conn>,
    data_dir: PathBuf,
    store: Arc<dyn IntentStore>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConnectionInfo {
    kind: String,
    endpoint: Option<String>,
    network: Option<String>,
    sidecar_available: bool,
}

fn err(e: impl std::fmt::Display) -> ApiError {
    ApiError::Other(e.to_string())
}

/// `~/x` → `$HOME/x`.
fn expand(p: &str) -> PathBuf {
    let p = p.trim();
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

fn litd_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let name = if cfg!(windows) { "litd.exe" } else { "litd" };
    let p = dir.join(name);
    p.exists().then_some(p)
}

/// Random per-install secret (hex), created 0600 on first use.
fn secret_file(path: &std::path::Path) -> ApiResult<String> {
    if let Ok(s) = std::fs::read_to_string(path) {
        return Ok(s.trim().to_string());
    }
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).map_err(err)?;
    let s = hex::encode(b);
    std::fs::write(path, &s).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
    }
    Ok(s)
}

async fn backend(s: &AppState) -> ApiResult<Arc<LitdBackend>> {
    let mut c = s.conn.lock().await;
    if let Some(b) = &c.backend {
        return Ok(b.clone());
    }
    // Sidecar whose wallet was just created: the macaroon now exists.
    if let Some(cfg) = c.connect.clone() {
        if cfg.macaroon.exists() {
            let (b, _) =
                LitdBackend::open(&cfg, s.store.clone()).await.map_err(|e| ApiError::Unreachable(e.to_string()))?;
            let b = Arc::new(b);
            c.backend = Some(b.clone());
            return Ok(b);
        }
        return Err(ApiError::Locked);
    }
    Err(ApiError::Unreachable("not connected to a node".into()))
}

// ------------------------------------------------------------ connection

#[tauri::command]
async fn connection_info(s: State<'_, AppState>) -> ApiResult<ConnectionInfo> {
    let c = s.conn.lock().await;
    Ok(match &c.profile {
        None => ConnectionInfo { kind: "none".into(), sidecar_available: litd_path().is_some(), ..Default::default() },
        Some(p) => ConnectionInfo {
            kind: p.kind().into(),
            endpoint: c.connect.as_ref().map(|c| c.endpoint.clone()),
            network: p.network().map(|n| n.to_string()),
            sidecar_available: litd_path().is_some(),
        },
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteNode {
    endpoint: String,
    tls_cert_path: String,
    macaroon_path: String,
}

async fn do_connect_remote(s: &AppState, req: &RemoteNode) -> ApiResult<()> {
    let cfg = ConnectConfig {
        endpoint: req.endpoint.trim().to_string(),
        tls_cert: expand(&req.tls_cert_path),
        macaroon: expand(&req.macaroon_path),
        mainnet: MainnetPolicy::Refuse,
    };
    let (b, report) =
        LitdBackend::open(&cfg, s.store.clone()).await.map_err(|e| ApiError::Unreachable(e.to_string()))?;
    if !report.needs_recovery.is_empty() {
        tracing::warn!(count = report.needs_recovery.len(), "payments need recovery");
    }
    let profile = Profile::Remote {
        endpoint: cfg.endpoint.clone(),
        tls_cert_path: cfg.tls_cert.display().to_string(),
        macaroon_path: cfg.macaroon.display().to_string(),
    };
    profile.save(&s.data_dir).map_err(err)?;
    let mut c = s.conn.lock().await;
    *c = Conn { profile: Some(profile), connect: Some(cfg), backend: Some(Arc::new(b)), child: None };
    Ok(())
}

#[tauri::command]
async fn connect_remote(s: State<'_, AppState>, req: RemoteNode) -> ApiResult<ConnectionInfo> {
    do_connect_remote(&s, &req).await?;
    connection_info(s).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarRequest {
    network: String,
    neutrino_peers: Vec<String>,
}

async fn do_start_sidecar(s: &AppState, network: Network, peers: Vec<String>) -> ApiResult<()> {
    if network.is_mainnet() {
        return Err(ApiError::MainnetRefused);
    }
    let bin = litd_path().ok_or_else(|| {
        ApiError::Unsupported("bundled litd not found (dev: run `cargo xtask fetch-sidecars`)".into())
    })?;
    let cfg = SidecarConfig {
        litd_bin: bin,
        data_dir: s.data_dir.join("node").join(network.to_string()),
        network,
        ports: SidecarPorts::default(),
        neutrino_peers: peers.clone(),
        ui_password: secret_file(&s.data_dir.join("litd-ui-password"))?,
    };
    let mut child = rw_litd::sidecar::spawn(&cfg).map_err(err)?;
    if !rw_litd::sidecar::wait_for_file(&cfg.tls_cert(), Duration::from_secs(90)).await {
        let _ = child.kill().await;
        return Err(ApiError::Unreachable("litd did not start (no TLS certificate after 90s)".into()));
    }
    let connect = ConnectConfig {
        endpoint: format!("127.0.0.1:{}", cfg.ports.lnd_rpc),
        tls_cert: cfg.tls_cert(),
        macaroon: cfg.admin_macaroon(),
        mainnet: MainnetPolicy::Refuse,
    };
    let profile = Profile::Sidecar { network, neutrino_peers: peers };
    profile.save(&s.data_dir).map_err(err)?;
    let mut c = s.conn.lock().await;
    *c = Conn { profile: Some(profile), connect: Some(connect), backend: None, child: Some(child) };
    Ok(())
}

#[tauri::command]
async fn start_sidecar(s: State<'_, AppState>, req: SidecarRequest) -> ApiResult<ConnectionInfo> {
    let network: Network = req.network.parse().map_err(ApiError::Invalid)?;
    do_start_sidecar(&s, network, req.neutrino_peers).await?;
    connection_info(s).await
}

#[tauri::command]
async fn disconnect(s: State<'_, AppState>) -> ApiResult<()> {
    let mut c = s.conn.lock().await;
    if let Some(mut child) = c.child.take() {
        let _ = child.kill().await;
    }
    *c = Conn::default();
    Profile::clear(&s.data_dir);
    Ok(())
}

// ------------------------------------------------------- wallet lifecycle

async fn unlocker(
    s: &AppState,
) -> ApiResult<rw_litd::proto::lnrpc::wallet_unlocker_client::WalletUnlockerClient<tonic_channel::Channel>> {
    let cfg = s.conn.lock().await.connect.clone().ok_or_else(|| ApiError::Unreachable("no node".into()))?;
    rw_litd::connect_unlocker(&cfg).await.map_err(|e| ApiError::Unreachable(e.to_string()))
}

mod tonic_channel {
    pub use tonic::transport::Channel;
}

#[tauri::command]
async fn gen_seed(s: State<'_, AppState>) -> ApiResult<Vec<String>> {
    let r = unlocker(&s)
        .await?
        .gen_seed(rw_litd::proto::lnrpc::GenSeedRequest::default())
        .await
        .map_err(|e| ApiError::Rejected(e.message().to_string()))?;
    Ok(r.into_inner().cipher_seed_mnemonic)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitWalletRequest {
    password: String,
    mnemonic: Vec<String>,
}

#[tauri::command]
async fn init_wallet(s: State<'_, AppState>, req: InitWalletRequest) -> ApiResult<()> {
    if req.password.len() < 8 {
        return Err(ApiError::Invalid("password must be at least 8 characters".into()));
    }
    if req.mnemonic.len() != 24 {
        return Err(ApiError::Invalid("expected a 24-word aezeed".into()));
    }
    unlocker(&s)
        .await?
        .init_wallet(rw_litd::proto::lnrpc::InitWalletRequest {
            wallet_password: req.password.into_bytes(),
            cipher_seed_mnemonic: req.mnemonic,
            ..Default::default()
        })
        .await
        .map_err(|e| ApiError::Rejected(e.message().to_string()))?;
    wait_ready(&s).await
}

#[tauri::command]
async fn unlock_wallet(s: State<'_, AppState>, password: String) -> ApiResult<()> {
    unlocker(&s)
        .await?
        .unlock_wallet(rw_litd::proto::lnrpc::UnlockWalletRequest {
            wallet_password: password.into_bytes(),
            ..Default::default()
        })
        .await
        .map_err(|e| ApiError::Rejected(e.message().to_string()))?;
    wait_ready(&s).await
}

/// After create/unlock: wait for the macaroon and a ready RPC server, then
/// run the deferred crash recovery.
async fn wait_ready(s: &AppState) -> ApiResult<()> {
    for _ in 0..120 {
        if let Ok(b) = backend(s).await {
            if b.state().await.map(|v| v.phase == Phase::Ready).unwrap_or(false) {
                b.recover_if_ready().await.map_err(err)?;
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(ApiError::Unreachable("node did not become ready within 60s".into()))
}

// ------------------------------------------------------------- node api

#[tauri::command]
async fn state(s: State<'_, AppState>) -> ApiResult<StateView> {
    match backend(&s).await {
        Ok(b) => b.state().await,
        Err(ApiError::Locked) => Ok(StateView { phase: Phase::Missing, info: None, error: None }),
        Err(e) => Ok(StateView { phase: Phase::Undetermined, info: None, error: Some(e.to_string()) }),
    }
}

#[tauri::command]
async fn capabilities(s: State<'_, AppState>) -> ApiResult<CapabilitySet> {
    Ok(backend(&s).await?.capabilities().clone())
}

#[tauri::command]
async fn intents(s: State<'_, AppState>) -> ApiResult<Vec<PaymentIntent>> {
    s.store.list().map_err(err)
}

macro_rules! passthrough {
    ($($name:ident -> $ty:ty;)*) => {$(
        #[tauri::command]
        async fn $name(s: State<'_, AppState>) -> ApiResult<$ty> {
            backend(&s).await?.$name().await
        }
    )*};
}

passthrough! {
    wallet_balance -> WalletBalanceView;
    transactions -> TransactionsView;
    new_address -> AddressView;
    channels -> ChannelsView;
    channel_backup -> ChannelBackupView;
    peers -> PeersView;
    payments -> PaymentsView;
    invoices -> InvoicesView;
    diagnostics -> DiagnosticsView;
    services -> ServicesView;
    assets -> AssetsView;
    swaps -> SwapsView;
}

#[tauri::command]
async fn estimate_fee(s: State<'_, AppState>, req: SendOnchainRequest) -> ApiResult<FeeEstimateView> {
    backend(&s).await?.estimate_fee(&req).await
}

#[tauri::command]
async fn send_onchain(s: State<'_, AppState>, req: SendOnchainRequest) -> ApiResult<TxidView> {
    backend(&s).await?.send_onchain(&req).await
}

#[tauri::command]
async fn open_channel(s: State<'_, AppState>, req: OpenChannelRequest) -> ApiResult<OpenChannelResult> {
    backend(&s).await?.open_channel(&req).await
}

#[tauri::command]
async fn create_invoice(s: State<'_, AppState>, req: CreateInvoiceRequest) -> ApiResult<CreatedInvoiceView> {
    backend(&s).await?.create_invoice(&req).await
}

#[tauri::command]
async fn decode_invoice(s: State<'_, AppState>, payment_request: String) -> ApiResult<DecodedInvoiceView> {
    backend(&s).await?.decode_invoice(&payment_request).await
}

#[tauri::command]
async fn pay_invoice(s: State<'_, AppState>, req: PayRequest) -> ApiResult<PaymentOutcomeView> {
    backend(&s).await?.pay_invoice(&req).await
}

// ------------------------------------------------------------------- run

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store: Arc<dyn IntentStore> = Arc::new(SqliteStore::open(data_dir.join("intents.db"))?);
            app.manage(AppState { conn: Mutex::new(Conn::default()), data_dir: data_dir.clone(), store });

            // Reconnect the last profile in the background.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let s = handle.state::<AppState>();
                let r = match Profile::load(&data_dir) {
                    Some(Profile::Remote { endpoint, tls_cert_path, macaroon_path }) => {
                        do_connect_remote(&s, &RemoteNode { endpoint, tls_cert_path, macaroon_path }).await
                    }
                    Some(Profile::Sidecar { network, neutrino_peers }) => {
                        do_start_sidecar(&s, network, neutrino_peers).await
                    }
                    None => Ok(()),
                };
                if let Err(e) = r {
                    tracing::warn!(error = %e, "could not restore the last connection");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_info,
            connect_remote,
            start_sidecar,
            disconnect,
            gen_seed,
            init_wallet,
            unlock_wallet,
            state,
            capabilities,
            intents,
            wallet_balance,
            transactions,
            new_address,
            channels,
            channel_backup,
            peers,
            payments,
            invoices,
            diagnostics,
            services,
            assets,
            swaps,
            estimate_fee,
            send_onchain,
            open_channel,
            create_invoice,
            decode_invoice,
            pay_invoice,
        ])
        .run(tauri::generate_context!())
        .expect("error while running rust-wallet");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands() {
        std::env::set_var("HOME", "/home/u");
        assert_eq!(expand("~/.lnd/tls.cert"), PathBuf::from("/home/u/.lnd/tls.cert"));
        assert_eq!(expand(" /abs "), PathBuf::from("/abs"));
    }

    #[test]
    fn secret_is_stable_and_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("pw");
        let a = secret_file(&p).unwrap();
        assert_eq!(a.len(), 48);
        assert_eq!(secret_file(&p).unwrap(), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
