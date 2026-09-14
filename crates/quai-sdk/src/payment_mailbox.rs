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
use quai_provider::{BlockTag, Provider};
use quai_rpc::{Transport, U256};
use serde_json::Value;

/// Mailbox used by Pelagus (`pelagus-extension` 8c8a044, `MAILBOX_CONTRACT_ADDRESS`).
/// Identical runtime bytes were observed at this address on mainnet and Orchard
/// on 2026-09-14 (SHA-256 `475892f0…3051691d`). Verify deployments before use.
pub const PELAGUS_MAILBOX_ADDRESS: &str = "0x004C82298b3ED69a949008d7037918B13A4260c5";

/// Maximum announcements accepted from one read; larger results fail explicitly.
pub const MAX_MAILBOX_NOTIFICATIONS: usize = 4096;

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
        let mut out = MailboxNotifications::default();
        for entry in entries {
            let text = entry.as_str().ok_or(ContractError::InvalidResult)?;
            match PaymentCode::from_base58(text) {
                Ok(code) if out.senders.contains(&code) => out.duplicates += 1,
                Ok(code) => out.senders.push(code),
                Err(_) => out.invalid.push(text.to_owned()),
            }
        }
        Ok(out)
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
