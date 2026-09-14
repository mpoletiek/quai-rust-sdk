//! Explicit WQI and WQUAI contract intents. No implicit signing or submission.
use crate::contracts::{Contract, ContractCall, ContractError, Erc20};
use quai_abi::AbiInterface;
use quai_primitives::{Hash32, QiAddress, QuaiAddress};
use quai_provider::{BlockTag, ContractCodeObservation, Provider};
use quai_rpc::{Transport, U256};
use serde_json::json;

/// User-confirmed WQI deployment on mainnet and Orchard Cyprus-1.
pub const WQI_ADDRESS: &str = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750";
/// Mainnet WQUAI deployment in Cyprus-1. Verify code before a funded workflow.
pub const WQUAI_MAINNET_ADDRESS: &str = "0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB";
/// Orchard WQUAI deployment, verified by code and token metadata on 2026-09-14.
pub const WQUAI_ORCHARD_ADDRESS: &str = "0x005c46f661Baef20671943f2b4c087Df3E7CEb13";
/// Compatibility alias for the mainnet deployment; Orchard uses
/// [`WQUAI_ORCHARD_ADDRESS`]. A constant does not attest deployment availability.
pub const WQUAI_ADDRESS: &str = WQUAI_MAINNET_ADDRESS;
/// WQI has 18 token decimals while native Qi has 3: one Qit = 10^15 token atoms.
pub const WQI_ATOMS_PER_QIT: u64 = 1_000_000_000_000_000;

/// Pinned go-quai v0.56.0 redemption output accounting, independent of contract
/// balance or node execution. Small denominations are discarded by that profile.
#[derive(Clone, Debug)]
pub struct QiRedemptionPlan {
    /// Exact surviving output denominations, largest first.
    pub outputs: Vec<quai_consensus::Denomination>,
    /// Native Qits lost to the profile's denomination trim threshold.
    pub discarded_qits: U256,
    /// Minimum destination ETX gas for all surviving outputs (9000 each).
    pub minimum_etx_gas: u64,
}
impl QiRedemptionPlan {
    /// Account for up to 1024 outputs under the pinned profile. No fork lock is
    /// guessed: use observed outpoint locks after destination execution.
    pub fn go_quai_v056(qits: U256) -> Result<Self, ContractError> {
        if qits == U256::ZERO {
            return Err(ContractError::InvalidResult);
        }
        let mut remaining = qits;
        let mut outputs = Vec::new();
        for index in (6..=14).rev() {
            let denomination = quai_consensus::Denomination::new(index)
                .map_err(|_| ContractError::InvalidResult)?;
            let unit = U256::from(denomination.value());
            let count = remaining / unit;
            if count > U256::from(1024 - outputs.len()) {
                return Err(ContractError::InvalidResult);
            }
            outputs.extend(std::iter::repeat_n(denomination, count.to::<usize>()));
            remaining %= unit;
        }
        Ok(Self {
            minimum_etx_gas: outputs.len() as u64 * 9000,
            outputs,
            discarded_qits: remaining,
        })
    }
}

/// Convert exact Qits to WQI token atoms, rejecting overflow.
pub fn qits_to_wqi_atoms(qits: U256) -> Result<U256, ContractError> {
    qits.checked_mul(U256::from(WQI_ATOMS_PER_QIT))
        .ok_or(ContractError::InvalidResult)
}
/// Convert WQI token atoms to Qits, rejecting fractional-Qit truncation.
pub fn wqi_atoms_to_qits(atoms: U256) -> Result<U256, ContractError> {
    let scale = U256::from(WQI_ATOMS_PER_QIT);
    if atoms % scale != U256::ZERO {
        return Err(ContractError::InvalidResult);
    }
    Ok(atoms / scale)
}

const WQUAI_ABI: &str = r#"[
 {"type":"function","name":"deposit","stateMutability":"payable","inputs":[],"outputs":[]},
 {"type":"function","name":"withdraw","stateMutability":"nonpayable","inputs":[{"name":"amount","type":"uint256"}],"outputs":[]}
]"#;
const WQI_ABI: &str = r#"[
 {"type":"function","name":"claimDeposit","stateMutability":"nonpayable","inputs":[],"outputs":[{"type":"uint256"}]},
 {"type":"function","name":"unwrapQi","stateMutability":"nonpayable","inputs":[{"name":"beneficiary","type":"address"},{"name":"amount","type":"uint256"},{"name":"etxGas","type":"uint64"}],"outputs":[]}
]"#;

/// Wrapped native Quai contract, using deposit()/withdraw(uint256).
pub struct WrappedQuai<'a, T> {
    contract: Contract<'a, T>,
    provider: &'a Provider<T>,
}
impl<'a, T: Transport> WrappedQuai<'a, T> {
    /// Bind only after checking nonempty code, trusted genesis and optional runtime
    /// Keccak at a rechecked mined block. Retain the returned observation's scope
    /// and block; this does not prove ABI/proxy semantics or future availability.
    pub async fn new_verified(
        address: QuaiAddress,
        provider: &'a Provider<T>,
        expected_genesis: Hash32,
        expected_runtime: Option<Hash32>,
        block: BlockTag,
    ) -> Result<(Self, ContractCodeObservation), ContractError> {
        let wrapper = Self::new(address, provider)?;
        let observation = wrapper
            .contract
            .verify_deployment(expected_genesis, expected_runtime, block)
            .await?;
        Ok((wrapper, observation))
    }
    /// Bind an explicitly selected deployment; verify its network/code separately.
    pub fn new(address: QuaiAddress, provider: &'a Provider<T>) -> Result<Self, ContractError> {
        Ok(Self {
            contract: Contract::new(
                address,
                AbiInterface::from_json(WQUAI_ABI.as_bytes())?,
                provider,
            ),
            provider,
        })
    }
    /// Prepare a positive native deposit in Its; the call value equals the amount.
    pub fn deposit(&self, its: U256) -> Result<ContractCall, ContractError> {
        if its == U256::ZERO {
            return Err(ContractError::InvalidResult);
        }
        self.contract.prepare("deposit", &[], its)
    }
    /// Prepare withdrawal of exact wrapped atoms with zero native call value.
    pub fn withdraw(&self, atoms: U256) -> Result<ContractCall, ContractError> {
        if atoms == U256::ZERO {
            return Err(ContractError::InvalidResult);
        }
        self.contract
            .prepare("withdraw", &[json!(atoms.to_string())], U256::ZERO)
    }
    /// Balance, allowance, transfer and approval operations for the same contract.
    pub fn token(&self) -> Result<Erc20<'a, T>, ContractError> {
        Erc20::new(self.contract.address(), self.provider)
    }
}

/// Wrapped Qi contract, using claimDeposit()/unwrapQi(address,uint256,uint64).
/// Wrapping itself starts with `QiWrappingTransaction` on the native Qi ledger.
pub struct WrappedQi<'a, T> {
    contract: Contract<'a, T>,
    provider: &'a Provider<T>,
}
impl<'a, T: Transport> WrappedQi<'a, T> {
    /// Bind only after checking nonempty code, trusted genesis and optional runtime
    /// Keccak at a rechecked mined block. Native Qi wrapping remains a separate
    /// reviewed protocol operation; code presence does not qualify its execution.
    pub async fn new_verified(
        address: QuaiAddress,
        provider: &'a Provider<T>,
        expected_genesis: Hash32,
        expected_runtime: Option<Hash32>,
        block: BlockTag,
    ) -> Result<(Self, ContractCodeObservation), ContractError> {
        let wrapper = Self::new(address, provider)?;
        let observation = wrapper
            .contract
            .verify_deployment(expected_genesis, expected_runtime, block)
            .await?;
        Ok((wrapper, observation))
    }
    /// Bind an explicitly selected deployment; no node calls occur here.
    pub fn new(address: QuaiAddress, provider: &'a Provider<T>) -> Result<Self, ContractError> {
        Ok(Self {
            contract: Contract::new(
                address,
                AbiInterface::from_json(WQI_ABI.as_bytes())?,
                provider,
            ),
            provider,
        })
    }
    /// Claim the signer's unclaimed protocol backing. The contract determines
    /// the amount at execution; query/simulate before authorizing this intent.
    pub fn claim_deposit(&self) -> Result<ContractCall, ContractError> {
        self.protocol_call(self.contract.prepare("claimDeposit", &[], U256::ZERO)?)
    }
    /// Prepare redemption to a fresh same-zone Qi address. Input is Qits, converted
    /// exactly to WQI atoms. ETX gas is explicit and is not the origin gas limit.
    /// Rejects amounts with trimmed denominations and insufficient ETX output gas
    /// under go-quai v0.56.0. Retains the lockup access declaration. A receipt
    /// still does not prove destination credit.
    pub fn unwrap(
        &self,
        beneficiary: QiAddress,
        qits: U256,
        etx_gas: u64,
    ) -> Result<ContractCall, ContractError> {
        if beneficiary.zone() != self.contract.address().zone() {
            return Err(ContractError::ZoneMismatch);
        }
        if qits == U256::ZERO || etx_gas == 0 {
            return Err(ContractError::InvalidResult);
        }
        let plan = QiRedemptionPlan::go_quai_v056(qits)?;
        if plan.discarded_qits != U256::ZERO || etx_gas < plan.minimum_etx_gas {
            return Err(ContractError::InvalidResult);
        }
        self.protocol_call(self.contract.prepare(
            "unwrapQi",
            &[
                json!(beneficiary.to_string()),
                json!(qits_to_wqi_atoms(qits)?.to_string()),
                json!(etx_gas.to_string()),
            ],
            U256::ZERO,
        )?)
    }
    fn protocol_call(&self, call: ContractCall) -> Result<ContractCall, ContractError> {
        let mut bytes = [0; 20];
        bytes[0] = self.contract.address().zone().byte();
        bytes[19] = 0x0a;
        call.with_access_list(vec![quai_consensus::AccessTuple {
            address: quai_primitives::Address::from_bytes(bytes),
            storage_keys: vec![],
        }])
    }
    /// Observe unclaimed protocol backing in native Qits at an explicit selector.
    /// The pinned node's exact absent-deposit response maps to zero; unrelated
    /// transport, protocol and state errors remain failures.
    pub async fn unclaimed(
        &self,
        beneficiary: QuaiAddress,
        block: BlockTag,
    ) -> Result<U256, ContractError> {
        Ok(self
            .provider
            .wrapped_qi_deposit_optional(self.contract.address(), beneficiary, block)
            .await?
            .unwrap_or(U256::ZERO))
    }
    /// ERC-20 operations on wrapped token atoms, not native Qits.
    pub fn token(&self) -> Result<Erc20<'a, T>, ContractError> {
        Erc20::new(self.contract.address(), self.provider)
    }
}
