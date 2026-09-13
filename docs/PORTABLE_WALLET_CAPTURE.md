# Portable recovery capture across wallet journals

With `backup`, `WalletBackup::capture_portable` combines explicitly frozen public
journals and supplied private origins into one ownership-proved recovery backup.
The SDK exposes it through `wallet::full_backup`. Capture requires neither SQLite,
Tokio nor a network provider; the same API runs in a dedicated browser worker.

`PortableWalletCapture` takes:

- `allocations`: HD account journals, including receive/change floors and all
  burned pending/abandoned ranges; completed public addresses join the inventory.
- `accounts`: account custody journals paired with their exact public origin
  metadata, retaining nonce floors, operation IDs, claims and signed candidates.
- `qi`: Qi custody journals, retaining public origins, exact inputs, operation IDs
  and transfer/conversion/wrapping candidate bytes.
- `payments`: owner/peer/direction journals, combining zone cursors into owned
  payment channels and preserving completed send/receive exposures.
- `additional_addresses`: owned address inventory retained outside those journals,
  including addresses recovered from an older backup before starting new allocators.

Supply every relevant journal and known owned address. Missing inputs cannot be
inferred from an empty UTXO response, address balance or another journal. Capture
checks duplicate namespaces, conflicting public origins and operation IDs, and
private derivation ownership. It does not mutate any source journal. Payment
receive exposures prove their corresponding imported Qi keys using a retained
seed/master/payment-account origin; send destinations do not become owned addresses.

Call the returned backup's `encrypt` method with an explicit password and bounded
`BackupKdf`. Existing authenticated QUAIWALT v1–v5 encoding is unchanged; the
required version follows the included origins/channels/candidates. Decrypt with
`EncryptedWalletBackup`, then restore a native store or initialize the browser
address, account, Qi and payment books from the same authenticated backup.
Initialization requires unused namespaces. Existing account/Qi books use their
explicit monotonic merge methods; never overwrite a live journal with an old copy.

Inclusion and reservation checkpoints are omitted. Current UTXOs, balances,
canonical ancestry, external wallet connection state and network observations
require fresh queries. Account/Qi operation IDs and exact signed candidates remain
held after restore. The recovery format retains burned allocation cursors and
completed address/exposure inventory, but not allocator request IDs or pending
search work. Start new allocation IDs above the restored floors; retain the
public journal separately when exact pending-request resumption is required.

Capture accepts at most 128 journal descriptors totaling 16 MiB of encoded public
journal state, plus up to 100,000 supplied inventory entries. Full-backup limits
also apply: 64 network/zone scopes, 16 secret origins, 100,000 total records and
16 MiB encoded plaintext. Duplicate journals or conflicting operation IDs across
account/Qi journals fail atomically. Generate unique transaction operation IDs
across each scope. A caller-selected Qi namespace is supplied again on restore;
it is not a private ownership proof or an implicit allocation identity.

These are detached snapshots. Applications must establish a consistent read across
all live browser stores before calling this API; the pure capture method cannot
synchronize writers or discover omitted databases. Automated browser collection
and coordinated live restoration remain separate integration work.

Native and actual Chromium worker tests combine HD Quai/Qi owners, account and
mixed HD/payment-input signed transactions, both payment directions, pending and
abandoned ranges, and exhaustion across zones/networks. Encryption restores both
ledgers and channels to SQLite and recaptures them with the same seed ownership.
The browser test initializes all journal types in one database and verifies that
signed claims remain held. See the [retained report](../test-infra/reports/portable-wallet-capture-2026-09-13.json).
