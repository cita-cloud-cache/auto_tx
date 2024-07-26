use crate::{kms::Account, util::*};
use cita_tool::client::basic::STORE_ADDRESS;
use color_eyre::eyre::Result;
use ethabi::ethereum_types::U256;
use hex::ToHex;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use std::fmt::Display;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxData {
    #[serde(with = "crate::util::bytes_hex")]
    pub to: Vec<u8>,
    #[serde(with = "crate::util::bytes_hex")]
    pub data: Vec<u8>,
    #[serde(with = "crate::util::value_hex")]
    pub value: (Vec<u8>, U256),
}

impl Display for TxData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data_len = self.data.len();
        let data = if data_len > 10 {
            add_0x(hex::encode(&self.data.clone()[..4]))
                + "..."
                + &hex::encode(&self.data.clone()[(data_len - 4)..data_len])
        } else {
            add_0x(hex::encode(self.data.clone()))
        };

        let value_str = hex::encode(self.value.0.clone());
        let display_value = display_value(&value_str).unwrap_or_default();

        write!(
            f,
            "to: {}, data: {}, value: {}",
            add_0x(hex::encode(self.to.clone())),
            data,
            display_value,
        )
    }
}

#[derive(Clone, Copy)]
pub enum TxType {
    Create,
    Store,
    Normal,
}

impl TxData {
    pub fn new(to: &str, data: &str, value: &str) -> Result<Self> {
        let to = parse_data(to).unwrap_or_default();
        let data = parse_data(data)?;
        let value_u256 = U256::from_dec_str(value)?;
        let value = parse_value(value)?;
        Ok(Self {
            to,
            data,
            value: (value, value_u256),
        })
    }

    pub fn tx_type(&self) -> TxType {
        if self.to.is_empty() {
            TxType::Create
        } else if self.to.encode_hex::<String>() == remove_quotes_and_0x(STORE_ADDRESS) {
            TxType::Store
        } else {
            TxType::Normal
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Timeout {
    Cita(CitaTimeout),
    Eth(EthTimeout),
}
impl Display for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Timeout::Cita(timeout) => write!(
                f,
                "remain_time: {}, vnb: {}",
                timeout.remain_time, timeout.valid_until_block
            ),
            Timeout::Eth(timeout) => write!(f, "nonce: {}", timeout.nonce),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CitaTimeout {
    pub remain_time: u32,
    pub valid_until_block: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EthTimeout {
    pub nonce: U256,
}

impl Timeout {
    pub fn get_cita_timeout(&self) -> CitaTimeout {
        match self {
            Timeout::Cita(timeout) => *timeout,
            Timeout::Eth(_) => unreachable!(),
        }
    }

    pub fn get_eth_timeout(&self) -> EthTimeout {
        match self {
            Timeout::Cita(_) => unreachable!(),
            Timeout::Eth(timeout) => *timeout,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Gas {
    pub gas: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawTransactionBytes {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashToCheck {
    #[serde(with = "crate::util::bytes_hex")]
    pub hash: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Status {
    Unsend,
    Uncheck,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseData {
    pub request_key: String,
    pub chain_name: String,
    pub account: Account,
    pub tx_data: TxData,
}

#[derive(Debug)]
pub struct InitTaskParam {
    pub base_data: BaseData,
    pub timeout: u32,
    pub gas: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub init_hash: String,

    #[serde(flatten)]
    pub base_data: BaseData,

    pub status: Status,
    pub timeout: Timeout,
    pub gas: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_to_check: Option<HashToCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTask {
    pub base_data: BaseData,
    pub timeout: Timeout,
    pub gas: Gas,
    pub raw_transaction_bytes: Option<RawTransactionBytes>,
}

#[derive(Debug)]
pub struct CheckTask {
    pub base_data: BaseData,
    pub hash_to_check: HashToCheck,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskResult {
    is_success: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    onchain_hash: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    contract_address: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    err: String,
}

impl TaskResult {
    pub fn success(onchain_hash: String, contract_address: Option<String>) -> Self {
        Self {
            is_success: true,
            onchain_hash,
            contract_address: contract_address.unwrap_or_default(),
            err: String::default(),
        }
    }

    pub fn failed(onchain_hash: Option<String>, err: String) -> Self {
        Self {
            is_success: false,
            onchain_hash: onchain_hash.unwrap_or_default(),
            err,
            contract_address: String::default(),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}
