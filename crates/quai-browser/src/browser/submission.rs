//! Explicit signed-payload submission capability for application-selected wallets.
use super::*;
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction};

/// Explicit transport capability for canonical, already-signed Quai/Qi payloads.
/// Compose with `quai_provider::Provider` to retain its expected transaction ID,
/// acknowledgement validation and ambiguous-submission errors. All other wallet
/// signing/permission/sending methods remain unavailable through `Transport`.
#[derive(Clone, Debug)]
pub struct InjectedSubmissionTransport(InjectedProvider);
impl InjectedProvider {
    /// Opt into forwarding verified signed bytes through this wallet's RPC.
    /// Construction performs no network requests or permission/signing prompts.
    /// The original `InjectedProvider` transport retains its read-only allowlist.
    pub fn signed_submission_transport(&self) -> InjectedSubmissionTransport {
        InjectedSubmissionTransport(self.clone())
    }
}
impl Transport for InjectedSubmissionTransport {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if method != "quai_sendRawTransaction" {
            return self
                .0
                .read(endpoint, method, params)
                .await
                .map_err(rpc_error);
        }
        if endpoint != &self.0.endpoint {
            return Err(RpcError::InvalidConfig);
        }
        let values = params
            .as_array()
            .filter(|p| p.len() == 1)
            .ok_or(RpcError::InvalidConfig)?;
        let bytes = values[0]
            .as_str()
            .ok_or(RpcError::InvalidConfig)
            .and_then(|text| {
                quai_primitives::get_bytes(text).map_err(|_| RpcError::InvalidConfig)
            })?;
        let (chain, zone) = if let Ok(signed) = SignedQuaiTransaction::decode(&bytes) {
            (signed.transaction().chain_id, signed.from().zone())
        } else {
            let signed = SignedQiOperation::decode(&bytes).map_err(|_| RpcError::InvalidConfig)?;
            (
                signed.transaction().chain_id,
                signed.origin_zone().map_err(|_| RpcError::InvalidConfig)?,
            )
        };
        if chain != self.0.expected_chain || Shard::Zone(zone) != self.0.shard {
            return Err(RpcError::InvalidConfig);
        }
        // Validate the complete envelope before touching the selected wallet.
        encode(
            &json!({"method":method,"params":params,"shard":self.0.shard.encoded()}),
            self.0.config.max_request_bytes,
        )?;
        let revision = self.0.context_revision();
        self.0.check_chain().await.map_err(rpc_error)?;
        if provider_changed(&self.0.watch.0, revision) {
            return Err(rpc_error(BrowserError::ContextChanged));
        }
        // Once raw submission starts, any error/cancellation is ambiguous. The
        // high-level Provider retains that distinction and never retries writes.
        let result = self.0.raw(method, params).await.map_err(rpc_error)?;
        self.0
            .verify_context(revision, None)
            .await
            .map_err(rpc_error)?;
        Ok(result)
    }
}
