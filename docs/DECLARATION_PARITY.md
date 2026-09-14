# Final declaration and utility review

Reference: published `quais@1.0.0-alpha.57`. This review reconciles the remaining
235 declarations across 57 groups. Runtime declarations were inspected in the
installed package, and interface/type aliases were checked against their `.d.ts`
definitions. The linked tests establish specific behavior; a type alias does not
by itself require another Rust wrapper or constitute an independent user feature.

## New utilities

`quai_primitives::numeric` supplies explicit signed integer text parsing, unsigned
byte/hex/quantity conversion, safe-number bridges and allocation-free hex shape
validation. `parse_integer` uses the existing `SignedUnits` range of -2^511 through
2^511-1, bounded to 1024 input bytes. `parse_uint` narrows to U256. Byte-to-U256
conversion accepts at most 1 MiB including leading zeros and 32 significant bytes.
General byte/hex padding is capped at 1 MiB before hexadecimal expansion.

Decimal, lowercase `0x`/`0o`/`0b` prefixes and an optional minus are accepted.
Whitespace, plus signs, uppercase prefixes, fractions and exponent notation are
rejected. This deliberately rejects source coercions such as whitespace or a
lone minus becoming zero. General quais.js BigInt operations are unbounded;
applications needing larger mathematical integers can choose their own integer
library. SDK chain quantities have explicit protocol-sized representations.

`integer_from_safe_number` and `integer_to_safe_number` are explicit f64 bridges,
limited to integral finite magnitudes at most 2^53-1. They never participate
implicitly in transaction amount, nonce or fee handling. Negative zero normalizes
to positive zero. `uint_to_be_array(0)` is empty, `uint_to_be_hex(0,None)` is
`0x00`, and `uint_to_quantity(0)` is `0x0`. Fixed-width hex rejects overflow,
including a zero-byte width for zero. `HexFormat::{Any,Bytes,Exact}` separates odd
hex digits from complete byte strings without allocation.

`SignedUnits::{INT256_MIN,INT256_MAX}` and `QUAIS_SYMBOL` retain the published
constants. `Zone::metadata`, `Shard::metadata` and `Shard::ALL` expose every
published label and encoded value. Source `context` is 2 for zones, regions and
Prime alike; the Rust enum supplies the actual hierarchy distinction.

`quai_crypto::recover_message_signer` and
`quai_sdk::recover_typed_data_signer` return recovered addresses for exact message
bytes or an immutable validated typed-data document. Recovery is not permission:
compare that address with an expected account and check chain/domain policy where
applicable. A changed message can recover a different valid address instead of
returning an error. The Rust SDK `VERSION` reports its own Cargo package version.

## How to interpret the mappings

The ledger now has no unreviewed declaration rows. Its `deviation` status records
specific representation, scheduling, resource or behavior differences. It does
not mean that the named operation is absent, and it also does not imply exact
behavioral identity. The table below preserves those distinctions explicitly.

Rust uses typed values, ownership, privacy, traits, futures and Result in places
where JavaScript uses property bags, duck typing, runtime guards, callback arrays
and exception-code strings. For example, a valid `Address` value replaces an
object that merely has a `getAddress` method; a `RemoteError` retains the actual
numeric server code instead of inferring nonce/fee errors from arbitrary text.
Local wallet/preflight errors have separate typed variants.

The intentionally restricted surfaces remain visible: finite integer/collection
limits; no global cryptographic replacement hooks; explicit event-loop ownership;
no hidden RPC batching/cache/static-network bypass; validated encryption with
fresh entropy; supported typed injected-wallet operations; explicit supported
block selectors. These must not be represented as byte-for-byte compatibility
with all inputs accepted by the JS runtime.

The seven source utility tests pin numeric/hex quirks, code-only error guards,
property/future helpers, shallow request copying, deferred ABI errors and global
crypto locking. Eight shared native/Chromium tests check numeric boundaries,
message/domain recovery and all shard metadata rows against the source fixtures.
Existing workflow tests cited below supply the evidence for the type mappings.

## Declaration mappings

### `getBigInt`, `BigNumberish`

Explicit signed 512-bit integer text/values replace unbounded BigInt unions. Decimal and 0x/0o/0b prefixes accept a leading minus; bounded grammar rejects whitespace, plus, uppercase prefix and the source lone-minus-as-zero coercion. No floating-point conversion is implicit.

Rust: `quai_primitives::numeric::parse_integer`, `quai_primitives::SignedUnits`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `getUint`

Exact nonnegative chain integers are bounded to U256. Negative/out-of-range values fail instead of silently wrapping; signed general interchange is separate.

Rust: `quai_primitives::numeric::parse_uint`, `quai_primitives::SignedUnits::try_u256`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `getNumber`, `toNumber`, `Numeric`

Explicit f64 bridges accept only integral finite values within plus/minus 2^53-1; native chain quantities remain exact U256 and indexes use checked integer conversions. Text/byte parsing is explicit before conversion, rather than dynamic union coercion.

Rust: `quai_primitives::numeric::integer_from_safe_number`, `quai_primitives::numeric::integer_to_safe_number`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `toBigInt`

Text parsing supports bounded signed 512-bit values; byte-to-chain-integer conversion allows bounded leading-zero padding and at most 32 significant bytes. Empty bytes are zero. General larger byte integers can use a caller-selected integer type; the SDK does not expose unbounded allocation.

Rust: `quai_primitives::numeric::parse_integer`, `quai_primitives::numeric::uint_from_be_bytes`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `toBeArray`, `toBeHex`, `toQuantity`

Exact U256 conversions retain source zero distinctions: empty byte array, 0x00 byte hex, 0x0 quantity. Optional byte padding is checked and bounded; zero width rejects even zero, and large integers never pass through JS numbers.

Rust: `quai_primitives::numeric::uint_to_be_array`, `quai_primitives::numeric::uint_to_be_hex`, `quai_primitives::numeric::uint_to_quantity`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `isHexString`, `isBytesLike`, `BytesLike`, `HexString`, `DataHexString`

Explicit borrowed byte slices or validated prefixed hex replace dynamic BytesLike unions. Allocation-free hex predicates distinguish arbitrary digits, complete bytes and exact widths; odd nibbles may be general hex but are not bytes. Uppercase 0X is rejected consistently rather than the source getBytes/predicate discrepancy.

Rust: `quai_primitives::numeric::is_hex_string`, `quai_primitives::numeric::HexFormat`, `quai_primitives::get_bytes`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `MaxInt256`

Exact 2^255-1 signed constant; source fixture matches without floating point.

Rust: `quai_primitives::SignedUnits::INT256_MAX`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `MinInt256`

Exact -2^255 signed constant; source fixture and two-complement boundary match.

Rust: `quai_primitives::SignedUnits::INT256_MIN`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `quaisymbol`

Exact published Greek capital Xi display constant; no amount or ledger semantics are attached.

Rust: `quai_primitives::QUAIS_SYMBOL`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `version`, `quais`

Rust crate namespaces and Cargo-selected exports replace the nested JS module object. VERSION reports this Rust package version, independently of the explicitly pinned JS reference; no global mutable runtime namespace.

Rust: `quai_sdk::VERSION`, `quai_sdk`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `verifyMessage`

Recover from the exact prefixed message bytes and validated low-S signature, then compare the returned address with an expected signer. Hex-looking strings remain UTF-8 text when supplied as bytes; recovery alone is not authorization.

Rust: `quai_crypto::recover_message_signer`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `verifyTypedData`

Recover from an immutable validated typed-data document and canonical signature; the caller must independently compare the recovered signer and apply chain/domain policy. Invalid schema and duplicate JSON keys fail before recovery.

Rust: `quai_sdk::recover_typed_data_signer`, `quai_abi::TypedData`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

Details: [TYPED_DATA_PARITY](../docs/TYPED_DATA_PARITY.md).

### `Zone`, `Shard`, `toZone`, `toShard`

Typed hierarchy preserves the 0x/0x0/0x00 distinction, all published names/nicknames/IDs/encodings and explicit parse failure for unknown identifiers. Published zone existence does not imply network activation.

Rust: `quai_primitives::Zone`, `quai_primitives::Shard`, `quai_primitives::Region`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-primitives/tests/primitives.rs](../crates/quai-primitives/tests/primitives.rs).

### `ZoneData`, `ShardData`

Immutable metadata reproduces every published name/nickname/shard/byte/context value. The source context field is 2 even for Prime/regions; callers use the typed enum for hierarchy, not that metadata field.

Rust: `quai_primitives::Zone::metadata`, `quai_primitives::Shard::metadata`, `quai_primitives::Shard::ALL`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `Ledger`

Typed Quai/Qi ledger variants and validated address-bit extraction replace numeric enum values; ledger identity and zone validity remain distinct checks.

Rust: `quai_primitives::Ledger`, `quai_primitives::Address::ledger`.

Evidence: [crates/quai-primitives/tests/primitives.rs](../crates/quai-primitives/tests/primitives.rs), [crates/quai-primitives/tests/differential.rs](../crates/quai-primitives/tests/differential.rs).

### `AddressLike`, `Addressable`, `isAddressable`

Callers resolve their futures or custom objects explicitly into validated Address values; signer/contract accessors retain concrete identity. The source isAddressable only checks for a getAddress function and does not validate its result. Rust does not accept arbitrary promise-shaped objects as addresses.

Rust: `quai_primitives::Address`, `quai_signer::Signer::address`, `quai_sdk::contracts::Contract`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-sdk/tests/contracts.rs](../crates/quai-sdk/tests/contracts.rs).

Details: [CONTRACT_PARITY](../docs/CONTRACT_PARITY.md).

### `AddressStatus`, `QiAddressInfo`

All four advisory usage states, origin, public metadata and last observation are explicit. Positive use is retained conservatively; empty/latest observations never authorize address reuse or prove a historical checkpoint.

Rust: `quai_wallet::qi_addresses::QiAddressStatus`, `quai_wallet::qi_addresses::QiAddressRecord`, `quai_wallet::qi_addresses::QiAddressBook`.

Evidence: [crates/quai-sdk/tests/qi_address_book.rs](../crates/quai-sdk/tests/qi_address_book.rs).

Details: [QI_ADDRESS_VIEWS](../docs/QI_ADDRESS_VIEWS.md).

### `AllowedCoinType`, `NeuteredAddressInfo`

Typed 969/994 coin choice and checked public address origin replace loose account/index/zone records. Imported keys have no fabricated HD ancestry; private material is absent from public metadata.

Rust: `quai_wallet::CoinType`, `quai_wallet::metadata::PublicAddress`, `quai_wallet::metadata::KeyOrigin`.

Evidence: [crates/quai-wallet/tests/reference.rs](../crates/quai-wallet/tests/reference.rs), [crates/quai-sdk/tests/key_origins.rs](../crates/quai-sdk/tests/key_origins.rs).

Details: [HD_WALLET_PARITY_REVIEW](../docs/HD_WALLET_PARITY_REVIEW.md).

### `OutpointInfo`

Exact outpoint/denomination/lock information and separately verified owner origins replace a loose combined record. Zone is derived and cross-checked; derivation metadata does not prove current spendability.

Rust: `quai_wallet::CandidateCoin`, `quai_wallet::metadata::PublicAddress`, `quai_provider::AddressOutpoint`.

Evidence: [crates/quai-wallet/tests/selection.rs](../crates/quai-wallet/tests/selection.rs), [crates/quai-sdk/tests/qi_address_book.rs](../crates/quai-sdk/tests/qi_address_book.rs).

Details: [QI_ADDRESS_VIEWS](../docs/QI_ADDRESS_VIEWS.md).

### `SerializedHDWallet`, `SerializedQiHDWallet`

Explicit bounded version-one interchange validates a trusted root and rederives ownership, discarding untrusted cached spend/use claims. Guarded encrypted Rust backups preserve native/browser journal identities and anti-reuse state; plaintext JS phrase metadata is never implicitly serialized.

Rust: `quai_wallet::full_backup::legacy::import_quais_json`, `quai_wallet::full_backup::legacy::export_quais_json`, `quai_wallet::full_backup::WalletBackup`.

Evidence: [crates/quai-sdk/tests/legacy_wallets.rs](../crates/quai-sdk/tests/legacy_wallets.rs), [crates/quai-sdk/tests/portable_capture.rs](../crates/quai-sdk/tests/portable_capture.rs).

Details: [LEGACY_WALLET_MIGRATION](../docs/LEGACY_WALLET_MIGRATION.md), [PORTABLE_WALLET_CAPTURE](../docs/PORTABLE_WALLET_CAPTURE.md).

### `SpendTarget`

Typed Qits and explicit fresh destination pools replace a loose address/value pair. Selection and address allocation are separate; fixed denominations can require multiple outputs for one target.

Rust: `quai_sdk::qi_preflight::QiIntent`, `quai_wallet::SelectionRequest`.

Evidence: [crates/quai-sdk/tests/qi_preflight.rs](../crates/quai-sdk/tests/qi_preflight.rs), [crates/quai-wallet/tests/selection.rs](../crates/quai-wallet/tests/selection.rs).

Details: [QI_SELECTION_PARITY_REVIEW](../docs/QI_SELECTION_PARITY_REVIEW.md).

### `SignatureLike`

Explicit canonical compact/parity/legacy metadata parsers replace ambiguous string/object unions. High-S and invalid recovery values fail; metadata extraction does not silently alter signed intent.

Rust: `quai_crypto::RecoverableSignature`, `quai_crypto::SignatureMetadata`.

Evidence: [crates/quai-crypto/tests/metadata.rs](../crates/quai-crypto/tests/metadata.rs), [crates/quai-crypto/tests/utilities.rs](../crates/quai-crypto/tests/utilities.rs).

Details: [CRYPTO_PARITY](../docs/CRYPTO_PARITY.md).

### `EncryptOptions`

Production export selects fresh OS/Web Crypto salt, IV and UUID with pinned strong scrypt defaults; predictable caller-supplied entropy and unchecked legacy metadata are not encryption options. Import and standalone KDF cost limits are explicit; guarded mnemonic export rederives ownership.

Rust: `quai_keystore::encrypt`, `quai_keystore::encrypt_with_mnemonic`, `quai_keystore::KdfLimits`, `quai_keystore::derive::DeriveParams`.

Evidence: [crates/quai-sdk/tests/utility_completion.rs](../crates/quai-sdk/tests/utility_completion.rs), [crates/quai-keystore/src/lib.rs](../crates/quai-keystore/src/lib.rs).

Details: [CRYPTO_PARITY](../docs/CRYPTO_PARITY.md), [UTILITY_PARITY](../docs/UTILITY_PARITY.md).

### `FixedFormat`

Validated signedness, byte-aligned 8..256-bit width and decimal scale replace loose format properties; checked fixed-point arithmetic and explicit rounding retain exact values.

Rust: `quai_primitives::FixedFormat`.

Evidence: [crates/quai-primitives/tests/fixed.rs](../crates/quai-primitives/tests/fixed.rs).

### `FormatType`, `FragmentType`, `InterfaceAbi`, `JsonFragment`, `JsonFragmentType`

Typed ABI formatting and function/event/error/constructor/parameter declarations replace string tag and JSON/human-fragment unions. Bounded parsers validate canonical declaration structure before reflection or encoding; unsupported dynamic fragment behavior remains explicitly documented.

Rust: `quai_abi::AbiFormat`, `quai_abi::AbiInterface`, `quai_abi::AbiParameter`, `quai_abi::AbiType`.

Evidence: [crates/quai-abi/tests/reflection.rs](../crates/quai-abi/tests/reflection.rs), [crates/quai-abi/tests/human.rs](../crates/quai-abi/tests/human.rs).

Details: [ABI_REFLECTION_PARITY](../docs/ABI_REFLECTION_PARITY.md).

### `ParamTypeWalkFunc`, `ParamTypeWalkAsyncFunc`

Rust closures/futures visit validated primitive leaves in declaration/index order. Shape validation precedes callbacks and result JSON stays bounded; async execution is explicitly sequential and callback side effects are not rolled back.

Rust: `quai_abi::AbiParameter::walk`, `quai_abi::AbiParameter::walk_async`.

Evidence: [crates/quai-abi/tests/reflection.rs](../crates/quai-abi/tests/reflection.rs), [crates/quai-sdk/tests/human_abi.rs](../crates/quai-sdk/tests/human_abi.rs).

Details: [ABI_REFLECTION_PARITY](../docs/ABI_REFLECTION_PARITY.md).

### `TypedDataDomain`, `TypedDataField`

Typed field declarations and validated canonical domain JSON replace permissive optional-field interfaces. Chain identifiers remain exact; schema/domain declarations and recursive values obey explicit resource constraints.

Rust: `quai_abi::TypedDataField`, `quai_abi::TypedData`, `quai_abi::hash_domain`.

Evidence: [crates/quai-abi/tests/typed_data.rs](../crates/quai-abi/tests/typed_data.rs), [crates/quai-abi/tests/typed_utils.rs](../crates/quai-abi/tests/typed_utils.rs).

Details: [TYPED_DATA_PARITY](../docs/TYPED_DATA_PARITY.md).

### `checkResultErrors`

Rust ABI decode validates eagerly and returns Result before exposing decoded values, so no deferred error objects are hidden behind array getters. Named result lookup remains explicit; source deferred-error path behavior is pinned in a regression.

Rust: `quai_abi::AbiResult`, `quai_abi::AbiError`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-abi/tests/reflection.rs](../crates/quai-abi/tests/reflection.rs).

Details: [ABI_REFLECTION_PARITY](../docs/ABI_REFLECTION_PARITY.md).

### `BlockTag`

Explicit Latest/Pending/Number tags replace unrestricted strings and numeric coercion. Genesis is Number(0); node signed-64-bit bounds are checked. Hash reads use explicit hash methods; safe/finalized/negative relative aliases are not silently reinterpreted as latest or finality.

Rust: `quai_provider::BlockTag`, `quai_provider::Provider::header_by_hash`.

Evidence: [crates/quai-provider/src/lib.rs](../crates/quai-provider/src/lib.rs), [crates/quai-sdk/tests/response_views.rs](../crates/quai-sdk/tests/response_views.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `BlockParams`, `MinedBlock`

Typed hash/full block modes and bounded metadata preserve source block/work-object fields. Parsed mined context is node-reported and does not prove canonicality/finality; explicit confirmation observations provide their documented additional checks.

Rust: `quai_provider::MinedBlock`, `quai_provider::BlockHashes`, `quai_provider::BlockMetadata`, `quai_provider::ZoneHeader`.

Evidence: [crates/quai-sdk/tests/response_views.rs](../crates/quai-sdk/tests/response_views.rs).

Details: [RESPONSE_PARITY](../docs/RESPONSE_PARITY.md).

### `LogParams`, `TransactionReceiptParams`, `TransactionResponseParams`

Typed exact RPC views, checked inclusion/signature/quantity fields and bounded extension maps replace constructor parameter bags. Raw response metadata remains inspectable without JS number truncation; confirmed execution is a separate observation.

Rust: `quai_provider::Log`, `quai_provider::Receipt`, `quai_provider::Transaction`, `quai_provider::TransactionDetails`, `quai_provider::Extensions`.

Evidence: [crates/quai-sdk/tests/response_views.rs](../crates/quai-sdk/tests/response_views.rs), [crates/quai-sdk/tests/transaction_documents.rs](../crates/quai-sdk/tests/transaction_documents.rs).

Details: [RESPONSE_PARITY](../docs/RESPONSE_PARITY.md), [TRANSACTION_INTERCHANGE_PARITY](../docs/TRANSACTION_INTERCHANGE_PARITY.md).

### `MinedTransactionResponse`

Inclusion fields and a distinct bounded confirmation observation replace a TypeScript nonnull intersection. Current canonical membership/depth is checked explicitly; neither a populated inclusion object nor a response constructor proves finality.

Rust: `quai_provider::Inclusion`, `quai_provider::Transaction`, `quai_provider::ConfirmedTransaction`.

Evidence: [crates/quai-sdk/tests/transaction_confirmation.rs](../crates/quai-sdk/tests/transaction_confirmation.rs).

Details: [TRANSACTION_RESPONSE_PARITY](../docs/TRANSACTION_RESPONSE_PARITY.md).

### `TransactionRequest`, `PreparedTransactionRequest`, `QuaiTransactionRequest`, `QuaiPreparedTransactionRequest`, `QiTransactionRequest`, `QiPreparedTransactionRequest`, `JsonRpcTransactionRequest`, `QuaiJsonRpcTransactionRequest`, `QiJsonRpcTransactionRequest`, `PerformActionTransaction`

Explicit account/Qi intent, simulation and canonical wire models replace optional mixed request unions. U256 chain amounts and full u64 nonce remain exact; access-list order and specialized Qi data are retained. Quote/population and signing are separate stages, and arbitrary customData is caller-owned rather than silently serialized to consensus.

Rust: `quai_consensus::TransactionDocument`, `quai_consensus::QuaiTransaction`, `quai_consensus::QiTransaction`, `quai_provider::CallRequest`, `quai_sdk::account_preflight::AccountIntent`, `quai_sdk::qi_preflight::QiOperationIntent`.

Evidence: [crates/quai-sdk/tests/transaction_documents.rs](../crates/quai-sdk/tests/transaction_documents.rs), [crates/quai-sdk/tests/account_preflight.rs](../crates/quai-sdk/tests/account_preflight.rs), [crates/quai-sdk/tests/qi_preflight.rs](../crates/quai-sdk/tests/qi_preflight.rs).

Details: [TRANSACTION_INTERCHANGE_PARITY](../docs/TRANSACTION_INTERCHANGE_PARITY.md), [WALLET_WORKFLOWS](../docs/WALLET_WORKFLOWS.md).

### `copyRequest`

Owned typed values implement explicit Clone or validated JSON construction; no JS coercion pass or unknown-property dropping is needed. Source copyRequest retains customData by reference and only shallow-copies Qi entries; Rust ownership makes copy/borrow decisions visible.

Rust: `quai_consensus::TransactionDocument`, `quai_provider::CallRequest`, `quai_sdk::account_preflight::AccountIntent`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-sdk/tests/transaction_documents.rs](../crates/quai-sdk/tests/transaction_documents.rs).

Details: [TRANSACTION_INTERCHANGE_PARITY](../docs/TRANSACTION_INTERCHANGE_PARITY.md).

### `EventFilter`, `Filter`, `FilterByBlockHash`, `PerformActionFilter`, `TopicFilter`

Explicit zone, address and positional topic filters support exact topics, wildcard positions and bounded OR lists. BlockHash and inclusive numeric ranges are mutually exclusive, with no unbounded implicit genesis-to-latest scan.

Rust: `quai_provider::LogFilter`, `quai_provider::LogRange`, `quai_provider::TopicMatch`.

Evidence: [crates/quai-sdk/tests/events.rs](../crates/quai-sdk/tests/events.rs), [crates/quai-sdk/tests/contract_io.rs](../crates/quai-sdk/tests/contract_io.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md), [CONTRACT_PARITY](../docs/CONTRACT_PARITY.md).

### `OrphanFilter`

Typed detached/attached canonical head updates, removed-log metadata and explicit candidate/replacement observations replace opaque orphan filter objects. Callers drive bounded replay and fan-out; notification loss or disappearance never automatically releases claims.

Rust: `quai_provider::HeadTracker`, `quai_provider::HeadUpdate`, `quai_provider::Log`, `quai_provider::AccountReplacementTracker`.

Evidence: [crates/quai-sdk/tests/replay.rs](../crates/quai-sdk/tests/replay.rs), [crates/quai-sdk/tests/receipt_confirmation.rs](../crates/quai-sdk/tests/receipt_confirmation.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md), [ACCOUNT_NONCE_REPLACEMENTS](../docs/ACCOUNT_NONCE_REPLACEMENTS.md).

### `EventEmitterable`, `Listener`, `ProviderEvent`, `Subscriber`, `Subscription`

Typed caller-owned event keys/payloads, bounded queues and explicit network subscription ownership replace callback arrays/subscriber subclass unions. Registration, once/off, pause/drop/buffer, unsubscribe, lag and closure are explicit; application code drives delivery and no hidden unbounded Promise chain or finality event is invented.

Rust: `quai_provider::event_hub::EventHub`, `quai_provider::event_hub::ListenerId`, `quai_rpc::WsTransport::subscribe`, `quai_provider::WsHeadFollower`.

Evidence: [crates/quai-sdk/tests/provider_events.rs](../crates/quai-sdk/tests/provider_events.rs), [compatibility/scripts/subscriber-lifecycle.test.mjs](../compatibility/scripts/subscriber-lifecycle.test.mjs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md), [UTILITY_PARITY](../docs/UTILITY_PARITY.md).

### `Provider`, `PerformActionRequest`

A chain-bound provider over an explicit transport supplies typed reads, simulation, observation and immutable signed broadcast. Low-level method dispatch is Transport::request; request-specific types replace one dynamic discriminated union and methods do not secretly create timers or wallet authority.

Rust: `quai_provider::Provider`, `quai_rpc::Transport`.

Evidence: [crates/quai-sdk/tests/response_views.rs](../crates/quai-sdk/tests/response_views.rs), [crates/quai-sdk/tests/account_preflight.rs](../crates/quai-sdk/tests/account_preflight.rs), [crates/quai-sdk/tests/qi_preflight.rs](../crates/quai-sdk/tests/qi_preflight.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `Signer`

Local signing traits and explicit remote/browser adapters replace a provider-bound JS signer interface. Account/Qi signing, personal and typed messages are distinct; population/sending/nonce policy compose provider and durable sessions instead of implicit mutable binding.

Rust: `quai_signer::Signer`, `quai_signer::LocalSigner`, `quai_signer::WatchOnlySigner`, `quai_sdk::rpc_signer::RpcAccountSigner`.

Evidence: [crates/quai-signer/tests/signers.rs](../crates/quai-signer/tests/signers.rs), [crates/quai-sdk/tests/rpc_signer.rs](../crates/quai-sdk/tests/rpc_signer.rs).

Details: [SIGNER_PARITY_REVIEW](../docs/SIGNER_PARITY_REVIEW.md), [RPC_SIGNER](../docs/RPC_SIGNER.md).

### `JsonRpcPayload`, `JsonRpcResult`, `JsonRpcError`

Transport encodes and correlates bounded JSON-RPC 2.0 envelopes with nonreused request IDs, returning validated result JSON or explicit numeric remote errors. Protocol fields are not arbitrary caller-mutated bags; untrusted remote text/data are explicitly accessible but redacted from diagnostics.

Rust: `quai_rpc::Transport`, `quai_rpc::RemoteError`, `quai_rpc::RpcError`.

Evidence: [crates/quai-rpc/tests/http.rs](../crates/quai-rpc/tests/http.rs), [crates/quai-rpc/tests/websocket.rs](../crates/quai-rpc/tests/websocket.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `JsonRpcApiProviderOptions`

Explicit size/deadline/concurrency/routing configuration replaces inherited cache/batch/static-network knobs. Every high-level endpoint read checks chain ID; callers may issue concurrent independent requests but there is no hidden batching, stale result cache or static-network bypass.

Rust: `quai_rpc::HttpConfig`, `quai_rpc::WsConfig`, `quai_rpc::Routing`, `quai_browser::BrowserConfig`.

Evidence: [crates/quai-rpc/tests/http.rs](../crates/quai-rpc/tests/http.rs), [crates/quai-rpc/tests/routing_differential.rs](../crates/quai-rpc/tests/routing_differential.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `Eip1193Provider`, `DebugEventBrowserProvider`

An application-selected injected provider is wrapped with bounded requests, context checks and explicit supported wallet operations. Debug instrumentation is an application-owned transport wrapper/event payload, avoiding automatic emission of potentially sensitive request data. Arbitrary extension permission/chain-switch methods are outside the supported typed wallet adapter.

Rust: `quai_browser::InjectedProvider`, `quai_browser::BrowserConfig`, `quai_rpc::Transport`.

Evidence: [crates/quai-browser/src/lib.rs](../crates/quai-browser/src/lib.rs), [crates/quai-sdk/tests/rpc_signer.rs](../crates/quai-sdk/tests/rpc_signer.rs).

Details: [RPC_SIGNER](../docs/RPC_SIGNER.md), [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `WebSocketCreator`, `WebSocketLike`

Explicit native/browser transport constructors and ownership replace JS duck-typed socket factories. Bounded frames, request correlation, subscriptions, close/lag and cancellation are qualified on the supported native and browser implementations; custom protocols use the Transport trait.

Rust: `quai_rpc::WsTransport`, `quai_rpc::WsConfig`, `quai_browser::BrowserWebSocketTransport`, `quai_browser::BrowserSocketConfig`.

Evidence: [crates/quai-rpc/tests/websocket.rs](../crates/quai-rpc/tests/websocket.rs), [crates/quai-sdk/tests/provider_events.rs](../crates/quai-sdk/tests/provider_events.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `ErrorCode`, `CodedquaisError`, `quaisError`, `UnknownError`, `makeError`, `isError`, `isCallException`

Scoped Rust error enums, Result and pattern matching replace extensible string-code exception objects and unchecked JS code-only type guards. No universal regex classification of remote messages or automatic inclusion of secret input in error strings; explicit remote numeric/data inspection and ABI revert decoding remain available.

Rust: `quai_rpc::RpcError`, `quai_rpc::RemoteError`, `quai_provider::ProviderError`, `quai_abi::ParsedRevert`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-rpc/tests/http.rs](../crates/quai-rpc/tests/http.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `NetworkError`, `ServerError`

Transport/HTTP/remote failures and explicit chain mismatch variants replace broad JS networking exceptions. This mapping is unrelated to the new network-metadata registry error type; remote messages/data remain inspectable and redacted.

Rust: `quai_rpc::RpcError`, `quai_provider::ProviderError::ChainMismatch`.

Evidence: [crates/quai-rpc/tests/http.rs](../crates/quai-rpc/tests/http.rs), [crates/quai-sdk/tests/rpc_signer.rs](../crates/quai-sdk/tests/rpc_signer.rs).

Details: [PROVIDER_PARITY](../docs/PROVIDER_PARITY.md).

### `TimeoutError`, `CancelledError`

Bounded deadlines, explicit fetch cancellation and future drop replace exception tagging. Cancelling a wait does not prove that a dispatched signing/submission action was cancelled; phase-aware errors and durable claims retain ambiguity.

Rust: `quai_rpc::RpcError::Timeout`, `quai_provider::WaitError`, `quai_rpc::fetch::FetchCancellation`.

Evidence: [crates/quai-rpc/tests/fetch.rs](../crates/quai-rpc/tests/fetch.rs), [crates/quai-sdk/tests/rpc_signer.rs](../crates/quai-sdk/tests/rpc_signer.rs), [crates/quai-sdk/tests/receipt_confirmation.rs](../crates/quai-sdk/tests/receipt_confirmation.rs).

Details: [FETCH_PARITY](../docs/FETCH_PARITY.md), [RPC_SIGNER](../docs/RPC_SIGNER.md).

### `BadDataError`, `BufferOverrunError`

Checked slices/widths, bounded parsers and typed response validation return domain errors instead of deferred RangeError objects. Errors omit raw input contents; checked bounds never silently truncate.

Rust: `quai_primitives::EncodingError`, `quai_abi::AbiError`, `quai_rpc::RpcError::InvalidResponse`, `quai_provider::ProviderError::InvalidResult`.

Evidence: [crates/quai-primitives/tests/encoding.rs](../crates/quai-primitives/tests/encoding.rs), [crates/quai-abi/tests/abi.rs](../crates/quai-abi/tests/abi.rs), [crates/quai-rpc/tests/http.rs](../crates/quai-rpc/tests/http.rs).

### `NumericFaultError`

Checked signed/unsigned amount, width, scale and rounding operations return typed failures; floating-point bridges are explicit and reject unsafe values. No error message embeds arbitrary original input.

Rust: `quai_primitives::AmountError`, `quai_primitives::FixedError`, `quai_primitives::EncodingError`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-primitives/tests/fixed.rs](../crates/quai-primitives/tests/fixed.rs).

### `InvalidArgumentError`, `MissingArgumentError`, `UnexpectedArgumentError`, `assert`, `assertArgument`, `assertArgumentCount`, `assertPrivate`, `defineProperties`

Rust function arity, field types, private constructors/fields and Result validation replace runtime JS assertions and Object.defineProperty guards. Dynamic ABI requests still validate exact parameter counts/shapes. SDK validation returns errors rather than panicking on caller data or mutating a partially defined object.

Rust: `quai_abi::AbiError`, `quai_provider::ProviderError::InvalidRequest`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs), [crates/quai-abi/tests/abi.rs](../crates/quai-abi/tests/abi.rs).

### `assertNormalize`

A typed normalization enum and compiled Unicode implementation replace probing host String.prototype.normalize. The supported forms and Unicode version are explicit and tested on native and browser platforms.

Rust: `quai_primitives::Utf8Normalization`, `quai_primitives::to_utf8_bytes`.

Evidence: [crates/quai-primitives/tests/text.rs](../crates/quai-primitives/tests/text.rs).

Details: [CRYPTO_PARITY](../docs/CRYPTO_PARITY.md).

### `resolveProperties`

Callers explicitly await owned fields/futures before constructing typed request values. Rust async composition replaces a dynamic JS object/Promise.all helper; no hidden network requests or recursive arbitrary property resolution is introduced.

Rust: `core::future::Future`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

### `lock`

Compiled cryptographic backends have no process-global mutable registration hooks, so their implementation cannot be replaced at runtime and requires no lock call. Caller-selected transports/KDF settings remain explicit and independently bounded.

Rust: `quai_crypto`, `quai_keystore::derive`.

Evidence: [crates/quai-sdk/tests/final_utilities.rs](../crates/quai-sdk/tests/final_utilities.rs), [compatibility/scripts/final-utilities.test.mjs](../compatibility/scripts/final-utilities.test.mjs).

Details: [CRYPTO_PARITY](../docs/CRYPTO_PARITY.md), [UTILITY_PARITY](../docs/UTILITY_PARITY.md).

### `NotImplementedError`, `UnsupportedOperationError`

Scoped unsupported-operation variants describe watch-only signing, unsupported adapter operations or unavailable qualified node estimation without pretending success. Feature-gated platform APIs are compile-time choices rather than JS abstract methods throwing at runtime.

Rust: `quai_signer::SignerError`, `quai_browser::BrowserError`, `quai_provider::ProviderError`.

Evidence: [crates/quai-signer/tests/signers.rs](../crates/quai-signer/tests/signers.rs), [crates/quai-browser/src/lib.rs](../crates/quai-browser/src/lib.rs).

Details: [SIGNER_PARITY_REVIEW](../docs/SIGNER_PARITY_REVIEW.md).

### `CallExceptionAction`, `CallExceptionError`, `CallExceptionTransaction`

Call/simulation context remains the caller-owned typed request; remote failure data is explicitly decoded into standard/custom ABI reverts. The SDK does not infer operation success or classify arbitrary remote strings as trusted reverts.

Rust: `quai_provider::CallRequest`, `quai_rpc::RemoteError`, `quai_abi::AbiInterface::parse_revert`, `quai_sdk::contracts::ContractError`.

Evidence: [crates/quai-sdk/tests/contracts.rs](../crates/quai-sdk/tests/contracts.rs), [crates/quai-abi/tests/workflows.rs](../crates/quai-abi/tests/workflows.rs).

Details: [CONTRACT_PARITY](../docs/CONTRACT_PARITY.md).

### `InsufficientFundsError`

Typed local selection/preflight failures distinguish insufficient spendable value; remote node errors retain numeric code and inspectable data without brittle message inference or secret transaction echoes.

Rust: `quai_wallet::SelectionError::InsufficientFunds`, `quai_sdk::account_preflight::AccountPreflightError`, `quai_rpc::RemoteError`.

Evidence: [crates/quai-wallet/tests/selection.rs](../crates/quai-wallet/tests/selection.rs), [crates/quai-sdk/tests/account_preflight.rs](../crates/quai-sdk/tests/account_preflight.rs).

### `NonceExpiredError`, `ReplacementUnderpricedError`, `TransactionReplacedError`

Explicit nonce reservations, monotone replacement fee policy and bounded canonical replacement observations replace exception-driven candidate replacement. Raw remote nonce/price errors remain inspectable; disappearance alone never releases nonce claims or rebroadcasts a competing transaction.

Rust: `quai_sdk::account_replacement::ReplacementPolicy`, `quai_provider::AccountReplacementPoll`, `quai_provider::ReplacementReason`, `quai_wallet::account_custody::AccountOperationBook`, `quai_rpc::RemoteError`.

Evidence: [crates/quai-sdk/tests/account_custody.rs](../crates/quai-sdk/tests/account_custody.rs), [crates/quai-sdk/tests/receipt_confirmation.rs](../crates/quai-sdk/tests/receipt_confirmation.rs).

Details: [ACCOUNT_NONCE_REPLACEMENTS](../docs/ACCOUNT_NONCE_REPLACEMENTS.md).

### `ActionRejectedError`

Injected EIP-1193 numeric rejection codes (including 4001), remote numeric errors and explicit dispatch phase replace inferred reason/action strings. A rejection or timeout after dispatch does not automatically release durable claims.

Rust: `quai_browser::BrowserError::Provider`, `quai_sdk::rpc_signer::RpcSignerError`, `quai_rpc::RemoteError`.

Evidence: [crates/quai-browser/src/lib.rs](../crates/quai-browser/src/lib.rs), [crates/quai-sdk/tests/rpc_signer.rs](../crates/quai-sdk/tests/rpc_signer.rs).

Details: [RPC_SIGNER](../docs/RPC_SIGNER.md).

