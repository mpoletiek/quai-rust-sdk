//! Scoped atomic IndexedDB snapshots for public metadata or application-encrypted backups.
use crate::BrowserError;
use quai_primitives::{Hash32, Zone};
use quai_rpc::U256;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
#[wasm_bindgen(module = "/src/storage.js")]
extern "C" {
    #[wasm_bindgen(catch,js_name=openSnapshotStore)]
    async fn open_store(name: &str, scope: &str, max_bytes: usize) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name=closeSnapshotStore)]
    fn close_store(handle: &JsValue);
    #[wasm_bindgen(catch,js_name=readSnapshot)]
    async fn read_snapshot(handle: &JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch,js_name=compareExchangeSnapshot)]
    async fn compare_exchange(
        handle: &JsValue,
        expected: f64,
        bytes: &JsValue,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch,js_name=compareExchangeSnapshots)]
    async fn compare_exchanges(
        handles: &js_sys::Array,
        expected: &js_sys::Array,
        values: &js_sys::Array,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name=snapshotRevision)]
    fn revision(record: &JsValue) -> f64;
    #[wasm_bindgen(js_name=snapshotBytes)]
    fn bytes(record: &JsValue) -> JsValue;
}
struct Handle(JsValue);
impl Drop for Handle {
    fn drop(&mut self) {
        close_store(&self.0);
    }
}
/// Exact network and wallet namespace. Bind it as associated data when encrypting
/// the snapshot: database names alone do not authenticate ciphertext ownership.
#[derive(Clone, Copy, Debug)]
pub struct BrowserStorageScope {
    /// Replay-protection chain identity.
    pub chain_id: U256,
    /// Trusted genesis, not just a network label.
    pub genesis: Hash32,
    /// Wallet zone.
    pub zone: Zone,
    /// Application-selected nonsecret wallet identity.
    pub wallet: Hash32,
}
/// Monotonic snapshot revision and opaque bytes. A tombstone retains its revision,
/// preventing stale tabs from recreating deleted state with an old revision.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BrowserSnapshot {
    /// Revision up to JavaScript's maximum safe integer.
    pub revision: u64,
    /// Opaque public/encrypted state, or a deletion tombstone.
    pub bytes: Option<Vec<u8>>,
}
/// IndexedDB snapshots with atomic compare-and-exchange across tabs and workers.
/// The caller owns snapshot serialization and encryption. No plaintext secret
/// origin belongs in this store. A cancelled write may already have committed;
/// read the current revision before deciding whether to retry.
#[derive(Clone)]
pub struct BrowserSnapshotStore {
    handle: Rc<Handle>,
    max_bytes: usize,
}
/// One entry in an atomic multi-journal commit. Bytes must be public or encrypted.
pub struct BrowserSnapshotUpdate<'a> {
    /// Target namespace; all entries must share one named database.
    pub store: &'a BrowserSnapshotStore,
    /// Exact current revision, or None for a never-written namespace.
    pub expected: Option<u64>,
    /// New bytes, or None to retain a deletion tombstone.
    pub bytes: Option<&'a [u8]>,
}
/// Commit 1..=128 distinct namespaces in one IndexedDB transaction, with a
/// combined 16 MiB payload limit. Any conflict/error aborts every update. Returns
/// revisions in input order. Cross-database updates and duplicate keys reject.
/// Cancellation after dispatch may commit all entries: re-read before retrying.
pub async fn compare_exchange_snapshots(
    updates: &[BrowserSnapshotUpdate<'_>],
) -> Result<Vec<u64>, BrowserError> {
    if updates.is_empty() || updates.len() > 128 {
        return Err(BrowserError::InvalidConfig);
    }
    let mut total = 0usize;
    for update in updates {
        let size = update.bytes.map_or(0, <[u8]>::len);
        total = total.checked_add(size).ok_or(BrowserError::InvalidConfig)?;
        if total > 16 * 1024 * 1024
            || size > update.store.max_bytes
            || update
                .expected
                .is_some_and(|r| r == 0 || r > 9_007_199_254_740_991)
        {
            return Err(BrowserError::InvalidConfig);
        }
    }
    let handles = js_sys::Array::new();
    let expected = js_sys::Array::new();
    let values = js_sys::Array::new();
    for update in updates {
        handles.push(&update.store.handle.0);
        expected.push(&JsValue::from_f64(
            update.expected.map_or(-1.0, |r| r as f64),
        ));
        values.push(
            &update
                .bytes
                .map_or(JsValue::NULL, |b| js_sys::Uint8Array::from(b).into()),
        );
    }
    let result = compare_exchanges(&handles, &expected, &values)
        .await
        .map_err(storage_error)?;
    js_sys::Array::from(&result)
        .iter()
        .map(|r| r.as_f64().map(|r| r as u64).ok_or(BrowserError::Storage))
        .collect()
}
impl BrowserSnapshotStore {
    /// Open a named store without account prompts. No localStorage fallback exists.
    /// `max_bytes` is 1..=16 MiB; names contain 1..=128 ASCII letters/digits/_/-.
    /// A database permits 2048 namespaces, including deletion tombstones.
    pub async fn open(
        name: &str,
        scope: BrowserStorageScope,
        max_bytes: usize,
    ) -> Result<Self, BrowserError> {
        if scope.genesis == Hash32::ZERO
            || scope.wallet == Hash32::ZERO
            || scope.chain_id == U256::ZERO
            || !(1..=16 * 1024 * 1024).contains(&max_bytes)
        {
            return Err(BrowserError::InvalidConfig);
        }
        let scope = format!(
            "{:x}:{}:{:02x}:{}",
            scope.chain_id,
            scope.genesis,
            scope.zone.byte(),
            scope.wallet
        );
        let handle = open_store(name, &scope, max_bytes)
            .await
            .map_err(storage_error)?;
        Ok(Self {
            handle: Rc::new(Handle(handle)),
            max_bytes,
        })
    }
    /// Read after the IndexedDB transaction completes. None means never written.
    pub async fn read(&self) -> Result<Option<BrowserSnapshot>, BrowserError> {
        let record = read_snapshot(&self.handle.0).await.map_err(storage_error)?;
        if record.is_null() {
            return Ok(None);
        }
        let revision = revision(&record) as u64;
        let data = bytes(&record);
        let bytes = if data.is_null() {
            None
        } else {
            Some(js_sys::Uint8Array::new(&data).to_vec())
        };
        Ok(Some(BrowserSnapshot { revision, bytes }))
    }
    /// Commit only if the stored revision exactly matches (None means never
    /// written). None bytes writes a tombstone. Concurrent losers receive
    /// `StorageConflict`; cursor/reservation merging is an application operation.
    pub async fn compare_exchange(
        &self,
        expected: Option<u64>,
        snapshot: Option<&[u8]>,
    ) -> Result<u64, BrowserError> {
        if expected.is_some_and(|r| r == 0 || r > 9_007_199_254_740_991)
            || snapshot.is_some_and(|b| b.len() > self.max_bytes)
        {
            return Err(BrowserError::InvalidConfig);
        }
        let data = snapshot.map_or(JsValue::NULL, |b| js_sys::Uint8Array::from(b).into());
        let revision = compare_exchange(&self.handle.0, expected.map_or(-1.0, |r| r as f64), &data)
            .await
            .map_err(storage_error)?;
        revision
            .as_f64()
            .map(|r| r as u64)
            .ok_or(BrowserError::Storage)
    }
}
fn storage_error(error: JsValue) -> BrowserError {
    let kind = js_sys::Reflect::get(&error, &JsValue::from_str("quaiBrowserError"))
        .ok()
        .and_then(|s| s.as_string());
    match kind.as_deref() {
        Some("storage_conflict") => BrowserError::StorageConflict,
        Some("invalid") => BrowserError::InvalidConfig,
        _ => BrowserError::Storage,
    }
}
