# Provider declaration review — September 13, 2026

This review reconciles 144 inventoried rows for 12 behaviors inherited across
AbstractProvider, BrowserProvider, JsonRpcApiProvider, JsonRpcProvider,
SocketProvider and WebSocketProvider, each exported at the root and providers
subpath. It does not claim that 144 new behaviors were implemented, remove any
declaration from the inventory or establish overall SDK completeness.

The reference is `quais@1.0.0-alpha.57`, source commit
`94e32c7eb9960de36054135c40a341c44c84f922`. The offline
[generator](../compatibility/scripts/generate-provider-basics.mjs) walks prototype
ownership without constructing or connecting a provider. The
[fixture](../compatibility/fixtures/provider-basics.json) records 72 inherited
bindings, six RPC mappings and two unsupported actions. All reviewed methods are
defined on AbstractProvider, except SocketProvider's WebSocket URL override.

| Reference behavior | Rust equivalent or explicit limitation | Evidence |
| --- | --- | --- |
| `getBlockNumber` | `Provider::block_number` takes an explicit shard and returns chain-checked U256 without JS Number narrowing | [Reference mapping](../crates/quai-provider/tests/basic_reference.rs), [routing/quantity checks](../crates/quai-provider/tests/provider.rs) |
| `getFeeData` | Pinned FeeData wraps gasPrice; `gas_price` returns the exact reported U256 with an explicit zone. Qi fee estimation is a separate method, without fallback prices or fabricated priority-fee fields | [Reference mapping](../crates/quai-provider/tests/basic_reference.rs), [Qi estimator](../crates/quai-provider/tests/qi.rs) |
| `getNetwork` | Explicit expected chain ID, route and optional trusted genesis comparison replace mutable/cached JS Network objects and implicit switching | [Chain/genesis checks](../crates/quai-provider/tests/provider.rs), [worker composition](../crates/quai-browser/tests/worker.rs) |
| `getRunningLocations` | `running_zones` queries an explicit Prime route, validates unique published zones and never adds routes implicitly. Arbitrary shard overrides are not supported | [Prime routing and malformed locations](../crates/quai-provider/tests/provider.rs), [reference mapping](../crates/quai-provider/tests/basic_reference.rs) |
| `estimateFeeForQi` | Exact validated ordinary input/output shape and Qit estimate; conversion/wrapping instead require the explicit qualified special-fee profile because the pinned estimator drops their data | [Ordinary Qi fees](../crates/quai-provider/tests/qi.rs), [special fees](../crates/quai-provider/tests/qi_special_fee.rs) |
| `broadcastTransaction` | Typed immutable verified Quai/Qi/conversion/wrapping envelopes, one submission, local-ID acknowledgement and retained ambiguity. Durable SDK sessions persist first; injected providers additionally require `signed_submission_transport` opt-in. No implicit response-class lookup or retry | [Quai submission](../crates/quai-provider/tests/submission.rs), [Qi submission](../crates/quai-provider/tests/qi.rs), [specialized envelopes](../crates/quai-provider/tests/conversions.rs) |
| `zoneFromAddress` | Synchronous typed address `.zone()` validates the published zone; no Promise or Addressable object | [Address/zone fixtures](../crates/quai-primitives/tests/differential.rs), [strict parsing](../crates/quai-primitives/tests/primitives.rs) |
| `_getAddress` | Rust typed construction/FromStr and checksummed Display replace dynamic literal/Promise/Addressable resolution; no implicit name service | [Address fixtures](../crates/quai-primitives/tests/differential.rs), [strict parsing](../crates/quai-primitives/tests/primitives.rs) |
| `_getBlockTag` | Typed Latest/Pending/Number, explicit number zero for earliest, signed-64-bit node limits and separate hash lookup methods. Relative offsets and `safe`/`finalized` are not inferred | [Block selectors](../crates/quai-provider/tests/provider.rs), [separate block/hash queries](../crates/quai-provider/tests/blocks.rs) |
| `validateUrl` | Structured HTTP(S)/WS(S) parsing plus explicit route/transport selection replaces the JS regex variants. Paths/query are preserved; authority credentials, fragments, controls and ambiguous normalization fail | [Routing comparison](../crates/quai-rpc/tests/routing_differential.rs), [endpoint policy](../crates/quai-rpc/src/routing.rs) |
| `waitForBlock` | Pinned method always throws `NOT_IMPLEMENTED`. No Rust method is added solely to reproduce that placeholder; explicit block/header reads and head tracking are separate APIs | [Executable pinned behavior](../compatibility/scripts/provider-basics.test.mjs) |
| `getTransactionResult` | Pinned JSON-RPC mapper returns null and `_perform` throws `UNSUPPORTED_OPERATION`. Historical execution-return data is not claimed; neither receipts nor call simulation are substitutes | [Executable pinned behavior](../compatibility/scripts/provider-basics.test.mjs) |

The pinned go-quai v0.56.0 source at
`f3f345c877300c044e3e0081a48bf3cf786fb9cc`, `rpc/types.go`, accepts `earliest`,
`latest`, `pending` or a hex block number through `math.MaxInt64`. Its
`BlockNumber.UnmarshalJSON` has no `safe` or `finalized` case. The JS normalizer
passing these strings through does not establish a finality-capable node API.
Callers can choose an explicitly observed height; the SDK does not fabricate a
safe/finalized checkpoint.

The fixture's empty Qi shape and `0x00` broadcast are sentinels used only to inspect
RPC mapping. They are not valid transactions and are never submitted. The Rust
comparison executes the four basic read mappings; existing signed-envelope and
fee tests supply the validation evidence for those separate transaction methods.

142 pending rows are now reconciled, and two existing injected-broadcast mappings
are expanded while retaining their explicit submission-capability requirement.
All reviewed rows are marked as explicit deviations with APIs, evidence and
limits. Other provider behaviors, runtime inheritance hooks, record properties,
browser wallet integration, real extension interoperability and independent
release qualification still require their own review. Counts are bookkeeping,
not a completion percentage.
