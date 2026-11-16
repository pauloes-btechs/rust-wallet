use crate::client::{connect, ConnectError, LitdClient};
use crate::config::ConnectConfig;
use crate::dispatcher::{default_fee_cap_sat, LndDispatcher};
use crate::map;
use crate::proto::{frdrpc, litrpc, lnrpc, looprpc, taprpc};
use rw_core::destination::{parse_destination, Destination};
use rw_core::views::*;
use rw_core::{ApiError, ApiResult, Capability, CapabilitySet, MainnetPolicy, Network, NodeApi};
use rw_payments::{IntentState, IntentStore, PaymentEngine, PaymentIntent, PaymentKind, RecoveryReport};
use std::sync::Arc;

pub type Engine = PaymentEngine<Arc<dyn IntentStore>, LndDispatcher>;

/// [`NodeApi`] over a live litd (or plain LND) node.
pub struct LitdBackend {
    client: LitdClient,
    caps: CapabilitySet,
    mainnet: MainnetPolicy,
    engine: Engine,
}

fn rpc(s: tonic::Status) -> ApiError {
    use tonic::Code::*;
    match s.code() {
        Unavailable | DeadlineExceeded => ApiError::Unreachable(s.message().to_string()),
        InvalidArgument | OutOfRange => ApiError::Invalid(s.message().to_string()),
        Unimplemented => ApiError::Unsupported(s.message().to_string()),
        _ if map::phase_from_status(&s) == Phase::Locked => ApiError::Locked,
        _ => ApiError::Rejected(s.message().to_string()),
    }
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Seconds-precision UTC without a date crate.
    let days = secs / 86_400;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's days → civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn new_intent_id() -> String {
    // 128 random bits from the OS via a fresh ring RNG; formatted as a UUIDv4.
    use rustls::crypto::ring::default_provider;
    let mut b = [0u8; 16];
    default_provider().secure_random.fill(&mut b).expect("OS RNG");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

impl LitdBackend {
    /// Connect, apply the mainnet guard, probe litd sub-servers to declare
    /// capabilities, and, if the wallet is unlocked, reconcile any payment a
    /// previous run left interrupted. With a locked wallet recovery is
    /// deferred to [`Self::recover_if_ready`]: a lookup against a locked node
    /// would wrongly escalate in-flight payments to needs-recovery.
    pub async fn open(
        cfg: &ConnectConfig,
        store: Arc<dyn IntentStore>,
    ) -> Result<(Self, RecoveryReport), BackendOpenError> {
        let client = connect(cfg).await?;
        let backend = Self::from_client(client, cfg.mainnet, store).await;
        let report = backend.recover_if_ready().await?;
        Ok((backend, report))
    }

    /// Mainnet guard + crash recovery, only once the node answers GetInfo.
    /// Returns an empty report while the wallet is locked or still starting.
    pub async fn recover_if_ready(&self) -> Result<RecoveryReport, BackendOpenError> {
        let Ok(info) = self.client.lightning().get_info(lnrpc::GetInfoRequest {}).await else {
            return Ok(RecoveryReport::default());
        };
        let net = map::node_health(&info.into_inner()).network().ok_or(BackendOpenError::UnknownNetwork)?;
        self.mainnet.check(net)?;
        self.engine.recover_interrupted().await.map_err(|e| BackendOpenError::Recovery(e.to_string()))
    }

    pub async fn from_client(client: LitdClient, mainnet: MainnetPolicy, store: Arc<dyn IntentStore>) -> Self {
        use Capability::*;
        let mut caps =
            vec![Read, OnchainReceive, OnchainSend, LightningReceive, LightningSend, Channels, ChannelBackups];
        if let Ok(r) = client.status().sub_server_status(litrpc::SubServerStatusReq {}).await {
            caps.push(Services);
            let subs = r.into_inner().sub_servers;
            let running = |k: &str| subs.get(k).is_some_and(|s| s.running && !s.disabled);
            if running("loop") {
                caps.push(Loop);
            }
            if running("taproot-assets") {
                caps.push(TaprootAssets);
            }
            if running("faraday") {
                caps.push(Faraday);
            }
        }
        let engine = PaymentEngine::new(store, LndDispatcher::new(client.clone()), new_intent_id, now_rfc3339);
        Self { client, caps: CapabilitySet::of(&caps), mainnet, engine }
    }

    pub fn client(&self) -> &LitdClient {
        &self.client
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Node-reported network, after the mainnet guard. Called before every write.
    async fn guard(&self) -> ApiResult<Network> {
        let info = self.client.lightning().get_info(lnrpc::GetInfoRequest {}).await.map_err(rpc)?.into_inner();
        let net = map::node_health(&info).network().ok_or_else(|| ApiError::Other("node reported no chain".into()))?;
        self.mainnet.check(net)?;
        Ok(net)
    }

    fn outcome(intent: &PaymentIntent, payment_hash: String) -> PaymentOutcomeView {
        let status = match intent.state {
            IntentState::Settled => PaymentOutcomeStatus::Succeeded,
            IntentState::Failed => PaymentOutcomeStatus::Failed,
            _ => PaymentOutcomeStatus::NoResult,
        };
        PaymentOutcomeView {
            status,
            payment_hash,
            preimage: None,
            failure_reason: intent.failure_reason.clone(),
            intent_id: Some(intent.intent_id.clone()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BackendOpenError {
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error(transparent)]
    Guard(#[from] ApiError),
    #[error("the node reported no recognisable network")]
    UnknownNetwork,
    #[error("payment recovery failed: {0}")]
    Recovery(String),
}

impl NodeApi for LitdBackend {
    fn capabilities(&self) -> &CapabilitySet {
        &self.caps
    }

    async fn state(&self) -> ApiResult<StateView> {
        match self.client.lightning().get_info(lnrpc::GetInfoRequest {}).await {
            Ok(r) => {
                let info = map::node_health(&r.into_inner());
                if let Some(net) = info.network() {
                    self.mainnet.check(net)?;
                }
                Ok(StateView { phase: Phase::Ready, info: Some(info), error: None })
            }
            Err(s) => {
                Ok(StateView { phase: map::phase_from_status(&s), info: None, error: Some(s.message().to_string()) })
            }
        }
    }

    async fn wallet_balance(&self) -> ApiResult<WalletBalanceView> {
        let r = self.client.lightning().wallet_balance(lnrpc::WalletBalanceRequest::default()).await.map_err(rpc)?;
        Ok(map::wallet_balance(&r.into_inner()))
    }

    async fn transactions(&self) -> ApiResult<TransactionsView> {
        let r =
            self.client.lightning().get_transactions(lnrpc::GetTransactionsRequest::default()).await.map_err(rpc)?;
        let mut transactions: Vec<_> = r.into_inner().transactions.iter().map(map::transaction).collect();
        transactions.sort_by_key(|t| std::cmp::Reverse(t.at));
        Ok(TransactionsView { transactions })
    }

    async fn new_address(&self) -> ApiResult<AddressView> {
        self.caps.require(Capability::OnchainReceive)?;
        self.guard().await?;
        let r = self
            .client
            .lightning()
            .new_address(lnrpc::NewAddressRequest {
                r#type: lnrpc::AddressType::TaprootPubkey as i32,
                ..Default::default()
            })
            .await
            .map_err(rpc)?;
        Ok(AddressView { address: r.into_inner().address })
    }

    async fn estimate_fee(&self, req: &SendOnchainRequest) -> ApiResult<FeeEstimateView> {
        let net = self.guard().await?;
        let d = parse_destination(&req.address, Some(net))?;
        let r = self
            .client
            .lightning()
            .estimate_fee(lnrpc::EstimateFeeRequest {
                addr_to_amount: [(d.as_str().to_string(), req.amount_sat as i64)].into(),
                target_conf: 6,
                ..Default::default()
            })
            .await
            .map_err(rpc)?
            .into_inner();
        Ok(FeeEstimateView { sat_per_vbyte: r.sat_per_vbyte, fee_sat: r.fee_sat.max(0) as u64 })
    }

    async fn send_onchain(&self, req: &SendOnchainRequest) -> ApiResult<TxidView> {
        self.caps.require(Capability::OnchainSend)?;
        let net = self.guard().await?;
        let d = parse_destination(&req.address, Some(net))?;
        if d.is_lightning() {
            return Err(ApiError::Invalid("that is a lightning invoice, not an address".into()));
        }
        if req.amount_sat == 0 {
            return Err(ApiError::Invalid("amount must be greater than zero".into()));
        }
        let intent = self
            .engine
            .submit_with_rate(PaymentKind::Onchain, d.as_str(), Some(req.amount_sat), None, req.sat_per_vbyte)
            .map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
        let done = self.engine.execute(&intent.intent_id).await.map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
        match done.state {
            IntentState::Settled => Ok(TxidView { txid: done.backend_ref.unwrap_or_default() }),
            IntentState::Failed => Err(ApiError::Rejected(done.failure_reason.unwrap_or_else(|| "send failed".into()))),
            _ => Err(ApiError::PaymentSafety(format!(
                "broadcast not confirmed by the node; intent {} will be reconciled by its label, not re-sent",
                done.intent_id
            ))),
        }
    }

    async fn channels(&self) -> ApiResult<ChannelsView> {
        let mut ln = self.client.lightning();
        let open = ln.list_channels(lnrpc::ListChannelsRequest::default()).await.map_err(rpc)?.into_inner();
        let pending = ln.pending_channels(lnrpc::PendingChannelsRequest::default()).await.map_err(rpc)?.into_inner();
        let bal = ln.channel_balance(lnrpc::ChannelBalanceRequest {}).await.map_err(rpc)?.into_inner();
        Ok(map::channels(&open, &pending, &bal))
    }

    async fn open_channel(&self, req: &OpenChannelRequest) -> ApiResult<OpenChannelResult> {
        self.caps.require(Capability::Channels)?;
        self.guard().await?;
        let (pubkey, host) = match req.peer.split_once('@') {
            Some((p, h)) => (p.to_string(), Some(h.to_string())),
            None => (req.peer.clone(), None),
        };
        if pubkey.len() != 66 || hex::decode(&pubkey).is_err() {
            return Err(ApiError::Invalid("peer must be a 33-byte hex pubkey, optionally @host:port".into()));
        }
        let mut ln = self.client.lightning();
        if let Some(host) = host {
            let r = ln
                .connect_peer(lnrpc::ConnectPeerRequest {
                    addr: Some(lnrpc::LightningAddress { pubkey: pubkey.clone(), host }),
                    perm: false,
                    timeout: 30,
                })
                .await;
            if let Err(s) = r {
                if !s.message().contains("already connected") {
                    return Err(rpc(s));
                }
            }
        }
        let cp = ln
            .open_channel_sync(lnrpc::OpenChannelRequest {
                node_pubkey: hex::decode(&pubkey).unwrap_or_default(),
                local_funding_amount: req.local_funding_sat as i64,
                sat_per_vbyte: req.sat_per_vbyte.unwrap_or(0),
                target_conf: if req.sat_per_vbyte.is_some() { 0 } else { 6 },
                private: req.private,
                ..Default::default()
            })
            .await
            .map_err(rpc)?
            .into_inner();
        Ok(OpenChannelResult { channel_point: map::channel_point_string(&cp) })
    }

    async fn channel_backup(&self) -> ApiResult<ChannelBackupView> {
        self.caps.require(Capability::ChannelBackups)?;
        let r = self
            .client
            .lightning()
            .export_all_channel_backups(lnrpc::ChanBackupExportRequest {})
            .await
            .map_err(rpc)?
            .into_inner();
        let multi = r.multi_chan_backup.map(|m| m.multi_chan_backup).unwrap_or_default();
        Ok(ChannelBackupView { multi_chan_backup_hex: hex::encode(multi) })
    }

    async fn peers(&self) -> ApiResult<PeersView> {
        let r = self.client.lightning().list_peers(lnrpc::ListPeersRequest::default()).await.map_err(rpc)?;
        Ok(PeersView { peers: r.into_inner().peers.iter().map(map::peer).collect() })
    }

    async fn payments(&self) -> ApiResult<PaymentsView> {
        let r = self
            .client
            .lightning()
            .list_payments(lnrpc::ListPaymentsRequest {
                include_incomplete: true,
                reversed: true,
                max_payments: 200,
                ..Default::default()
            })
            .await
            .map_err(rpc)?;
        Ok(PaymentsView { payments: r.into_inner().payments.iter().map(map::payment).collect() })
    }

    async fn invoices(&self) -> ApiResult<InvoicesView> {
        let r = self
            .client
            .lightning()
            .list_invoices(lnrpc::ListInvoiceRequest { reversed: true, num_max_invoices: 200, ..Default::default() })
            .await
            .map_err(rpc)?;
        Ok(InvoicesView { invoices: r.into_inner().invoices.iter().map(map::invoice).collect() })
    }

    async fn create_invoice(&self, req: &CreateInvoiceRequest) -> ApiResult<CreatedInvoiceView> {
        self.caps.require(Capability::LightningReceive)?;
        self.guard().await?;
        let r = self
            .client
            .lightning()
            .add_invoice(lnrpc::Invoice {
                memo: req.memo.clone(),
                value: req.amount_sat as i64,
                expiry: req.expiry_secs.map(i64::from).unwrap_or(3600),
                ..Default::default()
            })
            .await
            .map_err(rpc)?
            .into_inner();
        Ok(CreatedInvoiceView { payment_request: r.payment_request, r_hash_hex: hex::encode(r.r_hash) })
    }

    async fn decode_invoice(&self, payment_request: &str) -> ApiResult<DecodedInvoiceView> {
        let net = self.guard().await?;
        let d = parse_destination(payment_request, Some(net))?;
        let Destination::Bolt11 { invoice, .. } = d else {
            return Err(ApiError::Invalid("not a lightning invoice".into()));
        };
        let r = self.client.lightning().decode_pay_req(lnrpc::PayReqString { pay_req: invoice }).await.map_err(rpc)?;
        Ok(map::decoded(&r.into_inner()))
    }

    /// Validate offline → decode on the node → run through the payment
    /// engine (persist, dispatch once, reconcile on doubt).
    async fn pay_invoice(&self, req: &PayRequest) -> ApiResult<PaymentOutcomeView> {
        self.caps.require(Capability::LightningSend)?;
        let net = self.guard().await?;
        let d = parse_destination(&req.payment_request, Some(net))?;
        let Destination::Bolt11 { invoice, .. } = d else {
            return Err(ApiError::Invalid("not a lightning invoice".into()));
        };
        let decoded = self.decode_invoice(&invoice).await?;
        let amount = match (decoded.amount_sat, req.amount_sat) {
            (Some(_), Some(_)) => return Err(ApiError::Invalid("this invoice already has an amount".into())),
            (None, None) => return Err(ApiError::Invalid("this invoice has no amount; enter one".into())),
            (Some(a), None) => a,
            (None, Some(a)) => a,
        };
        let fee_cap = req.max_fee_sat.unwrap_or_else(|| default_fee_cap_sat(amount));
        let intent = self
            .engine
            .submit(PaymentKind::LightningInvoice, &invoice, req.amount_sat, Some(fee_cap))
            .map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
        let done = self.engine.execute(&intent.intent_id).await.map_err(|e| ApiError::PaymentSafety(e.to_string()))?;
        Ok(Self::outcome(&done, decoded.payment_hash_hex))
    }

    async fn diagnostics(&self) -> ApiResult<DiagnosticsView> {
        let mut ln = self.client.lightning();
        let node = map::node_health(&ln.get_info(lnrpc::GetInfoRequest {}).await.map_err(rpc)?.into_inner());
        let closed = ln
            .closed_channels(lnrpc::ClosedChannelsRequest::default())
            .await
            .map_err(rpc)?
            .into_inner()
            .channels
            .iter()
            .map(map::closed_channel)
            .collect();
        let forwarding = ln
            .forwarding_history(lnrpc::ForwardingHistoryRequest { num_max_events: 200, ..Default::default() })
            .await
            .map(|r| r.into_inner().forwarding_events.iter().map(map::forwarding_event).collect())
            .unwrap_or_default();
        let faraday = if self.caps.has(Capability::Faraday) {
            match self.client.faraday().channel_insights(frdrpc::ChannelInsightsRequest {}).await {
                Ok(r) => FaradayView {
                    available: true,
                    error: None,
                    insights: r.into_inner().channel_insights.iter().map(map::insight).collect(),
                },
                Err(s) => FaradayView { available: false, error: Some(s.message().to_string()), insights: vec![] },
            }
        } else {
            FaradayView { available: false, error: Some("faraday is not running".into()), insights: vec![] }
        };
        Ok(DiagnosticsView { node, closed, forwarding, faraday })
    }

    async fn services(&self) -> ApiResult<ServicesView> {
        self.caps.require(Capability::Services)?;
        let net = self.guard().await?;
        let r = self.client.status().sub_server_status(litrpc::SubServerStatusReq {}).await.map_err(rpc)?.into_inner();
        let sub_servers = map::sub_servers(&r.sub_servers);
        let ready = sub_servers.get("lnd").is_some_and(|s| s.running);
        Ok(ServicesView { network: net.to_string(), ready, sub_servers })
    }

    async fn assets(&self) -> ApiResult<AssetsView> {
        self.caps.require(Capability::TaprootAssets)?;
        let mut tap = self.client.taproot_assets();
        let balances = tap
            .list_balances(taprpc::ListBalancesRequest {
                group_by: Some(taprpc::list_balances_request::GroupBy::AssetId(true)),
                ..Default::default()
            })
            .await
            .map_err(rpc)?
            .into_inner();
        let universe = tap.get_info(taprpc::GetInfoRequest {}).await.ok().map(|r| {
            let r = r.into_inner();
            UniverseView { block_height: r.block_height, network: r.network, version: r.version }
        });
        Ok(AssetsView { balances: map::asset_balances(&balances), universe })
    }

    async fn swaps(&self) -> ApiResult<SwapsView> {
        self.caps.require(Capability::Loop)?;
        let r = self.client.loop_client().list_swaps(looprpc::ListSwapsRequest::default()).await.map_err(rpc)?;
        Ok(SwapsView { swaps: r.into_inner().swaps.iter().map(map::swap).collect() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_408), (2025, 11, 16));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn intent_ids_are_uuid_v4() {
        let a = new_intent_id();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert_ne!(a, new_intent_id());
    }
}
