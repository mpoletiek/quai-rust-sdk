//! Scoped, advisory Qi address inventory and cached usage views. Never allocation authority.
use crate::discovery::{Checkpoint, NetworkScope};
use crate::metadata::{PublicAddress, StorageError};
use crate::{AccountPublic, CoinType};
use quai_crypto::PublicKey;
#[cfg(feature = "payments")]
use quai_payments::{PaymentCode, PrivatePaymentCode};
use quai_primitives::{Address, Hash32, QiAddress};
use std::collections::{BTreeMap, BTreeSet};

/// Maximum addresses in one in-memory view. No implicit eviction.
pub const MAX_QI_ADDRESS_RECORDS: usize = 4096;
/// Advisory status matching the reference's four states. Unused only means an
/// explicitly recorded empty observation; none of these values authorize reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QiAddressStatus {
    /// No retained current observation.
    Unknown,
    /// Observed empty without a positive use hint or prior recorded use.
    Unused,
    /// A positive current-output/use-hint observation has been retained.
    Used,
    /// Explicitly marked attempted; no execution or inclusion is inferred.
    AttemptedUse,
}
/// Exact public origin for lookup; imported keys are not assigned an HD account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QiAddressOrigin {
    /// Validated BIP44 address.
    Bip44 {
        /// Exact unhardened account number.
        account: u32,
        /// Internal/change branch when true.
        change: bool,
        /// Exact raw nonhardened child index.
        index: u32,
    },
    /// Explicit public-key import, without ancestry.
    Imported,
    /// Locally verified BIP47 receive derivation; no send address is owned here.
    #[cfg(feature = "payments")]
    PaymentReceive {
        /// Owner's BIP47 account number.
        account: u32,
        /// Explicit sender payment code.
        peer: Box<PaymentCode>,
        /// Exact raw receiving child index.
        index: u32,
    },
}
/// Immutable record inspected through the enclosing book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiAddressRecord {
    public: PublicAddress,
    origin: QiAddressOrigin,
    status: QiAddressStatus,
    checkpoint: Option<Checkpoint>,
}
impl QiAddressRecord {
    /// Verified public metadata, containing no private material.
    pub fn public(&self) -> &PublicAddress {
        &self.public
    }
    /// Exact origin including payment account/peer context where applicable.
    pub fn origin(&self) -> &QiAddressOrigin {
        &self.origin
    }
    /// Advisory cache status, never a spend or allocation permission.
    pub fn status(&self) -> QiAddressStatus {
        self.status
    }
    /// Last caller-supplied observation; canonicality is not proven by the cache.
    pub fn checkpoint(&self) -> Option<Checkpoint> {
        self.checkpoint
    }
}
/// One explicitly scoped usage observation. Does not contain coins or spend claims.
#[derive(Clone, Copy, Debug)]
pub struct QiUsageObservation {
    /// Known Qi address in the enclosing book's zone.
    pub address: QiAddress,
    /// True for nonempty current outputs or an explicitly positive use hint.
    pub used: bool,
}
/// Bounded public inventory and usage cache for one exact network/zone.
/// No serialization, private keys, nonce/UTXO claims or allocation cursors. Rebuild
/// from verified origins/journals after restart and refresh before relying on views.
#[derive(Clone, Debug)]
pub struct QiAddressBook {
    scope: NetworkScope,
    records: BTreeMap<Address, QiAddressRecord>,
}
impl QiAddressBook {
    /// Start an empty inventory bound to a nonzero chain ID and trusted genesis.
    pub fn new(scope: NetworkScope) -> Result<Self, StorageError> {
        if scope.chain_id == quai_consensus::U256::ZERO || scope.genesis == Hash32::ZERO {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            scope,
            records: BTreeMap::new(),
        })
    }
    /// Immutable network/zone binding.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// All known origins, in address-byte order.
    pub fn addresses(&self) -> impl Iterator<Item = &QiAddressRecord> {
        self.records.values()
    }
    /// Exact address lookup, independent of textual checksum casing.
    pub fn address(&self, address: QiAddress) -> Option<&QiAddressRecord> {
        self.records.get(&address.address())
    }
    fn insert(
        &mut self,
        public: PublicAddress,
        origin: QiAddressOrigin,
    ) -> Result<(), StorageError> {
        let address = QiAddress::try_from(public.address()).map_err(|_| StorageError::Invalid)?;
        if address.zone() != self.scope.zone {
            return Err(StorageError::Invalid);
        }
        if let Some(old) = self.records.get(&public.address()) {
            return if old.public == public && old.origin == origin {
                Ok(())
            } else {
                Err(StorageError::Conflict)
            };
        }
        if self.records.len() >= MAX_QI_ADDRESS_RECORDS {
            return Err(StorageError::Invalid);
        }
        self.records.insert(
            public.address(),
            QiAddressRecord {
                public,
                origin,
                status: QiAddressStatus::Unknown,
                checkpoint: None,
            },
        );
        Ok(())
    }
    /// Import one exact BIP44 child, verifying its ancestry and ledger/zone.
    /// Idempotent identical imports preserve usage; conflicting ancestry rejects.
    /// This never allocates a fresh address or advances a durable derivation floor.
    pub fn import_hd(
        &mut self,
        account: &AccountPublic,
        change: bool,
        index: u32,
    ) -> Result<(), StorageError> {
        if account.coin_type() != CoinType::Qi {
            return Err(StorageError::Invalid);
        }
        let public = PublicAddress::derive(account, change, index)?;
        self.insert(
            public,
            QiAddressOrigin::Bip44 {
                account: account.account_index(),
                change,
                index,
            },
        )
    }
    /// Import a public Qi key. Starts Unknown; possession of a secret is not inferred.
    pub fn import_public(&mut self, key: &PublicKey) -> Result<(), StorageError> {
        self.insert(PublicAddress::imported(key)?, QiAddressOrigin::Imported)
    }
    /// Verify and import a known BIP47 receive exposure. Its durable allocation or
    /// authenticated recovery must be handled separately before exposing it anew.
    #[cfg(feature = "payments")]
    pub fn import_payment_receive(
        &mut self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
        index: u32,
    ) -> Result<(), StorageError> {
        let key = owner
            .receive_public_key(peer, index)
            .map_err(|_| StorageError::Invalid)?;
        self.insert(
            PublicAddress::imported(&key)?,
            QiAddressOrigin::PaymentReceive {
                account: owner.account(),
                peer: Box::new(peer.clone()),
                index,
            },
        )
    }
    /// BIP44 external or change addresses, excluding imports and payment channels.
    pub fn branch(&self, change: bool) -> impl Iterator<Item = &QiAddressRecord> {
        self.addresses()
            .filter(move |r| matches!(r.origin, QiAddressOrigin::Bip44 {change:c,..} if c==change))
    }
    /// All exact HD/payment account origins. Plain imported keys have no account.
    pub fn account(&self, account: u32) -> impl Iterator<Item = &QiAddressRecord> {
        self.addresses().filter(move |r| match r.origin {
            QiAddressOrigin::Bip44 { account: a, .. } => a == account,
            #[cfg(feature = "payments")]
            QiAddressOrigin::PaymentReceive { account: a, .. } => a == account,
            QiAddressOrigin::Imported => false,
        })
    }
    /// Direct imports only; BIP47 receive metadata has a separate origin here.
    pub fn imported(&self) -> impl Iterator<Item = &QiAddressRecord> {
        self.addresses()
            .filter(|r| matches!(r.origin, QiAddressOrigin::Imported))
    }
    /// Receive addresses for a known peer, across explicit accounts in this zone.
    #[cfg(feature = "payments")]
    pub fn payment_channel<'a>(
        &'a self,
        peer: &'a PaymentCode,
    ) -> impl Iterator<Item = &'a QiAddressRecord> {
        self.addresses().filter(
            move |r| matches!(&r.origin, QiAddressOrigin::PaymentReceive {peer:p,..} if p.as_ref()==peer),
        )
    }
    /// Cached unused external/change view; Unknown and attempted addresses are excluded.
    pub fn gap_branch(&self, change: bool) -> impl Iterator<Item = &QiAddressRecord> {
        self.branch(change)
            .filter(|r| r.status == QiAddressStatus::Unused)
    }
    /// Cached unused channel receive view. No historical or allocation guarantee.
    #[cfg(feature = "payments")]
    pub fn gap_payment_channel<'a>(
        &'a self,
        peer: &'a PaymentCode,
    ) -> impl Iterator<Item = &'a QiAddressRecord> {
        self.payment_channel(peer)
            .filter(|r| r.status == QiAddressStatus::Unused)
    }
    /// Atomically update a bounded batch at one caller-verified checkpoint. Scope,
    /// duplicates, unknown addresses and backward/conflicting checkpoints reject
    /// before any mutation. Positive use is retained until explicit invalidation.
    pub fn record_observations(
        &mut self,
        scope: NetworkScope,
        checkpoint: Checkpoint,
        observations: &[QiUsageObservation],
    ) -> Result<(), StorageError> {
        if scope != self.scope
            || checkpoint.hash == Hash32::ZERO
            || observations.len() > MAX_QI_ADDRESS_RECORDS
        {
            return Err(StorageError::Invalid);
        }
        let mut seen = BTreeSet::new();
        for observation in observations {
            let old = self
                .address(observation.address)
                .ok_or(StorageError::Invalid)?;
            if !seen.insert(observation.address.address())
                || old.checkpoint.is_some_and(|c| {
                    c.height > checkpoint.height
                        || (c.height == checkpoint.height && c.hash != checkpoint.hash)
                })
            {
                return Err(StorageError::StaleSnapshot);
            }
        }
        for observation in observations {
            let old = self
                .records
                .get_mut(&observation.address.address())
                .ok_or(StorageError::Invalid)?;
            old.status = if observation.used || old.status == QiAddressStatus::Used {
                QiAddressStatus::Used
            } else if old.status == QiAddressStatus::AttemptedUse {
                QiAddressStatus::AttemptedUse
            } else {
                QiAddressStatus::Unused
            };
            old.checkpoint = Some(checkpoint);
        }
        Ok(())
    }
    /// Record an attempted use locally; never downgrade previously observed use.
    pub fn mark_attempted(&mut self, address: QiAddress) -> Result<(), StorageError> {
        let old = self
            .records
            .get_mut(&address.address())
            .ok_or(StorageError::Invalid)?;
        if old.status != QiAddressStatus::Used {
            old.status = QiAddressStatus::AttemptedUse;
        }
        Ok(())
    }
    /// Drop every chain observation after a reorg/restart. Exact public origins
    /// remain. No durable allocation/claim state is accessible or changed here.
    pub fn invalidate_observations(&mut self) {
        for old in self.records.values_mut() {
            old.status = QiAddressStatus::Unknown;
            old.checkpoint = None;
        }
    }
}
