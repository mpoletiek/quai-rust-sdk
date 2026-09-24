//! Account and storage values proven against a rechecked header's state root.
use crate::header_hash::{VerifiedHeader, verify_header_hash};
use crate::state_proof::{
    Account, EMPTY_CODE_HASH, EMPTY_TRIE_ROOT, ProofError, verify_account_proof,
    verify_storage_proof,
};
use crate::{BlockReference, BlockTag, Provider, ProviderError, RpcData, types};
use quai_primitives::{Hash32, QuaiAddress, Zone};
use quai_rpc::{Transport, U256};
use serde_json::json;

/// Most accounts one [`Provider::prove_accounts`] call accepts.
pub const MAX_PROVEN_ACCOUNTS: usize = 32;
/// Most storage slots proven per account in one call.
pub const MAX_PROVEN_SLOTS: usize = 64;

/// One storage slot's value, proven under its account's storage root.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ProvenSlot {
    /// The 32-byte storage key.
    pub slot: Hash32,
    /// The slot's value; zero when the slot, or the whole account, is absent.
    pub value: U256,
    /// The proof nodes, kept so the value can be verified again later.
    pub proof: Vec<RpcData>,
}

/// An account's state, proven against one block's `evmRoot`.
///
/// "Proven" is relative to the header: the node showed that the state root in
/// the header it reported holds these values, and that root hashes into
/// `header_hash`. The header is the node's report of the canonical chain,
/// rechecked across the read. It is independent of the node only once
/// `header_hash` is confirmed by another source; see [`StateAnchor`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ProvenAccount {
    /// Configured chain, checked by the guard that leads each read or batch.
    pub chain_id: U256,
    /// The trusted genesis this was read under.
    pub genesis: Hash32,
    /// The account proven.
    pub address: QuaiAddress,
    /// The block whose state root the proofs verify against.
    pub block: BlockReference,
    /// That block's recomputed header hash, which covers `state_root`. Compare
    /// it with another node's to confirm the root, now or from a stored record.
    pub header_hash: Hash32,
    /// That block's `evmRoot`.
    pub state_root: Hash32,
    /// The account; `None` is a proven absence (no nonce, balance, code or storage).
    pub account: Option<Account>,
    /// The account proof nodes, kept so the account can be verified again later.
    pub account_proof: Vec<RpcData>,
    /// Each requested slot, in request order.
    pub storage: Vec<ProvenSlot>,
}
impl ProvenAccount {
    /// Balance in Its; zero for an absent account.
    pub fn balance(&self) -> U256 {
        self.account.map_or(U256::ZERO, |a| a.balance)
    }
    /// Nonce; zero for an absent account.
    pub fn nonce(&self) -> u64 {
        self.account.map_or(0, |a| a.nonce)
    }
    /// Runtime code hash; [`EMPTY_CODE_HASH`] when the account has no code.
    pub fn code_hash(&self) -> Hash32 {
        self.account.map_or(EMPTY_CODE_HASH, |a| a.code_hash)
    }
    /// Whether the account has runtime code.
    pub fn has_code(&self) -> bool {
        self.code_hash() != EMPTY_CODE_HASH
    }
    /// The proven value of a requested slot, if it was requested.
    pub fn storage_value(&self, slot: Hash32) -> Option<U256> {
        self.storage
            .iter()
            .find(|s| s.slot == slot)
            .map(|s| s.value)
    }
}

/// A block to prove state at, read once and shared by any number of proofs.
///
/// The genesis was checked against the caller's before the header was used,
/// and the header's hashes were recomputed from its fields, so `evmRoot` and
/// every other root are bound to `header.header_hash`. The header is still
/// the serving node's report of the canonical chain: confirm
/// `header.header_hash` with a second node, through
/// [`Provider::confirm_anchor`], before relying on what is proven here.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StateAnchor {
    /// Configured chain of the provider that read the anchor.
    pub chain_id: U256,
    /// The trusted genesis, checked before the header was used.
    pub genesis: Hash32,
    /// The zone whose state this anchor proves.
    pub zone: Zone,
    /// The block's height and reported hash.
    pub block: BlockReference,
    /// The header's recomputed hashes and roots.
    pub header: VerifiedHeader,
}
impl StateAnchor {
    /// The state root proofs at this anchor verify against: `evmRoot`.
    pub fn state_root(&self) -> Hash32 {
        self.header.evm_root
    }
    /// `StaleAnchor` when the block is below `number`, for example a second
    /// node's head less a few blocks of allowance. A lagging node, or one
    /// replaying an old canonical block, fails here.
    ///
    /// The height is part of the seal, which `header_hash` does not cover.
    /// Against a dishonest node it holds once [`Provider::confirm_anchor`]
    /// has matched `header_hash` at this height on another node.
    pub fn require_number_at_least(&self, number: u64) -> Result<(), ProviderError> {
        if self.block.number < number {
            return Err(ProviderError::StaleAnchor);
        }
        Ok(())
    }
    /// `StaleAnchor` when the block's time is before `unix_seconds`.
    ///
    /// The time is part of the seal, not of `header_hash`: it binds only when
    /// `header.hash_verified`, and otherwise catches an honest node that lags,
    /// not a dishonest one. Prefer [`Self::require_number_at_least`].
    pub fn require_timestamp_at_least(&self, unix_seconds: u64) -> Result<(), ProviderError> {
        if self.header.timestamp < unix_seconds {
            return Err(ProviderError::StaleAnchor);
        }
        Ok(())
    }
}

/// Weight of one account in a proof batch, in slot-proof units. An account
/// proof runs about twice the size of a storage proof.
const ACCOUNT_WEIGHT: usize = 2;
/// Most weight in one batch response. A deep mainnet storage proof is about
/// 2.3 KB of JSON, so a full page stays near half a megabyte, well inside
/// the default 2 MiB response cap.
const PAGE_WEIGHT: usize = 192;

impl<T: Transport> Provider<T> {
    /// Prove accounts of one zone, and chosen storage slots of each, against
    /// one block's state root.
    ///
    /// [`Self::state_anchor`] then [`Self::prove_accounts_at`]: the genesis
    /// and header first, with no address, and `GenesisMismatch` before any
    /// address is sent; then every `quai_getProof` by that block's hash, with
    /// the header and genesis rechecked in the same batch where the transport
    /// batches.
    ///
    /// Each proof is verified against the header's `evmRoot`, and `evmRoot`
    /// against the header's recomputed `headerHash`. The values the node
    /// reports beside a proof must equal what the proof shows; a proof that
    /// fails either check is `ProviderError::Proof`, never a value.
    ///
    /// The result is consistent with the header this node reported. It is
    /// independent of the node only once that header is confirmed elsewhere:
    /// see [`StateAnchor`].
    ///
    /// Accepts 1 to [`MAX_PROVEN_ACCOUNTS`] accounts, all in one zone, each
    /// with at most [`MAX_PROVEN_SLOTS`] slots.
    pub async fn prove_accounts(
        &self,
        genesis: Hash32,
        targets: &[(QuaiAddress, &[Hash32])],
        block: BlockTag,
    ) -> Result<Vec<ProvenAccount>, ProviderError> {
        let zone = validate(targets)?;
        let anchor = self.state_anchor(genesis, zone, block).await?;
        self.prove_accounts_at(&anchor, targets).await
    }

    /// Read a block to prove state at: the genesis and the header in one
    /// round trip, naming no address. A genesis other than `genesis` is
    /// `GenesisMismatch`; a header whose hashes do not follow from its fields
    /// is `ProviderError::Header`.
    ///
    /// Share the anchor across [`Self::prove_accounts_at`] calls, each one
    /// round trip, and confirm it with a second node through
    /// [`Self::confirm_anchor`] before any address is sent.
    pub async fn state_anchor(
        &self,
        genesis: Hash32,
        zone: Zone,
        block: BlockTag,
    ) -> Result<StateAnchor, ProviderError> {
        if genesis == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest("zero trusted genesis"));
        }
        let anchor = self
            .anchor(
                zone,
                block,
                Some(genesis),
                "state proof requires a positive mined height or latest",
                true,
            )
            .await?;
        let header = anchor
            .verified
            .clone()
            .ok_or(ProviderError::ObservationChanged)?;
        if header.hash != anchor.header.hash {
            return Err(ProviderError::InvalidResult("header parsed two ways"));
        }
        Ok(StateAnchor {
            chain_id: self.expected_chain_id,
            genesis: anchor.genesis,
            zone,
            block: anchor.reference(),
            header,
        })
    }

    /// Prove accounts at an anchor from [`Self::state_anchor`], in one round
    /// trip where the transport batches: every `quai_getProof` by the
    /// anchor's hash, with its header and genesis rechecked in the same batch.
    /// A reorganized anchor is `ObservationChanged`.
    ///
    /// Large requests are split into batches of about half a megabyte of
    /// response, sent concurrently, each with its own recheck.
    pub async fn prove_accounts_at(
        &self,
        anchor: &StateAnchor,
        targets: &[(QuaiAddress, &[Hash32])],
    ) -> Result<Vec<ProvenAccount>, ProviderError> {
        if validate(targets)? != anchor.zone {
            return Err(ProviderError::InvalidRequest(
                "accounts to prove are outside the anchor's zone",
            ));
        }
        if anchor.chain_id != self.expected_chain_id {
            return Err(ProviderError::InvalidRequest(
                "anchor was read on another chain",
            ));
        }
        let at = crate::anchored::block_param(anchor.block);
        let pages = pages(targets).into_iter().map(|page| {
            let calls = page
                .iter()
                .map(|(address, slots)| {
                    let keys = slots.iter().map(ToString::to_string).collect::<Vec<_>>();
                    ("quai_getProof", json!([address.to_string(), keys, at]))
                })
                .collect();
            self.read_at_anchor(anchor.zone, anchor.genesis, anchor.block, calls)
        });
        let values = futures_util::future::try_join_all(pages).await?;
        targets
            .iter()
            .zip(values.into_iter().flatten())
            .map(|(&(address, slots), value)| {
                let reported = types::parse_account_proof(value?)?;
                let (account, account_proof, storage) =
                    prove(anchor.state_root(), address, slots, reported)?;
                Ok(ProvenAccount {
                    chain_id: anchor.chain_id,
                    genesis: anchor.genesis,
                    address,
                    block: anchor.block,
                    header_hash: anchor.header.header_hash,
                    state_root: anchor.state_root(),
                    account,
                    account_proof,
                    storage,
                })
            })
            .collect()
    }

    /// Read a zone header and recompute its hashes, or `None` when the node
    /// has no block there. Compare its `header_hash` with a stored
    /// [`ProvenAccount::header_hash`] to confirm an old record against this
    /// node. It does not check the genesis; [`Self::confirm_anchor`] does.
    pub async fn verified_header(
        &self,
        zone: Zone,
        block: BlockTag,
    ) -> Result<Option<VerifiedHeader>, ProviderError> {
        let value = self
            .header_values(zone, &[block])
            .await
            .into_iter()
            .next()
            .unwrap_or(Err(ProviderError::InvalidResult("batch response count")))?;
        if value.is_null() {
            return Ok(None);
        }
        let header = verify_header_hash(&value)?;
        let wrong_height = match block {
            BlockTag::Number(number) => U256::from(header.number) != number,
            _ => false,
        };
        if header.zone != Some(zone) || wrong_height {
            return Err(ProviderError::InvalidResult("header is for another block"));
        }
        Ok(Some(header))
    }

    /// Confirm an anchor read from another node: this node's header at the
    /// anchor's height must recompute to the same `headerHash`, which binds
    /// every root in the header, `evmRoot` included. Names no address.
    ///
    /// Returns this node's verified header. `GenesisMismatch` when this node
    /// is on another network; `ObservationChanged` when it has no block at
    /// that height yet; `AnchorDisputed` when its block there differs.
    pub async fn confirm_anchor(
        &self,
        anchor: &StateAnchor,
    ) -> Result<VerifiedHeader, ProviderError> {
        if anchor.chain_id != self.expected_chain_id {
            return Err(ProviderError::InvalidRequest(
                "anchor was read on another chain",
            ));
        }
        let height = BlockTag::Number(U256::from(anchor.block.number));
        let mut values = self
            .header_values(anchor.zone, &[BlockTag::Number(U256::ZERO), height])
            .await
            .into_iter();
        let mut next = || {
            values
                .next()
                .unwrap_or(Err(ProviderError::InvalidResult("batch response count")))
        };
        if types::genesis_hash(next()?)? != anchor.genesis {
            return Err(ProviderError::GenesisMismatch);
        }
        let value = next()?;
        if value.is_null() {
            return Err(ProviderError::ObservationChanged);
        }
        let header = verify_header_hash(&value)?;
        if header.zone != Some(anchor.zone) || header.number != anchor.block.number {
            return Err(ProviderError::InvalidResult("header is for another block"));
        }
        if header.header_hash != anchor.header.header_hash {
            return Err(ProviderError::AnchorDisputed);
        }
        Ok(header)
    }
}

/// The one zone every target is in, or why the request is invalid.
fn validate(targets: &[(QuaiAddress, &[Hash32])]) -> Result<Zone, ProviderError> {
    let Some((first, _)) = targets.first() else {
        return Err(ProviderError::InvalidRequest("no accounts to prove"));
    };
    if targets.len() > MAX_PROVEN_ACCOUNTS
        || targets
            .iter()
            .any(|(_, slots)| slots.len() > MAX_PROVEN_SLOTS)
    {
        return Err(ProviderError::InvalidRequest(
            "too many accounts or slots to prove",
        ));
    }
    let zone = first.zone();
    if targets.iter().any(|(address, _)| address.zone() != zone) {
        return Err(ProviderError::InvalidRequest(
            "accounts to prove span zones",
        ));
    }
    Ok(zone)
}

/// Split targets, in order, into batches of at most [`PAGE_WEIGHT`], never
/// splitting an account from its slots.
fn pages<'t, 's>(
    targets: &'t [(QuaiAddress, &'s [Hash32])],
) -> Vec<&'t [(QuaiAddress, &'s [Hash32])]> {
    let mut pages = Vec::new();
    let (mut start, mut weight) = (0, 0);
    for (index, (_, slots)) in targets.iter().enumerate() {
        let next = ACCOUNT_WEIGHT + slots.len();
        if index > start && weight + next > PAGE_WEIGHT {
            pages.push(&targets[start..index]);
            (start, weight) = (index, 0);
        }
        weight += next;
    }
    pages.push(&targets[start..]);
    pages
}

/// What one verified `quai_getProof` result establishes.
type Proven = (Option<Account>, Vec<RpcData>, Vec<ProvenSlot>);

/// Verify one reported proof and check the node's own fields against it.
fn prove(
    state_root: Hash32,
    address: QuaiAddress,
    slots: &[Hash32],
    reported: types::ReportedProof,
) -> Result<Proven, ProviderError> {
    if reported.address != address.address() || reported.storage.len() != slots.len() {
        return Err(ProviderError::InvalidResult("proof is for another request"));
    }
    let account = verify_account_proof(state_root, address, &reported.account_proof)?;
    let expected = account.map_or((U256::ZERO, 0, EMPTY_CODE_HASH, EMPTY_TRIE_ROOT), |a| {
        (a.balance, a.nonce, a.code_hash, a.storage_root)
    });
    if (
        reported.balance,
        reported.nonce,
        reported.code_hash,
        reported.storage_hash,
    ) != expected
    {
        return Err(ProofError::Contradicted.into());
    }
    let storage_root = expected.3;
    let storage = slots
        .iter()
        .zip(reported.storage)
        .map(|(&slot, (key, value, proof))| {
            if key != slot {
                return Err(ProviderError::InvalidResult("proof is for another slot"));
            }
            let proven = verify_storage_proof(storage_root, slot, &proof)?;
            if proven != value {
                return Err(ProofError::Contradicted.into());
            }
            Ok(ProvenSlot {
                slot,
                value: proven,
                proof,
            })
        })
        .collect::<Result<_, ProviderError>>()?;
    Ok((account, reported.account_proof, storage))
}
