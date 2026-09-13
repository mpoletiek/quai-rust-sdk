//! Canonical same-claim candidate-family validation, independent of persistence.
use super::*;
use quai_consensus::SignedQuaiTransaction;
use std::collections::BTreeMap;
/// One immutable replacement edge. Its canonical signed bytes are stored before
/// exposure. Inclusion is reconciled separately for every candidate hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuaiReplacement {
    /// Original or earlier replacement transaction hash.
    pub parent: Hash32,
    /// Canonical signed bytes; validated on capture, read and restore.
    pub payload: Vec<u8>,
}
/// Both-ledger name for the immutable candidate edge.
pub type ReplacementCandidate = QuaiReplacement;
pub(crate) fn validate_family(root: &[u8], variants: &[QuaiReplacement]) -> Result<()> {
    if let Ok(root) = quai_consensus::SignedQiOperation::decode(root) {
        return validate_qi_family(root, variants);
    }
    if variants.len() > 32 {
        return Err(StorageError::Invalid);
    }
    let root = SignedQuaiTransaction::decode(root).map_err(|_| StorageError::Invalid)?;
    let mut known = BTreeMap::from([(root.hash().map_err(|_| StorageError::Invalid)?, root)]);
    for variant in variants {
        let parent = known.get(&variant.parent).ok_or(StorageError::Invalid)?;
        let signed =
            SignedQuaiTransaction::decode(&variant.payload).map_err(|_| StorageError::Invalid)?;
        let mut expected = parent.transaction().clone();
        if signed.from() != parent.from() || signed.transaction().gas_price <= expected.gas_price {
            return Err(StorageError::Invalid);
        }
        expected.gas_price = signed.transaction().gas_price;
        if signed.transaction() != &expected {
            return Err(StorageError::Invalid);
        }
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if known.insert(hash, signed).is_some() {
            return Err(StorageError::Invalid);
        }
    }
    Ok(())
}

fn output_value(tx: &quai_consensus::QiTransaction) -> Result<U256> {
    tx.outputs.iter().try_fold(U256::ZERO, |sum, output| {
        sum.checked_add(U256::from(output.denomination.value()))
            .ok_or(StorageError::Invalid)
    })
}
fn validate_qi_family(
    root: quai_consensus::SignedQiOperation,
    variants: &[QuaiReplacement],
) -> Result<()> {
    if variants.len() > 32 {
        return Err(StorageError::Invalid);
    }
    let mut known = BTreeMap::from([(root.hash().map_err(|_| StorageError::Invalid)?, root)]);
    for variant in variants {
        let parent = known.get(&variant.parent).ok_or(StorageError::Invalid)?;
        let signed = quai_consensus::SignedQiOperation::decode(&variant.payload)
            .map_err(|_| StorageError::Invalid)?;
        let old = parent.transaction();
        let tx = signed.transaction();
        if tx.chain_id != old.chain_id
            || tx.inputs != old.inputs
            || tx.data != old.data
            || output_value(tx)? >= output_value(old)?
        {
            return Err(StorageError::Invalid);
        }
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if known.insert(hash, signed).is_some() {
            return Err(StorageError::Invalid);
        }
    }
    Ok(())
}
