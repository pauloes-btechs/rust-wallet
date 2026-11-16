//! The async surface every node backend implements.

#![allow(async_fn_in_trait)]

use crate::views::*;
use crate::{ApiResult, CapabilitySet};

/// Implemented by `rw_litd::LitdBackend` (live gRPC) and
/// `rw_replay::ReplayBackend` (recorded regtest responses).
///
/// Every mutating call must check [`NodeApi::capabilities`] first. Lightning
/// sends go through the payment-intent engine in the caller, never straight
/// to `pay_invoice` from UI code.
pub trait NodeApi {
    fn capabilities(&self) -> &CapabilitySet;

    async fn state(&self) -> ApiResult<StateView>;
    async fn wallet_balance(&self) -> ApiResult<WalletBalanceView>;
    async fn transactions(&self) -> ApiResult<TransactionsView>;
    async fn new_address(&self) -> ApiResult<AddressView>;
    async fn estimate_fee(&self, req: &SendOnchainRequest) -> ApiResult<FeeEstimateView>;
    async fn send_onchain(&self, req: &SendOnchainRequest) -> ApiResult<TxidView>;

    async fn channels(&self) -> ApiResult<ChannelsView>;
    async fn open_channel(&self, req: &OpenChannelRequest) -> ApiResult<OpenChannelResult>;
    async fn channel_backup(&self) -> ApiResult<ChannelBackupView>;
    async fn peers(&self) -> ApiResult<PeersView>;

    async fn payments(&self) -> ApiResult<PaymentsView>;
    async fn invoices(&self) -> ApiResult<InvoicesView>;
    async fn create_invoice(&self, req: &CreateInvoiceRequest) -> ApiResult<CreatedInvoiceView>;
    async fn decode_invoice(&self, payment_request: &str) -> ApiResult<DecodedInvoiceView>;
    async fn pay_invoice(&self, req: &PayRequest) -> ApiResult<PaymentOutcomeView>;

    async fn diagnostics(&self) -> ApiResult<DiagnosticsView>;
    async fn services(&self) -> ApiResult<ServicesView>;
    async fn assets(&self) -> ApiResult<AssetsView>;
    async fn swaps(&self) -> ApiResult<SwapsView>;
}
