//! Portable numbered account observations and bounded current Qi outpoint discovery.
mod qi;
pub use qi::{
    CurrentQiAddress, CurrentQiDiscovery, CurrentQiOutput, DEFAULT_QI_GAP, ObservedQiBalance,
    QiDiscoveryError, QiDiscoveryOptions, discover_qi,
};
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
        self.identity(scope).await?;
        let before = self
            .canonical(scope, checkpoint.height)
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
        if self
            .canonical(scope, checkpoint.height)
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
    async fn canonical(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        self.identity(scope).await?;
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
