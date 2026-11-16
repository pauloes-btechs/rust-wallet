//! Pure proto → view mappers. No I/O, so every rule here is unit-tested.

use crate::proto::{frdrpc, lnrpc, looprpc, taprpc};
use rw_core::views::*;
use std::collections::BTreeMap;

/// LND reports balances as int64; a negative value is a node bug, clamp to 0.
fn u(v: i64) -> u64 {
    v.max(0) as u64
}

pub fn node_health(r: &lnrpc::GetInfoResponse) -> NodeHealthView {
    NodeHealthView {
        identity_pubkey: r.identity_pubkey.clone(),
        alias: r.alias.clone(),
        version: r.version.clone(),
        synced_to_chain: r.synced_to_chain,
        synced_to_graph: r.synced_to_graph,
        block_height: r.block_height,
        num_peers: r.num_peers,
        num_active_channels: r.num_active_channels,
        num_pending_channels: r.num_pending_channels,
        num_inactive_channels: r.num_inactive_channels,
        uris: r.uris.clone(),
        // `Chain.chain` is deprecated upstream: LND only supports bitcoin.
        chains: r.chains.iter().map(|c| ChainView { chain: "bitcoin".into(), network: c.network.clone() }).collect(),
    }
}

/// Classify a GetInfo failure into a wallet phase.
pub fn phase_from_status(s: &tonic::Status) -> Phase {
    let m = s.message().to_ascii_lowercase();
    if m.contains("wallet locked") || m.contains("unlock it") {
        Phase::Locked
    } else if m.contains("wallet not created") || m.contains("create one") {
        Phase::Missing
    } else {
        Phase::Undetermined
    }
}

pub fn wallet_balance(r: &lnrpc::WalletBalanceResponse) -> WalletBalanceView {
    WalletBalanceView {
        total_sat: u(r.total_balance),
        confirmed_sat: u(r.confirmed_balance),
        unconfirmed_sat: u(r.unconfirmed_balance),
        locked_sat: u(r.locked_balance),
        reserved_anchor_sat: u(r.reserved_balance_anchor_chan),
    }
}

fn amt(a: &Option<lnrpc::Amount>) -> u64 {
    a.as_ref().map(|a| a.sat).unwrap_or(0)
}

pub fn channel_balance(r: &lnrpc::ChannelBalanceResponse) -> ChannelBalanceView {
    ChannelBalanceView {
        local_sat: amt(&r.local_balance),
        remote_sat: amt(&r.remote_balance),
        local_unsettled_sat: amt(&r.unsettled_local_balance),
        pending_open_local_sat: amt(&r.pending_open_local_balance),
        pending_open_remote_sat: amt(&r.pending_open_remote_balance),
    }
}

pub fn commitment_name(v: i32) -> Option<String> {
    lnrpc::CommitmentType::try_from(v).ok().map(|c| c.as_str_name().to_ascii_lowercase())
}

pub fn open_channel_row(c: &lnrpc::Channel) -> ChannelRowView {
    ChannelRowView {
        channel_point: c.channel_point.clone(),
        chan_id: c.chan_id.to_string(),
        remote_pubkey: c.remote_pubkey.clone(),
        capacity_sat: u(c.capacity),
        local_balance_sat: u(c.local_balance),
        remote_balance_sat: u(c.remote_balance),
        state: if c.active { ChannelState::Open } else { ChannelState::OpenInactive },
        initiator_label: if c.initiator { "opened by you" } else { "opened by peer" }.into(),
        commitment_type: commitment_name(c.commitment_type),
    }
}

fn pending_row(c: &lnrpc::pending_channels_response::PendingChannel, state: ChannelState) -> ChannelRowView {
    use lnrpc::Initiator;
    let initiator = match Initiator::try_from(c.initiator) {
        Ok(Initiator::Local) => "opened by you",
        Ok(Initiator::Remote) => "opened by peer",
        Ok(Initiator::Both) => "dual-funded",
        _ => "unknown",
    };
    ChannelRowView {
        channel_point: c.channel_point.clone(),
        chan_id: String::new(),
        remote_pubkey: c.remote_node_pub.clone(),
        capacity_sat: u(c.capacity),
        local_balance_sat: u(c.local_balance),
        remote_balance_sat: u(c.remote_balance),
        state,
        initiator_label: initiator.into(),
        commitment_type: commitment_name(c.commitment_type),
    }
}

pub fn channels(
    open: &lnrpc::ListChannelsResponse,
    pending: &lnrpc::PendingChannelsResponse,
    balance: &lnrpc::ChannelBalanceResponse,
) -> ChannelsView {
    let mut p = Vec::new();
    for c in &pending.pending_open_channels {
        if let Some(ch) = &c.channel {
            p.push(pending_row(ch, ChannelState::Opening));
        }
    }
    for c in &pending.waiting_close_channels {
        if let Some(ch) = &c.channel {
            p.push(pending_row(ch, ChannelState::WaitingClose));
        }
    }
    for c in &pending.pending_force_closing_channels {
        if let Some(ch) = &c.channel {
            p.push(pending_row(ch, ChannelState::ClosingForce));
        }
    }
    #[allow(deprecated)]
    for c in &pending.pending_closing_channels {
        if let Some(ch) = &c.channel {
            p.push(pending_row(ch, ChannelState::Closing));
        }
    }
    ChannelsView {
        open: open.channels.iter().map(open_channel_row).collect(),
        pending: p,
        balance: channel_balance(balance),
    }
}

pub fn payment_status_name(v: i32) -> String {
    lnrpc::payment::PaymentStatus::try_from(v)
        .map(|s| s.as_str_name().to_ascii_lowercase())
        .unwrap_or_else(|_| "unknown".into())
}

pub fn failure_reason_name(v: i32) -> Option<String> {
    match lnrpc::PaymentFailureReason::try_from(v) {
        Ok(lnrpc::PaymentFailureReason::FailureReasonNone) | Err(_) => None,
        Ok(r) => Some(r.as_str_name().trim_start_matches("FAILURE_REASON_").to_ascii_lowercase()),
    }
}

pub fn payment(p: &lnrpc::Payment) -> PaymentView {
    PaymentView {
        payment_hash: p.payment_hash.clone(),
        status: payment_status_name(p.status),
        value_sat: u(p.value_sat),
        fee_sat: u(p.fee_sat),
        creation_time_ns: u(p.creation_time_ns),
        failure_reason: failure_reason_name(p.failure_reason),
        payment_request: (!p.payment_request.is_empty()).then(|| p.payment_request.clone()),
    }
}

pub fn invoice(i: &lnrpc::Invoice) -> InvoiceRowView {
    let state = lnrpc::invoice::InvoiceState::try_from(i.state)
        .map(|s| s.as_str_name().to_ascii_lowercase())
        .unwrap_or_else(|_| "unknown".into());
    InvoiceRowView {
        payment_hash_hex: hex::encode(&i.r_hash),
        payment_request: i.payment_request.clone(),
        memo: i.memo.clone(),
        state,
        value_sat: u(i.value),
        paid_sat: u(i.amt_paid_sat),
        is_keysend: i.is_keysend,
        created_at: i.creation_date * 1000,
        expires_at: (i.creation_date + i.expiry) * 1000,
        settled_at: (i.settle_date > 0).then_some(i.settle_date * 1000),
    }
}

pub fn decoded(p: &lnrpc::PayReq) -> DecodedInvoiceView {
    DecodedInvoiceView {
        payment_hash_hex: p.payment_hash.clone(),
        amount_sat: (p.num_satoshis > 0).then_some(p.num_satoshis as u64),
        memo: p.description.clone(),
        destination: p.destination.clone(),
        cltv_expiry: u(p.cltv_expiry),
    }
}

pub fn transaction(t: &lnrpc::Transaction) -> OnchainTxView {
    OnchainTxView {
        txid: t.tx_hash.clone(),
        amount_sat: t.amount,
        fee_sat: u(t.total_fees),
        block_height: t.block_height,
        confirmations: t.num_confirmations,
        at: t.time_stamp * 1000,
        label: t.label.clone(),
        // `dest_addresses` is deprecated upstream; output_details carries them.
        addresses: t.output_details.iter().filter(|o| !o.address.is_empty()).map(|o| o.address.clone()).collect(),
    }
}

pub fn peer(p: &lnrpc::Peer) -> PeerView {
    PeerView {
        pubkey: p.pub_key.clone(),
        address: p.address.clone(),
        inbound: p.inbound,
        sat_sent: u(p.sat_sent),
        sat_received: u(p.sat_recv),
        // LND reports ping_time in microseconds.
        ping_ms: u(p.ping_time) / 1000,
        flap_count: p.flap_count.max(0) as u32,
    }
}

/// LND returns the funding txid as bytes in internal (reversed) order.
pub fn channel_point_string(cp: &lnrpc::ChannelPoint) -> String {
    use lnrpc::channel_point::FundingTxid;
    let txid = match &cp.funding_txid {
        Some(FundingTxid::FundingTxidStr(s)) => s.clone(),
        Some(FundingTxid::FundingTxidBytes(b)) => {
            let mut b = b.clone();
            b.reverse();
            hex::encode(b)
        }
        None => String::new(),
    };
    format!("{txid}:{}", cp.output_index)
}

pub fn closed_channel(c: &lnrpc::ChannelCloseSummary) -> ClosedChannelView {
    ClosedChannelView {
        channel_point: c.channel_point.clone(),
        chan_id: c.chan_id.to_string(),
        remote_pubkey: c.remote_pubkey.clone(),
        closing_tx_hash: c.closing_tx_hash.clone(),
        close_type: lnrpc::channel_close_summary::ClosureType::try_from(c.close_type)
            .map(|t| t.as_str_name().to_ascii_lowercase())
            .unwrap_or_else(|_| "unknown".into()),
        close_height: c.close_height,
        settled_balance_sat: u(c.settled_balance),
        time_locked_balance_sat: u(c.time_locked_balance),
    }
}

pub fn forwarding_event(e: &lnrpc::ForwardingEvent) -> ForwardingEventView {
    ForwardingEventView { timestamp_ns: e.timestamp_ns, amt_in_sat: e.amt_in, amt_out_sat: e.amt_out, fee_sat: e.fee }
}

pub fn insight(i: &frdrpc::ChannelInsight) -> ChannelInsightView {
    ChannelInsightView {
        chan_point: i.chan_point.clone(),
        monitored_seconds: i.monitored_seconds,
        uptime_seconds: i.uptime_seconds,
        uptime_ratio: (i.monitored_seconds > 0).then(|| i.uptime_seconds as f64 / i.monitored_seconds as f64),
        volume_incoming_msat: u(i.volume_incoming_msat),
        volume_outgoing_msat: u(i.volume_outgoing_msat),
        fees_earned_msat: u(i.fees_earned_msat),
        is_private: i.private,
    }
}

pub fn asset_balances(r: &taprpc::ListBalancesResponse) -> Vec<AssetBalanceView> {
    let mut v: Vec<_> = r
        .asset_balances
        .iter()
        .map(|(id, b)| {
            let g = b.asset_genesis.clone().unwrap_or_default();
            AssetBalanceView {
                asset_id_hex: if g.asset_id.is_empty() { id.clone() } else { hex::encode(&g.asset_id) },
                name: g.name,
                asset_type: taprpc::AssetType::try_from(g.asset_type)
                    .map(|t| t.as_str_name().to_ascii_lowercase())
                    .unwrap_or_else(|_| "unknown".into()),
                balance: b.balance,
            }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name).then(a.asset_id_hex.cmp(&b.asset_id_hex)));
    v
}

pub fn swap(s: &looprpc::SwapStatus) -> SwapView {
    let swap_type = looprpc::SwapType::try_from(s.r#type)
        .map(|t| t.as_str_name().to_ascii_lowercase().replace('_', "-"))
        .unwrap_or_else(|_| format!("type-{}", s.r#type));
    SwapView {
        // `id` (string) is deprecated upstream in favour of `id_bytes`.
        id: hex::encode(&s.id_bytes),
        swap_type,
        state: looprpc::SwapState::try_from(s.state)
            .map(|t| t.as_str_name().to_ascii_lowercase())
            .unwrap_or_else(|_| "unknown".into()),
        amount_sat: u(s.amt),
        cost_sat: u(s.cost_server) + u(s.cost_onchain) + u(s.cost_offchain),
        initiation_time_ns: s.initiation_time,
    }
}

pub fn sub_servers(
    m: &std::collections::HashMap<String, crate::proto::litrpc::SubServerStatus>,
) -> BTreeMap<String, SubServerView> {
    m.iter()
        .map(|(k, s)| {
            (
                k.clone(),
                SubServerView {
                    disabled: s.disabled,
                    running: s.running,
                    error: s.error.clone(),
                    custom_status: s.custom_status.clone(),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_point_bytes_are_reversed() {
        use lnrpc::channel_point::FundingTxid;
        let cp = lnrpc::ChannelPoint {
            output_index: 1,
            funding_txid: Some(FundingTxid::FundingTxidBytes(vec![0x01, 0x02, 0xff])),
        };
        assert_eq!(channel_point_string(&cp), "ff0201:1");
        let cp = lnrpc::ChannelPoint { output_index: 0, funding_txid: Some(FundingTxid::FundingTxidStr("ab".into())) };
        assert_eq!(channel_point_string(&cp), "ab:0");
    }

    #[test]
    fn payment_names_match_the_recorded_contract() {
        let p = lnrpc::Payment {
            payment_hash: "h".into(),
            status: lnrpc::payment::PaymentStatus::Succeeded as i32,
            value_sat: 2_000_000,
            ..Default::default()
        };
        let v = payment(&p);
        assert_eq!(v.status, "succeeded");
        assert_eq!(v.failure_reason, None);
        assert_eq!(v.payment_request, None);
        let f = lnrpc::Payment {
            status: lnrpc::payment::PaymentStatus::Failed as i32,
            failure_reason: lnrpc::PaymentFailureReason::FailureReasonNoRoute as i32,
            ..Default::default()
        };
        assert_eq!(payment(&f).failure_reason.as_deref(), Some("no_route"));
    }

    #[test]
    fn negative_balances_clamp() {
        let r = lnrpc::WalletBalanceResponse { total_balance: -5, confirmed_balance: 10, ..Default::default() };
        let v = wallet_balance(&r);
        assert_eq!((v.total_sat, v.confirmed_sat), (0, 10));
    }

    #[test]
    fn invoice_times_are_millis() {
        let i = lnrpc::Invoice {
            r_hash: vec![0xab, 0xcd],
            creation_date: 1_000,
            expiry: 3_600,
            state: lnrpc::invoice::InvoiceState::Settled as i32,
            settle_date: 1_010,
            value: 12_000,
            amt_paid_sat: 12_000,
            ..Default::default()
        };
        let v = invoice(&i);
        assert_eq!(v.payment_hash_hex, "abcd");
        assert_eq!((v.created_at, v.expires_at, v.settled_at), (1_000_000, 4_600_000, Some(1_010_000)));
        assert_eq!(v.state, "settled");
    }

    #[test]
    fn phase_classification() {
        assert_eq!(
            phase_from_status(&tonic::Status::unknown("wallet locked, unlock it to enable full RPC access")),
            Phase::Locked
        );
        assert_eq!(
            phase_from_status(&tonic::Status::unknown("wallet not created, create one to enable full RPC access")),
            Phase::Missing
        );
        assert_eq!(phase_from_status(&tonic::Status::unavailable("connection refused")), Phase::Undetermined);
    }

    #[test]
    fn channels_merge_pending_kinds() {
        use lnrpc::pending_channels_response as pcr;
        let pc = pcr::PendingChannel {
            channel_point: "x:0".into(),
            capacity: 100,
            initiator: lnrpc::Initiator::Local as i32,
            ..Default::default()
        };
        let pending = lnrpc::PendingChannelsResponse {
            pending_open_channels: vec![pcr::PendingOpenChannel { channel: Some(pc.clone()), ..Default::default() }],
            waiting_close_channels: vec![pcr::WaitingCloseChannel { channel: Some(pc), ..Default::default() }],
            ..Default::default()
        };
        let open = lnrpc::ListChannelsResponse {
            channels: vec![lnrpc::Channel { active: false, chan_id: 7, initiator: true, ..Default::default() }],
        };
        let v = channels(&open, &pending, &Default::default());
        assert_eq!(v.open[0].state, ChannelState::OpenInactive);
        assert_eq!(v.open[0].chan_id, "7");
        assert_eq!(
            v.pending.iter().map(|r| r.state).collect::<Vec<_>>(),
            [ChannelState::Opening, ChannelState::WaitingClose]
        );
        assert_eq!(v.pending[0].initiator_label, "opened by you");
    }
}
