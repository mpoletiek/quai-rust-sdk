//! Signed-intent-bound deployment/code observations, without finality claims.
use crate::{
    BlockReference, BlockTag, Provider, ProviderError, ReceiptOutcome, RpcData, TransactionKind,
};
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::{Hash32, QuaiAddress, Zone, contract_address};
use quai_rpc::{Transport, U256};
/// Exact locally signed creation intent and optional expected runtime-code hash.
#[derive(Clone, Debug)]
pub struct DeploymentReference {
    chain: U256,
    genesis: Hash32,
    sender: QuaiAddress,
    hash: Hash32,
    address: QuaiAddress,
    expected_runtime: Option<Hash32>,
}
impl DeploymentReference {
    /// Bind only a direct CREATE transaction with a valid same-zone Quai result.
    /// Runtime code is distinct from signed init code and must be supplied explicitly
    /// when the application wants an expected-code comparison.
    pub fn from_signed(
        genesis: Hash32,
        signed: &SignedQuaiTransaction,
        expected_runtime: Option<Hash32>,
    ) -> Result<Self, ProviderError> {
        let tx = signed.transaction();
        if genesis == Hash32::ZERO || tx.to.is_some() {
            return Err(ProviderError::InvalidRequest(
                "not a scoped direct deployment",
            ));
        }
        let nonce = tx.nonce;
        let address =
            QuaiAddress::try_from(contract_address(signed.from().address(), nonce, &tx.data))
                .map_err(|_| ProviderError::InvalidRequest("deployment address is not Quai"))?;
        if address.zone() != signed.from().zone() {
            return Err(ProviderError::InvalidRequest(
                "deployment address is outside sender zone",
            ));
        }
        Ok(Self {
            chain: tx.chain_id,
            genesis,
            sender: signed.from(),
            hash: signed
                .hash()
                .map_err(|_| ProviderError::InvalidRequest("invalid signed deployment"))?,
            address,
            expected_runtime,
        })
    }
    /// Exact predicted created account for the signed nonce and complete init code.
    pub fn address(&self) -> QuaiAddress {
        self.address
    }
    /// Original signed creation hash.
    pub fn transaction_hash(&self) -> Hash32 {
        self.hash
    }
    /// Explicit origin zone.
    pub fn zone(&self) -> Zone {
        self.sender.zone()
    }
}
/// Code in the source's state at the end of the canonical inclusion block.
/// Empty code is preserved: successful creation does not guarantee deployed code.
#[derive(Clone, Debug, PartialEq)]
pub struct DeploymentCode {
    /// Bounded runtime bytes, not init code. Debug reveals length only.
    pub bytes: RpcData,
    /// Keccak-256 of the exact runtime bytes, including the empty-code case.
    pub hash: Hash32,
    /// Comparison with the caller's expected runtime hash, when supplied.
    pub matches_expected: Option<bool>,
}
/// Current source-reported direct deployment state.
#[derive(Clone, Debug, PartialEq)]
pub enum DeploymentObservation {
    /// Receipt unavailable; a known transaction may be pending or indexed without
    /// its receipt. Absence never establishes rejection or permission to retry payment.
    NoReceipt {
        /// Whether the source supplied the exact transaction hash.
        transaction_known: bool,
    },
    /// Reported receipt inclusion does not match the current canonical-number view.
    Noncanonical {
        /// Receipt's inclusion claim.
        reported: BlockReference,
        /// Current canonical identity, when available.
        canonical: Option<BlockReference>,
    },
    /// Canonical origin inclusion with its exact outcome and sampled depth.
    Included {
        /// Inclusion rechecked around code lookup.
        block: BlockReference,
        /// Source execution result. Only Succeeded causes runtime lookup.
        outcome: ReceiptOutcome,
        /// Sampled canonical depth including the inclusion block, not finality.
        confirmations: u64,
        /// End-of-inclusion-block code for successful creation; failures/unknown
        /// outcomes do not infer deployment from pre-existing or later code.
        code: Option<DeploymentCode>,
    },
}
impl<T: Transport> Provider<T> {
    /// Observe the exact signed creation, validate receipt sender/type/destination
    /// and predicted contract, then read code at the numeric inclusion height.
    /// Pruned state and source errors propagate. Reorgs during the multi-request
    /// observation fail closed, and hash comparison does not attest contract safety.
    pub async fn observe_deployment(
        &self,
        reference: &DeploymentReference,
    ) -> Result<DeploymentObservation, ProviderError> {
        let zone = reference.zone();
        if self.expected_chain_id != reference.chain {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual: reference.chain,
            });
        }
        if self.genesis_hash(zone).await? != reference.genesis {
            return Err(ProviderError::InvalidResult("deployment genesis mismatch"));
        }
        let Some(receipt) = self.receipt(zone, reference.hash).await? else {
            return Ok(DeploymentObservation::NoReceipt {
                transaction_known: self.transaction(zone, reference.hash).await?.is_some(),
            });
        };
        if receipt.kind != TransactionKind::Quai
            || receipt
                .from
                .is_some_and(|a| a != reference.sender.address())
            || receipt.to.is_some()
            || receipt
                .contract_address
                .is_some_and(|a| a != reference.address)
            || (receipt.outcome == ReceiptOutcome::Succeeded
                && receipt.contract_address != Some(reference.address))
        {
            return Err(ProviderError::InvalidResult(
                "deployment receipt differs from signed intent",
            ));
        }
        let block = BlockReference {
            number: receipt.inclusion.block_number,
            hash: receipt.inclusion.block_hash,
        };
        let canonical = self
            .header_at(zone, block.number)
            .await?
            .map(|h| BlockReference {
                number: h.number,
                hash: h.hash,
            });
        if canonical != Some(block) {
            return Ok(DeploymentObservation::Noncanonical {
                reported: block,
                canonical,
            });
        }
        let head = self
            .latest_header(zone)
            .await?
            .ok_or(ProviderError::ObservationChanged)?;
        let confirmations = head
            .number
            .checked_sub(block.number)
            .and_then(|n| n.checked_add(1))
            .ok_or(ProviderError::ObservationChanged)?;
        let code = if receipt.outcome == ReceiptOutcome::Succeeded {
            let bytes = self
                .code(
                    reference.address,
                    BlockTag::Number(U256::from(block.number)),
                )
                .await?;
            let hash = Hash32::from_bytes(quai_crypto::keccak256(bytes.bytes()));
            Some(DeploymentCode {
                bytes,
                hash,
                matches_expected: reference.expected_runtime.map(|expected| expected == hash),
            })
        } else {
            None
        };
        for (number, hash) in [(block.number, block.hash), (head.number, head.hash)] {
            if self
                .header_at(zone, number)
                .await?
                .is_none_or(|h| h.hash != hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
        }
        Ok(DeploymentObservation::Included {
            block,
            outcome: receipt.outcome,
            confirmations,
            code,
        })
    }
}

/// Native bounded deployment-wait failures never cancel an already signed operation.
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeploymentWaitError {
    /// Invalid timeout, polling interval or confirmation depth; no I/O occurred.
    #[error("invalid deployment wait limits")]
    InvalidConfig,
    /// The elapsed-time budget expired, without inferring rejection or finality.
    #[error("timed out waiting for deployment {transaction_hash}")]
    Timeout {
        /// Exact signed creation being observed.
        transaction_hash: Hash32,
        /// Most recently seen inclusion, which may have subsequently reorganized.
        last_observed: Option<BlockReference>,
    },
    /// Source failure; the application decides whether to start another wait.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
impl<T: Transport> Provider<T> {
    /// Wait for canonical deployment inclusion at an explicit depth, refreshing
    /// receipt and inclusion-block code through `observe_deployment`. Failed,
    /// locked and unknown outcomes return explicitly once they reach that depth;
    /// empty or mismatched runtime code is never represented as successful code.
    /// Missing/reorganized observations continue until the overall deadline.
    /// Other source errors stop immediately. Dropping the future stops polling.
    pub async fn wait_for_deployment(
        &self,
        reference: &DeploymentReference,
        config: crate::WaitConfig,
    ) -> Result<DeploymentObservation, DeploymentWaitError> {
        if config.confirmations == 0
            || config.timeout.is_zero()
            || config.poll_interval.is_zero()
            || config.poll_interval > config.timeout
            || tokio::time::Instant::now()
                .checked_add(config.timeout)
                .is_none()
        {
            return Err(DeploymentWaitError::InvalidConfig);
        }
        let mut last_observed = None;
        let work = async {
            loop {
                match self.observe_deployment(reference).await {
                    Ok(observation) => match &observation {
                        DeploymentObservation::Included {
                            block,
                            confirmations,
                            ..
                        } => {
                            last_observed = Some(*block);
                            if *confirmations >= config.confirmations {
                                return Ok(observation);
                            }
                        }
                        DeploymentObservation::Noncanonical { reported, .. } => {
                            last_observed = Some(*reported)
                        }
                        DeploymentObservation::NoReceipt { .. } => {}
                    },
                    Err(ProviderError::ObservationChanged) => {}
                    Err(error) => return Err(DeploymentWaitError::Provider(error)),
                }
                tokio::time::sleep(config.poll_interval).await;
            }
        };
        match tokio::time::timeout(config.timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(DeploymentWaitError::Timeout {
                transaction_hash: reference.hash,
                last_observed,
            }),
        }
    }
}
