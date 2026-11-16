//! The backend → UI wire contract.
//!
//! Field names are camelCase on the wire. Amounts that LND carries as
//! (u)int64 are decimal strings (see [`crate::decstr`]); small counters and
//! timestamps in milliseconds stay JSON numbers. The shapes mirror the litd
//! regtest recording that ships in `rw-replay`, so the hosted demo and the
//! desktop app render the same structures.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- node state

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Wallet unlocked, RPC serving.
    Ready,
    /// Wallet exists but is locked.
    Locked,
    /// No wallet yet (needs create or restore).
    Missing,
    /// Could not determine (node unreachable or still starting).
    Undetermined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainView {
    pub chain: String,
    pub network: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeHealthView {
    pub identity_pubkey: String,
    pub alias: String,
    pub version: String,
    pub synced_to_chain: bool,
    pub synced_to_graph: bool,
    pub block_height: u32,
    pub num_peers: u32,
    pub num_active_channels: u32,
    pub num_pending_channels: u32,
    pub num_inactive_channels: u32,
    pub uris: Vec<String>,
    pub chains: Vec<ChainView>,
}

impl NodeHealthView {
    /// The node-reported network (first chain entry), if parseable.
    pub fn network(&self) -> Option<crate::Network> {
        self.chains.first().and_then(|c| c.network.parse().ok())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateView {
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<NodeHealthView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ------------------------------------------------------------------ on-chain

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletBalanceView {
    #[serde(with = "crate::decstr")]
    pub total_sat: u64,
    #[serde(with = "crate::decstr")]
    pub confirmed_sat: u64,
    #[serde(with = "crate::decstr")]
    pub unconfirmed_sat: u64,
    #[serde(with = "crate::decstr")]
    pub locked_sat: u64,
    #[serde(with = "crate::decstr")]
    pub reserved_anchor_sat: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnchainTxView {
    pub txid: String,
    #[serde(with = "crate::decstr::signed")]
    pub amount_sat: i64,
    #[serde(with = "crate::decstr")]
    pub fee_sat: u64,
    pub block_height: i32,
    pub confirmations: i32,
    /// Unix milliseconds.
    pub at: i64,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub addresses: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionsView {
    pub transactions: Vec<OnchainTxView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressView {
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeEstimateView {
    #[serde(with = "crate::decstr")]
    pub sat_per_vbyte: u64,
    #[serde(with = "crate::decstr")]
    pub fee_sat: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxidView {
    pub txid: String,
}

// ------------------------------------------------------------------ channels

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelState {
    Opening,
    Open,
    OpenInactive,
    Closing,
    ClosingForce,
    WaitingClose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRowView {
    pub channel_point: String,
    pub chan_id: String,
    pub remote_pubkey: String,
    #[serde(with = "crate::decstr")]
    pub capacity_sat: u64,
    #[serde(with = "crate::decstr")]
    pub local_balance_sat: u64,
    #[serde(with = "crate::decstr")]
    pub remote_balance_sat: u64,
    pub state: ChannelState,
    pub initiator_label: String,
    /// LND commitment type name (absent from older recordings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commitment_type: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelBalanceView {
    #[serde(with = "crate::decstr")]
    pub local_sat: u64,
    #[serde(with = "crate::decstr")]
    pub remote_sat: u64,
    #[serde(with = "crate::decstr")]
    pub local_unsettled_sat: u64,
    #[serde(with = "crate::decstr")]
    pub pending_open_local_sat: u64,
    #[serde(with = "crate::decstr")]
    pub pending_open_remote_sat: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelsView {
    pub open: Vec<ChannelRowView>,
    pub pending: Vec<ChannelRowView>,
    pub balance: ChannelBalanceView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenChannelRequest {
    /// `pubkey@host:port`, or a bare pubkey of an already-connected peer.
    pub peer: String,
    #[serde(with = "crate::decstr")]
    pub local_funding_sat: u64,
    #[serde(default, with = "crate::decstr::opt")]
    pub sat_per_vbyte: Option<u64>,
    #[serde(default)]
    pub private: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenChannelResult {
    pub channel_point: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelBackupView {
    pub multi_chan_backup_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub pubkey: String,
    pub address: String,
    pub inbound: bool,
    #[serde(with = "crate::decstr")]
    pub sat_sent: u64,
    #[serde(with = "crate::decstr")]
    pub sat_received: u64,
    pub ping_ms: u64,
    pub flap_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeersView {
    pub peers: Vec<PeerView>,
}

// ------------------------------------------------------------------ payments

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentView {
    pub payment_hash: String,
    /// LND payment status, lowercased: "succeeded" | "failed" | "in_flight" | "initiated" | "unknown".
    pub status: String,
    #[serde(with = "crate::decstr")]
    pub value_sat: u64,
    #[serde(with = "crate::decstr")]
    pub fee_sat: u64,
    #[serde(with = "crate::decstr")]
    pub creation_time_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_request: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentsView {
    pub payments: Vec<PaymentView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaymentOutcomeStatus {
    Succeeded,
    Failed,
    /// Dispatched, final state not observed yet. Reconciled later; never re-sent.
    NoResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentOutcomeView {
    pub status: PaymentOutcomeStatus,
    pub payment_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// The payment-intent id that tracked this send (absent in recordings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayRequest {
    pub payment_request: String,
    /// Required for zero-amount invoices only.
    #[serde(default, with = "crate::decstr::opt")]
    pub amount_sat: Option<u64>,
    /// Spend policy: hard cap on routing fees.
    #[serde(default, with = "crate::decstr::opt")]
    pub max_fee_sat: Option<u64>,
}

// ------------------------------------------------------------------ invoices

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceRowView {
    pub payment_hash_hex: String,
    pub payment_request: String,
    pub memo: String,
    /// "open" | "settled" | "canceled" | "accepted"
    pub state: String,
    #[serde(with = "crate::decstr")]
    pub value_sat: u64,
    #[serde(with = "crate::decstr")]
    pub paid_sat: u64,
    pub is_keysend: bool,
    /// Unix milliseconds.
    pub created_at: i64,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoicesView {
    pub invoices: Vec<InvoiceRowView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInvoiceRequest {
    #[serde(with = "crate::decstr")]
    pub amount_sat: u64,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub expiry_secs: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedInvoiceView {
    pub payment_request: String,
    pub r_hash_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedInvoiceView {
    pub payment_hash_hex: String,
    #[serde(default, with = "crate::decstr::opt", skip_serializing_if = "Option::is_none")]
    pub amount_sat: Option<u64>,
    pub memo: String,
    pub destination: String,
    #[serde(with = "crate::decstr")]
    pub cltv_expiry: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOnchainRequest {
    pub address: String,
    #[serde(with = "crate::decstr")]
    pub amount_sat: u64,
    #[serde(default, with = "crate::decstr::opt")]
    pub sat_per_vbyte: Option<u64>,
}

// --------------------------------------------------------------- diagnostics

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedChannelView {
    pub channel_point: String,
    pub chan_id: String,
    pub remote_pubkey: String,
    pub closing_tx_hash: String,
    pub close_type: String,
    pub close_height: u32,
    #[serde(with = "crate::decstr")]
    pub settled_balance_sat: u64,
    #[serde(with = "crate::decstr")]
    pub time_locked_balance_sat: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardingEventView {
    #[serde(with = "crate::decstr")]
    pub timestamp_ns: u64,
    #[serde(with = "crate::decstr")]
    pub amt_in_sat: u64,
    #[serde(with = "crate::decstr")]
    pub amt_out_sat: u64,
    #[serde(with = "crate::decstr")]
    pub fee_sat: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInsightView {
    pub chan_point: String,
    #[serde(with = "crate::decstr")]
    pub monitored_seconds: u64,
    #[serde(with = "crate::decstr")]
    pub uptime_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_ratio: Option<f64>,
    #[serde(with = "crate::decstr")]
    pub volume_incoming_msat: u64,
    #[serde(with = "crate::decstr")]
    pub volume_outgoing_msat: u64,
    #[serde(with = "crate::decstr")]
    pub fees_earned_msat: u64,
    pub is_private: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FaradayView {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub insights: Vec<ChannelInsightView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticsView {
    pub node: NodeHealthView,
    pub closed: Vec<ClosedChannelView>,
    pub forwarding: Vec<ForwardingEventView>,
    pub faraday: FaradayView,
}

// ------------------------------------------------------- litd sub-servers

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubServerView {
    pub disabled: bool,
    pub running: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub custom_status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesView {
    pub network: String,
    pub ready: bool,
    /// Keyed by litd sub-server name (lnd, loop, pool, faraday, taproot-assets, …).
    pub sub_servers: std::collections::BTreeMap<String, SubServerView>,
}

// ---------------------------------------------------------- taproot assets

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetBalanceView {
    pub asset_id_hex: String,
    pub name: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    #[serde(with = "crate::decstr")]
    pub balance: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniverseView {
    pub block_height: u32,
    pub network: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetsView {
    pub balances: Vec<AssetBalanceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universe: Option<UniverseView>,
}

// ---------------------------------------------------------------------- loop

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapView {
    pub id: String,
    /// "loop-out" | "loop-in"
    #[serde(rename = "type")]
    pub swap_type: String,
    pub state: String,
    #[serde(with = "crate::decstr")]
    pub amount_sat: u64,
    #[serde(with = "crate::decstr")]
    pub cost_sat: u64,
    pub initiation_time_ns: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapsView {
    pub swaps: Vec<SwapView>,
}
