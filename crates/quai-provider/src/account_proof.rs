//! Account and storage values proven against a rechecked header's state root.
use crate::state_proof::{
    Account, EMPTY_CODE_HASH, EMPTY_TRIE_ROOT, ProofError, verify_account_proof,
    verify_storage_proof,
};
use crate::{BlockReference, BlockTag, Provider, ProviderError, RpcData, types};
use quai_primitives::{Hash32, QuaiAddress};
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
/// "Proven" is relative to `block`: the node showed that its own state root
/// holds these values. That block is the node's report of the canonical chain,
/// rechecked across the read but not verified independently of the node.
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

impl<T: Transport> Provider<T> {
    /// Prove accounts of one zone, and chosen storage slots of each, against
    /// one block's state root.
    ///
    /// Two rounds, as [`Self::observe_contract_codes`]: the genesis and header
    /// first, with no address, and `GenesisMismatch` before any address is
    /// sent; then every `quai_getProof` by that block's hash, with the header
    /// and genesis rechecked in the same batch where the transport batches.
    ///
    /// Each proof is verified against the header's `evmRoot`. The values the
    /// node reports beside a proof must equal what the proof shows; a proof
    /// that fails either check is `ProviderError::Proof`, never a value.
    ///
    /// Accepts 1 to [`MAX_PROVEN_ACCOUNTS`] accounts, all in one zone, each
    /// with at most [`MAX_PROVEN_SLOTS`] slots.
    pub async fn prove_accounts(
        &self,
        genesis: Hash32,
        targets: &[(QuaiAddress, &[Hash32])],
        block: BlockTag,
    ) -> Result<Vec<ProvenAccount>, ProviderError> {
        if genesis == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest("zero trusted genesis"));
        }
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
        let anchor = self
            .anchor(
                zone,
                block,
                Some(genesis),
                "state proof requires a positive mined height or latest",
            )
            .await?;
        let state_root = state_root(&anchor.header)?;
        let at = anchor.block_param();
        let calls = targets
            .iter()
            .map(|(address, slots)| {
                let keys = slots.iter().map(ToString::to_string).collect::<Vec<_>>();
                ("quai_getProof", json!([address.to_string(), keys, at]))
            })
            .collect();
        let values = self.read_at_anchor(&anchor, calls).await?;
        let block = BlockReference {
            number: anchor.header.number,
            hash: anchor.header.hash,
        };
        targets
            .iter()
            .zip(values)
            .map(|(&(address, slots), value)| {
                let reported = types::parse_account_proof(value?)?;
                let (account, account_proof, storage) =
                    prove(state_root, address, slots, reported)?;
                Ok(ProvenAccount {
                    chain_id: self.expected_chain_id,
                    genesis: anchor.genesis,
                    address,
                    block,
                    state_root,
                    account,
                    account_proof,
                    storage,
                })
            })
            .collect()
    }
}

/// The header's `evmRoot`, which go-quai reports beside the parsed fields.
fn state_root(header: &crate::ZoneHeader) -> Result<Hash32, ProviderError> {
    let root = header
        .extensions
        .fields()
        .get("evmRoot")
        .and_then(|root| root.as_str())
        .ok_or(ProviderError::InvalidResult("header has no evmRoot"))?;
    root.parse()
        .map_err(|_| ProviderError::InvalidResult("header evmRoot is not a 32-byte hash"))
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
