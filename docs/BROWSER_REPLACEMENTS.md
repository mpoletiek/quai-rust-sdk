# Reviewed browser replacements

Replacements retain the original durable family and claim set. They never imply
that the parent was dropped, cannot release its nonce/inputs, and do not guarantee
pool preference. Submission remains an explicit
`BrowserRecoverySession::broadcast_candidate(id, candidate_hash)` call.

## Account fee-only candidates

Call `BrowserAccountSession::prepare_replacement(id, parent_hash,
ReplacementPolicy)` for a persisted Signed or Submitted member. The portable
`account_replacement::quote_account_replacement` implements the same quotation
used by the native account session.

The quote preserves nonce, recipient, value, calldata, access list and gas limit.
Only gas price increases: at least the explicitly selected 1–1000 percent bump,
rounded upward, at least one unit above the parent, and at least the observed node
price. Explicit gas/price/total-debit limits still apply. The exact ordinary or
Quai-to-Qi estimate plus the selected margin must fit the retained gas limit.
There is no access discovery or automatic nonce replacement during this step.

Admission checks use a rechecked confirmed block's nonce. A higher pending nonce
is normal when the parent is pooled and does not by itself prevent replacement.
Pending or explicitly PinnedLatest balance/simulation policy remains selectable;
unsupported observations do not silently fall back. Chain/genesis and canonical
anchors are checked around the quote.

Review `PreparedBrowserAccountReplacement::{transaction,parent_hash,
reservation_id,maximum_fee,signing_digest}`, then explicitly call `sign(&signer)`.
The signer context, current family and exact returned payload are checked before
the signed edge is committed. A confirmed family cannot authorize a new candidate.
Preparation and signing each use revision fences; concurrent mutations reject.

## Same-input Qi candidates

Capture the custody revision before refreshing the current source observations.
Call `BrowserQiSession::prepare_replacement(id, revision, source,
QiReplacementIntent, policy, fees)`. The intent names the parent, distinct parent
output indexes asserted to be owned change, and the replacement change outputs.

All original inputs, all unselected outputs in their original order, and the
complete conversion/wrapping data remain fixed. Only selected owned change may be
reduced. `aggregate_destination` is the one exception: on a conversion or a wrap
it re-decomposes the Quai-ledger destination outputs largest-first, keeping their
address and total value. The node credits that destination with one aggregated
value and charges `ETXGas` per destination output when deciding whether to
include the transaction, so collapsing the shape cuts the gas the fee must cover.
It is rejected for an ordinary transfer, whose recipient outputs stay bound to
the input denominations. The new change value must be strictly lower; both old and new change
require exact same-zone Qi metadata and locally resolved ownership, in addition
to input ownership. Supply these public origins in `QiSource::owners`, including
allocated outputs which do not yet exist as confirmed UTXOs. New change addresses
must already have been allocated and durably burned where fresh allocation is
required by the application.

The portable `qi_replacement::quote_qi_replacement` checks the fixed denomination
constraints, source input values, locks, expiry and canonical age. It estimates
once using the correct ordinary or specialized fee source. Insufficient fee
rejects without rewriting outputs. `QiFeeMode::Explicit(qits)` must equal the
planned input-minus-output fee; it is not presented as a node estimate. The
standalone quote returns `required_owners`; callers must prove ownership before
authorizing it, as the browser session does.

Review `PreparedBrowserQiReplacement::{transaction,parent_hash,reservation_id,
fee,signing_digest}`, then call `sign(&resolver)`. The original journal revalidates
claims and ownership before persisting the signed edge. Repeating signing for an
identical parent and transaction returns the already persisted bytes before
resolving keys or generating randomness. This matters for MuSig: signing the same
unsigned payload again could otherwise create a different transaction hash.

Both ledgers retain at most 32 replacement edges in addition to the root. Failed
quotes/signing/submission keep all prior candidates. Canonical reconciliation can
select any member as the winner, including the parent, and reorgs invalidate only
observations. Origin success is separate from destination settlement.

[Account tests](../crates/quai-sdk/tests/account_preflight.rs) and
[Qi tests](../crates/quai-sdk/tests/qi_preflight.rs) cover exact retained fields,
nonce admission, wrong selected outputs, changed heads, fee bounds, specialized
data, local change ownership, randomized-signature replay and restart after
ambiguous submission in actual Chromium. RPC results are synthetic and do not
establish real node pool replacement behavior.
