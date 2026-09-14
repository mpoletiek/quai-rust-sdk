# Wordlist and custom mnemonic parity

The published `quais@1.0.0-alpha.57` dictionaries, OWL/OWL-A compression and
mnemonic utilities have explicit Rust equivalents in `quai_wallet::wordlist`.
All ten built-in dictionaries retain exactly 2,048 words in published index order.

| Published API | Rust equivalent |
| --- | --- |
| `wordlists`, `LangEn.wordlist`, `LangEs.wordlist` | `Wordlist::builtin(Language)` borrows compiled word tables |
| `Wordlist` subclass | `Wordlist::from_words(locale, words, style)` validates an owned public dictionary |
| `WordlistOwl`, `WordlistOwlA` | `Wordlist::from_owl(locale, data, accents, checksum)` |
| `locale`, `getWord`, `getWordIndex` | `locale`, checked `get_word`, exact `get_word_index` |
| `split`, `join` | Explicit `WordlistStyle`, redacted zeroizing `SecretWords` / `SecretString` |
| `_data`, `_accent` | `owl_data`, `accent_data` retain imported encodings; compiled built-ins return None |
| `_decodeWords` | `words` iterates the validated dictionary; no unchecked checksum bypass |
| Mnemonic custom wordlist constructors | `CustomMnemonic::parse`, `from_entropy` borrow the exact dictionary |
| Mnemonic phrase, entropy, seed, wordlist | `phrase`, `entropy`, `to_seed`, `wordlist` |

## Dictionary validation and phrase conventions

Dictionary imports require 1–2,048 unique, nonempty NFKD words, at most 128 bytes
each, with no whitespace/control characters. Locale tags contain 1–32 ASCII
letters, digits, underscores or hyphens. General-style entries must already be
lowercase; Chinese-style entries must be single Unicode scalars. These checks
ensure generated phrases can be parsed under their selected policy. BIP39
conversion requires exactly 2,048 entries; locale names do not authenticate a
custom dictionary or choose its splitting rules.

General splitting lowercases and follows ECMAScript whitespace. Japanese splits
ASCII/ideographic spaces and joins with ideographic spaces. Chinese removes
ASCII/ideographic spaces and splits Unicode scalar characters. General/Japanese
split preserve leading/trailing empty fields, matching the source's regex split.
Rust never creates unpaired UTF-16 surrogates. `join` applies the separator but
does not validate membership or mnemonic checksums.

Phrases, joins and passphrases are bounded to 4,096 bytes; checks apply before and
after relevant normalization. `SecretWords::expose` and `SecretString::expose`
explicitly borrow secret text. These values redact Debug and erase owned storage
on drop. Caller copies and compiler/backend transient copies are outside those
guarantees. Public dictionaries and their compressed encodings are not secrets.

OWL imports cap encoded data and accent data at 65,536 bytes individually and
expanded intermediates at 1 MiB. Accent widths must be 1–9, overlays at most 32,
and combining marks in U+0300–U+036F. Malformed tokens, unsupported code points,
excess/unconsumed accent positions and expansion bombs reject. The required
Keccak checksum covers newline-joined words with a final newline. Rust verifies
it eagerly; the published `_decodeWords` bypasses the lazy lookup checksum.
Compiled built-ins use tables instead of retaining a second compressed copy.

## Custom mnemonic identity and recovery

`CustomMnemonic` validates standard 16/20/24/28/32-byte entropy and
12/15/18/21/24-word phrases. It uses word indexes for the BIP39 checksum, then the
selected dictionary's canonical phrase for PBKDF2-HMAC-SHA512 (2,048 rounds) with
NFKD normalization and an explicit passphrase. A custom dictionary changes seed
identity even when entropy is unchanged. Preserve the exact dictionary and
passphrase when recovering from a phrase.

Feed the guarded result of `to_seed` explicitly to `HdWallet::from_seed`.
`full_backup::BackupOrigin::from_seed` can preserve the effective seed in an
authenticated backup; that restores key identity without reconstructing the
original custom dictionary or phrase. It does not silently store custom words
inside the built-in mnemonic backup format. Passphrases are not retained fields.

The existing `Mnemonic` continues to support all ten built-in languages and its
more permissive surrounding/repeated whitespace normalization. It now accepts
unseparated Chinese phrases. Entropy export packs the stored word indexes
without re-detecting language: the backend's entropy helper could panic when a
valid explicitly selected Chinese phrase also matched the other Chinese list.
Both mnemonic types avoid that ambiguity.

## Evidence

[Source fixtures](../compatibility/fixtures/wordlists.json) retain all 20,480
words, six compressed dictionaries, phrase conventions, ten built-in seed
vectors and five custom dictionary seed vectors spanning all entropy lengths.
[Source regressions](../compatibility/scripts/wordlists.test.mjs) check exact
index order, compressed overlays, Chinese recovery and lazy checksum behavior.
[Five shared native/worker tests](../crates/quai-wallet/tests/wordlists.rs)
exercise all vectors, HD seed identity, malformed/bounded imports, redacted
outputs and 150 built-in entropy roundtrips across ten languages and five sizes.
Sanitizer fuzzing additionally checks compressed imports, checksum/word-index
identity and Chinese/custom mnemonic parsing without attacker-selected KDF work.
