//! Explicit pre-signing discovery of account access declarations.
use super::*;

/// How ordinary account calls and deployments obtain their access declaration.
/// Replacements retain the original signed declaration; conversions are separate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccountAccessListPolicy {
    /// Retain the application's exact ordered entries without another RPC.
    #[default]
    Preserve,
    /// Ask the node for a bounded declaration at the same selector and nonce as
    /// fee preparation. Existing address/key requirements must remain covered.
    /// The resulting list is visible in the prepared transaction before signing.
    Discover,
}
impl<T: Transport, S: Signer> AccountSession<'_, T, S> {
    /// Choose explicit access discovery for ordinary calls and deployments.
    /// Node errors propagate; signed payloads are never changed or repopulated.
    pub fn with_access_list_policy(mut self, policy: AccountAccessListPolicy) -> Self {
        self.access_list_policy = policy;
        self
    }
    pub(super) async fn populate_access(
        &self,
        request: &mut CallRequest,
        transaction: &mut QuaiTransaction,
        block: BlockTag,
    ) -> Result<(), AccountError> {
        if self.access_list_policy == AccountAccessListPolicy::Preserve {
            return Ok(());
        }
        let generated = self.provider.create_access_list(request, block).await?;
        // The RPC must not discard mandatory CREATE/lockup entries or explicit
        // storage requirements. Ordering of its final list remains observable.
        let mut coverage = std::collections::BTreeMap::<_, std::collections::BTreeSet<_>>::new();
        for entry in &generated.access_list {
            coverage
                .entry(entry.address)
                .or_default()
                .extend(entry.storage_keys.iter().copied());
        }
        for required in &request.access_list {
            let Some(keys) = coverage.get(&required.address) else {
                return Err(AccountError::InvalidOperation);
            };
            if required.storage_keys.iter().any(|key| !keys.contains(key)) {
                return Err(AccountError::InvalidOperation);
            }
        }
        transaction.access_list = generated
            .access_list
            .iter()
            .map(|entry| AccessTuple {
                address: entry.address,
                storage_keys: entry.storage_keys.clone(),
            })
            .collect();
        transaction
            .unsigned_bytes()
            .map_err(|_| AccountError::InvalidOperation)?;
        request.access_list = generated.access_list;
        Ok(())
    }
}
