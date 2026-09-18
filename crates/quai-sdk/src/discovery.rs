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
        if self
            .provider
            .chain_id(scope.zone.into())
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?
            != scope.chain_id
            || self
                .provider
                .genesis_hash(scope.zone)
                .await
                .map_err(|_| DiscoveryError::SourceUnavailable)?
                != scope.genesis
        {
            return Err(DiscoveryError::InvalidObservation);
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
            .map_err(|_| DiscoveryError::SourceUnavailable)?
            .ok_or(DiscoveryError::SourceUnavailable)?;
        Ok(ScopedCheckpoint {
            scope,
            checkpoint: Checkpoint {
                hash: header.hash,
                height: U256::from(header.number),
            },
        })
    }
    async fn observe(
        &self,
        scope: NetworkScope,
        address: &DerivedAddress,
        checkpoint: Checkpoint,
    ) -> Result<AddressObservation, DiscoveryError> {
        if address.coin != CoinType::Quai {
            return Err(DiscoveryError::SourceUnavailable);
        }
        let account = QuaiAddress::try_from(address.address)
            .map_err(|_| DiscoveryError::InvalidObservation)?;
        if account.zone() != scope.zone {
            return Err(DiscoveryError::InvalidObservation);
        }
        // Establish chain and genesis identity once for this observation.
        //
        // This previously ran three times per observed address: once here and
        // once inside each bracketing `canonical` call, each re-reading
        // `genesis_hash`, which is the height-zero header and immutable for the
        // life of the chain. The reorg bracket below is the property that
        // matters and is unchanged; only the repeated identity reads are gone.
        //
        // Every provider read now carries its own chain-ID guard in the same
        // request, so a mid-observation backend swap is still caught per call,
        // and a swap to a different chain would also fail the header checks
        // below, which compare the exact hash at the pinned height.
        self.identity(scope).await?;
        let before = self
            .canonical_at(scope, checkpoint.height)
            .await?
            .ok_or(DiscoveryError::SourceUnavailable)?;
        if before.checkpoint != checkpoint {
            return Err(DiscoveryError::InvalidObservation);
        }
        let block = BlockTag::Number(checkpoint.height);
        let balance = self
            .provider
            .balance(account, block)
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?;
        let nonce = self
            .provider
            .transaction_count(account, block)
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?;
        // Closing half of the reorg bracket: the pinned height must still carry
        // the same hash after the balance and nonce reads, or they may describe
        // a block that is no longer canonical.
        if self
            .canonical_at(scope, checkpoint.height)
            .await?
            .is_none_or(|v| v.checkpoint != checkpoint)
        {
            return Err(DiscoveryError::InvalidObservation);
        }
        Ok(AddressObservation {
            scope,
            checkpoint,
            address: address.address,
            ever_used: None,
            account_balance: Some(balance),
            account_nonce: Some(nonce),
            coins: vec![],
        })
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
impl<T: Transport + SourceConcurrency> AccountRpcSource<'_, T> {
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
            .map_err(|_| DiscoveryError::SourceUnavailable)?;
        Ok(header.map(|header| ScopedCheckpoint {
            scope,
            checkpoint: Checkpoint {
                hash: header.hash,
                height: U256::from(header.number),
            },
        }))
    }
}
