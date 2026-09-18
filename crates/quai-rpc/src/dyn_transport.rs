//! A type-erased transport, so one `Provider` type can hold any transport.
use crate::{BatchResult, Endpoint, RpcError, Transport};
use serde_json::Value;
use std::{fmt, future::Future, pin::Pin, sync::Arc};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe mirror of [`Transport`]. Private: callers implement `Transport`
/// and wrap it with [`DynTransport::new`].
trait ErasedTransport: Send + Sync {
    fn request<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>>;
    fn request_batch<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        requests: Vec<(&'a str, Value)>,
    ) -> BoxFuture<'a, Option<BatchResult>>;
}

impl<T: Transport + Send + Sync> ErasedTransport for T {
    fn request<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(Transport::request(self, endpoint, method, params))
    }
    fn request_batch<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        requests: Vec<(&'a str, Value)>,
    ) -> BoxFuture<'a, Option<BatchResult>> {
        Box::pin(Transport::request_batch(self, endpoint, requests))
    }
}

/// Any native transport behind one concrete, cheaply cloned type.
///
/// [`Transport`] returns `impl Future`, so it cannot be a trait object, and a
/// `Provider<T>` is generic over its transport. An application that chooses
/// HTTP or WebSocket at runtime, or swaps endpoints while running, holds a
/// `Provider<DynTransport>` instead. Each request costs one boxed future. The
/// wrapped transport's batch support is preserved, so batched reads and their
/// chain guards behave exactly as they do unwrapped.
///
/// Only the [`Transport`] surface is erased: keep the concrete handle for
/// anything else, such as WebSocket subscriptions.
#[derive(Clone)]
pub struct DynTransport(Arc<dyn ErasedTransport>);

impl DynTransport {
    /// Erase a transport's type.
    pub fn new<T: Transport + Send + Sync + 'static>(transport: T) -> Self {
        Self(Arc::new(transport))
    }
}

impl fmt::Debug for DynTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DynTransport")
    }
}

impl Transport for DynTransport {
    async fn request_batch(
        &self,
        endpoint: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        self.0.request_batch(endpoint, requests).await
    }
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        self.0.request(endpoint, method, params).await
    }
}
