//! Pelagus-compatible payment-channel mailbox.
//!
//! Pelagus announces BIP47 channels by calling `notify(sender, receiver)` on a
//! Quai-ledger contract rather than sending BIP47 notification transactions;
//! receivers read `getNotifications(receiver)` and open channels for returned
//! sender codes. This is a wallet convention, not a Quai protocol rule or a
//! quais.js feature. Announcements are unauthenticated public data: anyone can
//! announce any code to any receiver, and an announcement reveals that two codes
//! were linked. Validate each returned code and keep channel scanning bounded.
use crate::contracts::{Contract, ContractCall, ContractError};
use quai_abi::AbiInterface;
use quai_payments::PaymentCode;
use quai_primitives::QuaiAddress;
use quai_provider::{BlockTag, LogFilter, LogRange, Provider, ProviderError, TopicMatch};
use quai_rpc::{RpcError, Transport, U256};
use serde_json::Value;

/// Mailbox used by Pelagus (`pelagus-extension` 8c8a044, `MAILBOX_CONTRACT_ADDRESS`).
/// Identical runtime bytes were observed at this address on mainnet and Orchard
/// on 2026-09-14 (SHA-256 `475892f0…3051691d`). Verify deployments before use.
pub const PELAGUS_MAILBOX_ADDRESS: &str = "0x004C82298b3ED69a949008d7037918B13A4260c5";

/// Most blocks one `NotificationSent` log request covers: the provider's limit.
/// [`PaymentMailbox::notifications_in_blocks`] halves a request that exceeds
/// the transport's response limit, so a range dense with announcements still
/// reads.
pub const MAILBOX_LOG_BLOCKS: u64 = 10_000;

/// Blocks the end of a log range must sit below the tip. Zone reorgs are
/// usually one or two blocks deep, and a load-balanced backend may lag a few
/// blocks; below this depth, neither can change or hide the range.
pub const MAILBOX_SETTLED_DEPTH: u64 = 16;

/// Longest invalid announcement kept verbatim; the rest is cut. A valid code
/// is at most 120 characters.
const INVALID_ENTRY_CHARS: usize = 128;

/// Undecodable logs kept as diagnostics per read. They cannot be tied to a
/// receiver, so they are not counted toward the read's size cap: otherwise
/// one sender's junk could fail every wallet's read of that range.
const UNDECODABLE_KEPT: usize = 16;

/// Maximum announcements accepted from one read; larger results fail explicitly.
///
/// Announcements cost only a zero-value transaction, so a low bound let anyone
/// disable a receiver's discovery for good by announcing past it. This one sits
/// above what the default 2 MiB response limit can carry, so the response
/// limit binds first: each ABI-encoded code string takes about 192 bytes, 384
/// as hex, so about 5,400 fit. Discovery pages through the list,
/// so its length costs parsing, not scanning.
pub const MAX_MAILBOX_NOTIFICATIONS: usize = 32_768;

const MAILBOX_ABI: &str = r#"[
{"type":"function","name":"notify","stateMutability":"nonpayable","inputs":[{"name":"senderPaymentCode","type":"string"},{"name":"receiverPaymentCode","type":"string"}],"outputs":[]},
{"type":"function","name":"getNotifications","stateMutability":"view","inputs":[{"name":"receiverPaymentCode","type":"string"}],"outputs":[{"name":"","type":"string[]"}]},
{"type":"event","name":"NotificationSent","anonymous":false,"inputs":[{"name":"senderPaymentCode","type":"string","indexed":false},{"name":"receiverPaymentCode","type":"string","indexed":false}]}
]"#;

/// Announcements for one receiver, split into valid distinct codes and rejected entries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MailboxNotifications {
    /// Distinct valid sender codes in first-announcement order. Not proof of payment.
    pub senders: Vec<PaymentCode>,
    /// Entries that are not valid payment codes, retained for diagnostics.
    pub invalid: Vec<String>,
    /// Repeated valid announcements that were collapsed.
    pub duplicates: usize,
}

/// Typed mailbox binding. Reads are trusted-node observations; writes are explicit
/// account intents for review, signing and submission through a wallet session.
pub struct PaymentMailbox<'a, T> {
    contract: Contract<'a, T>,
    provider: &'a Provider<T>,
}

impl<'a, T: Transport> PaymentMailbox<'a, T> {
    /// Bind an explicitly selected mailbox without network I/O.
    pub fn new(address: QuaiAddress, provider: &'a Provider<T>) -> Result<Self, ContractError> {
        Ok(Self {
            contract: Contract::new(
                address,
                AbiInterface::from_json(MAILBOX_ABI.as_bytes())?,
                provider,
            ),
            provider,
        })
    }

    /// Bound contract.
    pub fn contract(&self) -> &Contract<'a, T> {
        &self.contract
    }

    /// Prepare `notify(sender, receiver)` with zero value. Sending it publicly links
    /// both codes on-chain; call only after the application decides to announce.
    pub fn notify(
        &self,
        sender: &PaymentCode,
        receiver: &PaymentCode,
    ) -> Result<ContractCall, ContractError> {
        self.contract.prepare(
            "notify",
            &[
                Value::String(sender.to_base58()),
                Value::String(receiver.to_base58()),
            ],
            U256::ZERO,
        )
    }

    /// Read and validate announcements for `receiver`. `caller` must be an account
    /// in the mailbox zone; the pinned node rejects calls from the contract itself.
    pub async fn notifications(
        &self,
        caller: QuaiAddress,
        receiver: &PaymentCode,
        block: BlockTag,
    ) -> Result<MailboxNotifications, ContractError> {
        let result = self
            .contract
            .call(
                caller,
                "getNotifications",
                &[Value::String(receiver.to_base58())],
                block,
            )
            .await?;
        let [Value::Array(entries)] = result.as_slice() else {
            return Err(ContractError::InvalidResult);
        };
        if entries.len() > MAX_MAILBOX_NOTIFICATIONS {
            return Err(ContractError::InvalidResult);
        }
        let mut out = Collector::default();
        for entry in entries {
            out.add(entry.as_str().ok_or(ContractError::InvalidResult)?);
        }
        Ok(out.notifications)
    }

    /// Announcements to `receiver` from `NotificationSent` logs in the
    /// inclusive block range `from..=to`, in announcement order.
    ///
    /// Unlike [`Self::notifications`], no single response grows with the
    /// mailbox. The range is read in requests of at most [`MAILBOX_LOG_BLOCKS`]
    /// blocks, halved while a response exceeds the transport's limit, so
    /// announcement spam can slow a read but cannot disable it. A log that does
    /// not decode is listed in `invalid` and skipped, never failing the read.
    /// The event indexes nothing, so the node returns every announcement in the
    /// range and the receiver is matched here: the query reveals nothing about
    /// which receiver is reading.
    ///
    /// `to` must be at least [`MAILBOX_SETTLED_DEPTH`] blocks below the tip
    /// (`ProviderError::InvalidRequest` otherwise), and its hash must be the
    /// same after the read as before (`ProviderError::ObservationChanged`
    /// otherwise; retry). A read that returns `Ok` covered a settled range, so
    /// the caller can persist `to + 1` as the next range's start. More than
    /// [`MAX_MAILBOX_NOTIFICATIONS`] entries in one read fail with
    /// `InvalidResult`; read a narrower range.
    pub async fn notifications_in_blocks(
        &self,
        receiver: &PaymentCode,
        from: u64,
        to: u64,
    ) -> Result<MailboxNotifications, ContractError> {
        if from > to {
            return Err(ProviderError::InvalidRequest("mailbox log range is empty").into());
        }
        let zone = self.contract.address().zone();
        let settled = |headers: Vec<Option<quai_provider::ZoneHeader>>| match headers.as_slice() {
            [Some(tip), Some(end)] => Ok((tip.number, end.hash)),
            _ => Err(ProviderError::ObservationChanged),
        };
        let at_end = [BlockTag::Latest, BlockTag::Number(U256::from(to))];
        let (tip, anchor) = settled(self.provider.headers(zone, &at_end).await?)?;
        if tip < to.saturating_add(MAILBOX_SETTLED_DEPTH) {
            return Err(ProviderError::InvalidRequest("mailbox log range is not settled").into());
        }
        let event = self.contract.interface().event("NotificationSent")?;
        let mut out = Collector::default();
        let (mut next, mut width) = (from, MAILBOX_LOG_BLOCKS);
        loop {
            let end = to.min(next.saturating_add(width - 1));
            let filter = LogFilter {
                zone,
                range: LogRange::Inclusive {
                    from: next,
                    to: end,
                },
                addresses: vec![self.contract.address().address()],
                topics: vec![TopicMatch::Exact(event.topic_hash())],
            };
            let logs = match self.provider.logs(&filter).await {
                Ok(logs) => logs,
                Err(ProviderError::Rpc(RpcError::ResponseTooLarge)) if end > next => {
                    width = (end - next).div_ceil(2);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for log in logs.iter().filter(|log| !log.removed) {
                // Each log is decoded alone: anyone can emit one that does not
                // decode, such as a non-UTF-8 string, and it must not hide the rest.
                let values = event.decode_log(&log.topics, log.data.bytes());
                let strings = values.as_deref().ok().and_then(|values| match values {
                    [
                        quai_abi::AbiEventValue::Value(Value::String(sender)),
                        quai_abi::AbiEventValue::Value(Value::String(to_receiver)),
                    ] => Some((sender, to_receiver)),
                    _ => None,
                });
                match strings {
                    Some((sender, to_receiver)) => {
                        if PaymentCode::from_base58(to_receiver).is_ok_and(|c| c == *receiver) {
                            out.add(sender);
                        }
                    }
                    None => out.undecodable(log.transaction_hash, log.log_index),
                }
                if out.len() > MAX_MAILBOX_NOTIFICATIONS {
                    return Err(ContractError::InvalidResult);
                }
            }
            if end == to {
                break;
            }
            next = end + 1;
            // Grow back after a dense stretch rather than retrying the full
            // width, which would repeat every halving for each chunk.
            width = width.saturating_mul(2).min(MAILBOX_LOG_BLOCKS);
        }
        let (_, after) = settled(self.provider.headers(zone, &at_end).await?)?;
        if after != anchor {
            return Err(ProviderError::ObservationChanged.into());
        }
        Ok(out.notifications)
    }

    /// Whether `sender` is already announced to `receiver` (Pelagus's
    /// `doesChannelExistForReceiver` check), avoiding a redundant notify transaction.
    pub async fn is_notified(
        &self,
        caller: QuaiAddress,
        sender: &PaymentCode,
        receiver: &PaymentCode,
        block: BlockTag,
    ) -> Result<bool, ContractError> {
        Ok(self
            .notifications(caller, receiver, block)
            .await?
            .senders
            .contains(sender))
    }
}

/// Validates and de-duplicates announced sender codes in order.
#[derive(Default)]
struct Collector {
    notifications: MailboxNotifications,
    seen: std::collections::HashSet<String>,
    undecodable: usize,
}
impl Collector {
    fn add(&mut self, text: &str) {
        match PaymentCode::from_base58(text) {
            Ok(code) if !self.seen.insert(code.to_base58()) => self.notifications.duplicates += 1,
            Ok(code) => self.notifications.senders.push(code),
            Err(_) => self
                .notifications
                .invalid
                .push(text.chars().take(INVALID_ENTRY_CHARS).collect()),
        }
    }
    fn undecodable(&mut self, transaction: quai_primitives::Hash32, index: u64) {
        if self.undecodable < UNDECODABLE_KEPT {
            self.notifications
                .invalid
                .push(format!("undecodable log {transaction}:{index}"));
        }
        self.undecodable += 1;
    }
    fn len(&self) -> usize {
        let n = &self.notifications;
        n.senders.len() + n.invalid.len() + n.duplicates - self.undecodable.min(UNDECODABLE_KEPT)
    }
}
