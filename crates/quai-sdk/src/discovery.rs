//! Portable numbered account observations and bounded current Qi outpoint discovery.
mod qi;
mod qi_addresses;
pub use qi::{
    CurrentQiAddress, CurrentQiDiscovery, CurrentQiOutput, DEFAULT_QI_GAP, ObservedQiBalance,
    QiDiscoveryError, QiDiscoveryOptions, discover_qi, discover_qi_with_use_checker,
};
pub use qi_addresses::{refresh_qi_address_book, refresh_qi_address_book_with_use_checker};
use quai_primitives::QuaiAddress;
use quai_provider::{BlockTag, Provider};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{
    AddressObservation, Checkpoint, DiscoveryError, HistoryCapability, NetworkScope,
    ObservationSource, ScopedCheckpoint,
};
use quai_wallet::{CoinType, DerivedAddress};

#[cfg(not(target_arch = "wasm32"))]
use std::marker::Sync as SourceConcurrency;
// Browser transports own thread-local JS handles; native discovery futures
// retain their existing Send/Sync contract.
#[cfg(target_arch = "wasm32")]
trait SourceConcurrency {}
#[cfg(target_arch = "wasm32")]
impl<T> SourceConcurrency for T {}

/// Account-ledger observation source backed by explicit numbered RPC reads.
///
/// Current balance and nonce cannot prove an address has never received/spent
/// value. Historical ever-used coverage is therefore unavailable. Qi is refused:
/// the node's latest-only outpoint method is not a block-pinned snapshot API.
pub struct AccountRpcSource<'a, T> {
    provider: &'a Provider<T>,
}
impl<'a, T: Transport> AccountRpcSource<'a, T> {
    /// Borrow an application-selected provider; no network call occurs.
    pub fn new(provider: &'a Provider<T>) -> Self {
        Self { provider }
    }
    async fn identity(&self, scope: NetworkScope) -> Result<(), DiscoveryError> {
        if !crate::network::on_network(self.provider, scope, scope.zone)
            .await
            .map_err(source_error)?
        {
            return Err(DiscoveryError::NetworkMismatch);
        }
        Ok(())
    }
}
impl<T: Transport + SourceConcurrency> ObservationSource for AccountRpcSource<'_, T> {
    fn history_capability(&self, _: NetworkScope, _: CoinType) -> HistoryCapability {
        HistoryCapability::CurrentStateOnly
    }
    async fn tip(&self, scope: NetworkScope) -> Result<ScopedCheckpoint, DiscoveryError> {
        self.identity(scope).await?;
        let header = self
            .provider
            .latest_header(scope.zone)
            .await
            .map_err(source_error)?
            .ok_or(DiscoveryError::SourceUnavailable)?;
        Ok(ScopedCheckpoint {
            scope,
            checkpoint: crate::network::checkpoint(&header),
        })
    }
    async fn observe(
        &self,
        scope: NetworkScope,
        address: &DerivedAddress,
        checkpoint: Checkpoint,
    ) -> Result<AddressObservation, DiscoveryError> {
        self.observe_many(scope, std::slice::from_ref(address), checkpoint)
            .await?
            .pop()
            .ok_or(DiscoveryError::InvalidObservation)
    }
    /// One identity check and one reorg bracket for the whole window, with the
    /// balance and nonce reads batched between them.
    async fn observe_many(
        &self,
        scope: NetworkScope,
        addresses: &[DerivedAddress],
        checkpoint: Checkpoint,
    ) -> Result<Vec<AddressObservation>, DiscoveryError> {
        let accounts = addresses
            .iter()
            .map(|address| {
                if address.coin != CoinType::Quai {
                    return Err(DiscoveryError::SourceUnavailable);
                }
                QuaiAddress::try_from(address.address)
                    .ok()
                    .filter(|account| account.zone() == scope.zone)
                    .ok_or(DiscoveryError::InvalidObservation)
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Genesis is immutable, so identity is established once per window;
        // every provider read still carries its own chain-ID guard, so a
        // mid-window backend swap is caught per call.
        self.identity(scope).await?;
        let before = self
            .canonical_at(scope, checkpoint.height)
            .await?
            .ok_or(DiscoveryError::SourceUnavailable)?;
        if before.checkpoint != checkpoint {
            return Err(DiscoveryError::InvalidObservation);
        }
        let states = self
            .provider
            .account_states(&accounts, BlockTag::Number(checkpoint.height))
            .await
            .map_err(source_error)?;
        // Closing half of the reorg bracket: the pinned height must still carry
        // the same hash after the reads, or they may describe a block that is
        // no longer canonical.
        if self
            .canonical_at(scope, checkpoint.height)
            .await?
            .is_none_or(|v| v.checkpoint != checkpoint)
        {
            return Err(DiscoveryError::InvalidObservation);
        }
        if states.len() != addresses.len() {
            return Err(DiscoveryError::InvalidObservation);
        }
        Ok(addresses
            .iter()
            .zip(states)
            .map(|(address, state)| AddressObservation {
                scope,
                checkpoint,
                address: address.address,
                ever_used: None,
                account_balance: Some(state.balance),
                account_nonce: Some(state.nonce),
                coins: vec![],
            })
            .collect())
    }
    /// Check identity, then read the canonical checkpoint at a height.
    ///
    /// This is the trait entry point, used by callers that have not already
    /// established identity for the surrounding operation.
    async fn canonical(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        self.identity(scope).await?;
        self.canonical_at(scope, height).await
    }
}
impl<T: Transport> AccountRpcSource<'_, T> {
    /// Read the canonical checkpoint at a height, assuming identity is already
    /// established for this observation. Callers that have not checked identity
    /// must use `canonical`.
    async fn canonical_at(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        if height == U256::ZERO {
            return Err(DiscoveryError::InvalidRequest);
        }
        let height = u64::try_from(height).map_err(|_| DiscoveryError::InvalidRequest)?;
        let header = self
            .provider
            .header_at(scope.zone, height)
            .await
            .map_err(source_error)?;
        Ok(header.map(|header| ScopedCheckpoint {
            scope,
            checkpoint: crate::network::checkpoint(&header),
        }))
    }
}

/// A provider failure as a discovery error. A chain mismatch is reported as
/// such rather than as an outage, so a sync loop stops instead of retrying.
fn source_error(error: quai_provider::ProviderError) -> DiscoveryError {
    match error {
        quai_provider::ProviderError::ChainMismatch { .. } => DiscoveryError::NetworkMismatch,
        _ => DiscoveryError::SourceUnavailable,
    }
}
