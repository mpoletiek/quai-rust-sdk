//! Explicit pre-signing discovery of account access declarations.
use super::*;

pub use crate::account_preflight::AccountAccessListPolicy;
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
        crate::account_preflight::populate_access(
            self.provider,
            self.access_list_policy,
            request,
            transaction,
            block,
        )
        .await
        .map_err(|error| match error {
            crate::account_preflight::AccountPreflightError::Provider(e) => {
                AccountError::Provider(e)
            }
            _ => AccountError::InvalidOperation,
        })
    }
}
