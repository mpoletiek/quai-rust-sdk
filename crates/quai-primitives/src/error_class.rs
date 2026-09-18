/// What a caller, such as a background sync loop, can do about an error.
///
/// Error enums describe what went wrong; this says how to react. Exhaustive on
/// purpose: a new class would change retry decisions, so it should fail to
/// compile rather than fall into a wildcard arm.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorClass {
    /// Temporary network or capacity trouble. Retry with backoff.
    Transient,
    /// Observed state moved: a new head, a reorg or a stale snapshot. Observe
    /// again, then retry.
    Stale,
    /// The endpoint is on a different chain or genesis than configured.
    /// Retrying cannot help; stop and surface it.
    NetworkMismatch,
    /// A submission may have been accepted. Never resubmit automatically;
    /// reconcile by the transaction hash instead.
    Ambiguous,
    /// The request, policy, local state or remote data is invalid. Repeating
    /// the same call fails the same way.
    Invalid,
    /// The caller cancelled.
    Cancelled,
    /// Local persistent storage failed.
    Storage,
}
