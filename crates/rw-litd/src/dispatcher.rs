//! The LND side of the payment engine.
//!
//! * Lightning: `prepare` decodes the invoice to get the payment hash (the
//!   reconciliation key) before anything is sent; `dispatch` calls
//!   `Router.SendPaymentV2` once; `lookup` calls `Router.TrackPaymentV2`.
//! * On-chain: LND's `SendCoins` builds, signs and broadcasts in one call, so
//!   the txid is not known in advance. Instead the intent's reconciliation key
//!   is a unique transaction **label** (`rw-intent:<id>`), persisted before the
//!   broadcast. `lookup` searches `GetTransactions` for that label; a missing
//!   label is ambiguous (needs recovery), not proof the send never happened.

use crate::client::LitdClient;
use crate::proto::{lnrpc, routerrpc};
use rw_payments::{DispatchOutcome, Dispatcher, PaymentIntent, PaymentKind, ReconcileResult};
use std::time::Duration;
use tonic::Code;

const PAY_TIMEOUT: Duration = Duration::from_secs(60);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
pub const LABEL_PREFIX: &str = "rw-intent:";

/// Default routing-fee cap when the caller gives none: 1% of the amount,
/// at least 10 sats.
pub fn default_fee_cap_sat(amount_sat: u64) -> u64 {
    (amount_sat / 100).max(10)
}

pub struct LndDispatcher {
    client: LitdClient,
}

impl LndDispatcher {
    pub fn new(client: LitdClient) -> Self {
        Self { client }
    }
}

/// Callers set the cap (see `default_fee_cap_sat`); a fallback here would
/// compute it from the intent amount, which is 0 for fixed-amount invoices.
fn fee_cap(intent: &PaymentIntent) -> Result<u64, String> {
    intent.max_fee_sat.ok_or_else(|| "lightning payment needs a fee cap".into())
}

fn label(intent: &PaymentIntent) -> String {
    format!("{LABEL_PREFIX}{}", intent.intent_id)
}

impl Dispatcher for LndDispatcher {
    async fn prepare(&self, intent: &PaymentIntent) -> Result<String, String> {
        match intent.kind {
            PaymentKind::LightningInvoice => {
                fee_cap(intent)?;
                let pr = self
                    .client
                    .lightning()
                    .decode_pay_req(lnrpc::PayReqString { pay_req: intent.destination.clone() })
                    .await
                    .map_err(|s| format!("invoice rejected by node: {}", s.message()))?
                    .into_inner();
                if pr.num_satoshis == 0 && pr.num_msat == 0 && intent.amount_sat.unwrap_or(0) == 0 {
                    return Err("zero-amount invoice needs an amount".into());
                }
                Ok(pr.payment_hash)
            }
            PaymentKind::Onchain => {
                if intent.amount_sat.unwrap_or(0) == 0 {
                    return Err("on-chain send needs an amount".into());
                }
                Ok(label(intent))
            }
        }
    }

    async fn dispatch(&self, intent: &PaymentIntent, backend_ref: &str) -> DispatchOutcome {
        match intent.kind {
            PaymentKind::LightningInvoice => self.pay(intent, backend_ref).await,
            PaymentKind::Onchain => self.send_coins(intent, backend_ref).await,
        }
    }

    async fn lookup(&self, intent: &PaymentIntent) -> ReconcileResult {
        match intent.kind {
            PaymentKind::LightningInvoice => self.track(intent).await,
            PaymentKind::Onchain => self.find_label(intent).await,
        }
    }
}

impl LndDispatcher {
    async fn pay(&self, intent: &PaymentIntent, hash: &str) -> DispatchOutcome {
        // prepare already refused this; nothing has been sent, so failing is safe.
        let Ok(fee_cap) = fee_cap(intent) else {
            return DispatchOutcome::DefinitiveFailure { reason: "lightning payment needs a fee cap".into() };
        };
        let amount = intent.amount_sat.unwrap_or(0);
        let req = routerrpc::SendPaymentRequest {
            payment_request: intent.destination.clone(),
            // Only for zero-amount invoices; LND rejects amt on fixed-amount ones.
            amt: if amount > 0 { amount as i64 } else { 0 },
            fee_limit_sat: fee_cap as i64,
            timeout_seconds: (PAY_TIMEOUT.as_secs() - 5) as i32,
            no_inflight_updates: true,
            ..Default::default()
        };
        let mut router = self.client.router();
        let stream = match router.send_payment_v2(req).await {
            Ok(s) => s.into_inner(),
            Err(s) => return classify_send_error(&s),
        };
        match tokio::time::timeout(PAY_TIMEOUT, final_update(stream)).await {
            Ok(Some(p)) => outcome_for(&p, hash),
            Ok(None) => DispatchOutcome::Sent,
            Err(_) => DispatchOutcome::Timeout,
        }
    }

    async fn send_coins(&self, intent: &PaymentIntent, label: &str) -> DispatchOutcome {
        let req = lnrpc::SendCoinsRequest {
            addr: intent.destination.clone(),
            amount: intent.amount_sat.unwrap_or(0) as i64,
            label: label.to_string(),
            // A user-chosen rate wins; otherwise LND estimates for 6 blocks.
            sat_per_vbyte: intent.sat_per_vbyte.unwrap_or(0),
            target_conf: if intent.sat_per_vbyte.is_some() { 0 } else { 6 },
            ..Default::default()
        };
        match self.client.lightning().send_coins(req).await {
            Ok(r) => DispatchOutcome::Settled { backend_ref: r.into_inner().txid },
            Err(s) => classify_coins_error(&s),
        }
    }

    async fn track(&self, intent: &PaymentIntent) -> ReconcileResult {
        let Some(hash) = intent.backend_ref.as_deref() else { return ReconcileResult::NotFound };
        let Ok(hash_bytes) = hex::decode(hash) else { return ReconcileResult::Unknown };
        let req = routerrpc::TrackPaymentRequest { payment_hash: hash_bytes, no_inflight_updates: false };
        let mut router = self.client.router();
        let stream = match router.track_payment_v2(req).await {
            Ok(s) => s.into_inner(),
            Err(s) if s.code() == Code::NotFound || s.message().contains("not initiated") => {
                return ReconcileResult::NotFound
            }
            Err(_) => return ReconcileResult::Unknown,
        };
        let mut stream = stream;
        // First message is the current state; that is all reconciliation needs.
        match tokio::time::timeout(LOOKUP_TIMEOUT, stream.message()).await {
            Ok(Ok(Some(p))) => match lnrpc::payment::PaymentStatus::try_from(p.status) {
                Ok(lnrpc::payment::PaymentStatus::Succeeded) => {
                    ReconcileResult::Settled { backend_ref: p.payment_hash }
                }
                Ok(lnrpc::payment::PaymentStatus::Failed) => {
                    ReconcileResult::Failed { reason: crate::map::failure_reason_name(p.failure_reason) }
                }
                _ => ReconcileResult::Unknown,
            },
            Ok(Err(s)) if s.code() == Code::NotFound => ReconcileResult::NotFound,
            _ => ReconcileResult::Unknown,
        }
    }

    async fn find_label(&self, intent: &PaymentIntent) -> ReconcileResult {
        let want = label(intent);
        match self.client.lightning().get_transactions(lnrpc::GetTransactionsRequest::default()).await {
            Ok(r) => label_lookup(r.into_inner().transactions, &want),
            Err(_) => ReconcileResult::Unknown,
        }
    }
}

async fn final_update(mut s: tonic::Streaming<lnrpc::Payment>) -> Option<lnrpc::Payment> {
    use lnrpc::payment::PaymentStatus::*;
    let mut last = None;
    while let Ok(Some(p)) = s.message().await {
        let terminal = matches!(lnrpc::payment::PaymentStatus::try_from(p.status), Ok(Succeeded | Failed));
        last = Some(p);
        if terminal {
            break;
        }
    }
    last
}

fn outcome_for(p: &lnrpc::Payment, hash: &str) -> DispatchOutcome {
    use lnrpc::payment::PaymentStatus::*;
    match lnrpc::payment::PaymentStatus::try_from(p.status) {
        Ok(Succeeded) => DispatchOutcome::Settled { backend_ref: hash.to_string() },
        Ok(Failed) => DispatchOutcome::DefinitiveFailure {
            reason: crate::map::failure_reason_name(p.failure_reason).unwrap_or_else(|| "failed".into()),
        },
        _ => DispatchOutcome::Sent,
    }
}

/// Errors from the *initial* SendPaymentV2 call. Validation errors mean the
/// payment never started (definitive); "already in flight/paid" means a
/// previous attempt exists and must be reconciled, never treated as failed.
fn classify_send_error(s: &tonic::Status) -> DispatchOutcome {
    let m = s.message().to_ascii_lowercase();
    if s.code() == Code::AlreadyExists
        || m.contains("already paid")
        || m.contains("in transition")
        || m.contains("already exists")
    {
        return DispatchOutcome::Sent;
    }
    match s.code() {
        Code::InvalidArgument
        | Code::FailedPrecondition
        | Code::OutOfRange
        | Code::PermissionDenied
        | Code::Unauthenticated => DispatchOutcome::DefinitiveFailure { reason: s.message().to_string() },
        _ => DispatchOutcome::Timeout,
    }
}

/// A missing label is never proof that nothing was sent: a timed-out
/// SendCoins may still broadcast, and SendCoins has no idempotency key. So
/// an absent label means needs-recovery (a human decides), never not-found.
fn label_lookup(txs: Vec<lnrpc::Transaction>, want: &str) -> ReconcileResult {
    match txs.into_iter().find(|t| t.label == want) {
        Some(t) => ReconcileResult::Settled { backend_ref: t.tx_hash },
        None => ReconcileResult::Unknown,
    }
}

/// Errors from SendCoins. Only validation and wallet errors prove nothing
/// was broadcast; anything else (transport, duplicate) must be reconciled.
fn classify_coins_error(s: &tonic::Status) -> DispatchOutcome {
    match s.code() {
        Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => {
            DispatchOutcome::DefinitiveFailure { reason: s.message().to_string() }
        }
        // Insufficient funds and similar wallet errors come back as Unknown
        // with a clear message; anything transport-shaped is ambiguous.
        Code::Unknown if s.message().contains("insufficient") => {
            DispatchOutcome::DefinitiveFailure { reason: s.message().to_string() }
        }
        _ => DispatchOutcome::Timeout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_cap_default() {
        assert_eq!(default_fee_cap_sat(0), 10);
        assert_eq!(default_fee_cap_sat(500), 10);
        assert_eq!(default_fee_cap_sat(2_000_000), 20_000);
    }

    #[test]
    fn missing_fee_cap_is_refused_not_defaulted() {
        let mut i = PaymentIntent::new("i", PaymentKind::LightningInvoice, "lnbcrt…", None, None);
        assert!(fee_cap(&i).is_err());
        i.max_fee_sat = Some(500);
        assert_eq!(fee_cap(&i), Ok(500));
    }

    #[test]
    fn duplicate_send_is_ambiguous_not_failed() {
        assert_eq!(
            classify_send_error(&tonic::Status::already_exists("invoice is already paid")),
            DispatchOutcome::Sent
        );
        assert_eq!(classify_send_error(&tonic::Status::unknown("payment is in transition")), DispatchOutcome::Sent);
        assert!(matches!(
            classify_send_error(&tonic::Status::invalid_argument("invoice expired")),
            DispatchOutcome::DefinitiveFailure { .. }
        ));
        assert_eq!(classify_send_error(&tonic::Status::unavailable("eof")), DispatchOutcome::Timeout);
    }

    #[test]
    fn duplicate_send_coins_is_ambiguous_not_failed() {
        assert_eq!(
            classify_coins_error(&tonic::Status::unknown("transaction already exists")),
            DispatchOutcome::Timeout
        );
        assert_eq!(classify_coins_error(&tonic::Status::unavailable("eof")), DispatchOutcome::Timeout);
        assert_eq!(classify_coins_error(&tonic::Status::deadline_exceeded("timeout")), DispatchOutcome::Timeout);
        for s in [
            tonic::Status::invalid_argument("invalid address"),
            tonic::Status::unknown("insufficient funds available to construct transaction"),
        ] {
            assert!(matches!(classify_coins_error(&s), DispatchOutcome::DefinitiveFailure { .. }), "{s:?}");
        }
    }

    #[test]
    fn missing_onchain_label_is_unknown_not_not_found() {
        let tx = |label: &str| lnrpc::Transaction { label: label.into(), tx_hash: "tx".into(), ..Default::default() };
        assert_eq!(label_lookup(vec![tx("other")], "rw-intent:1"), ReconcileResult::Unknown);
        assert_eq!(
            label_lookup(vec![tx("other"), tx("rw-intent:1")], "rw-intent:1"),
            ReconcileResult::Settled { backend_ref: "tx".into() }
        );
    }

    #[test]
    fn terminal_payment_mapping() {
        let ok = lnrpc::Payment { status: lnrpc::payment::PaymentStatus::Succeeded as i32, ..Default::default() };
        assert_eq!(outcome_for(&ok, "h"), DispatchOutcome::Settled { backend_ref: "h".into() });
        let inflight = lnrpc::Payment { status: lnrpc::payment::PaymentStatus::InFlight as i32, ..Default::default() };
        assert_eq!(outcome_for(&inflight, "h"), DispatchOutcome::Sent);
    }
}
