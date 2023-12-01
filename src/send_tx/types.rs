use crate::{kms::Account, util::*};
use cita_tool::client::basic::STORE_ADDRESS;
use color_eyre::eyre::Result;
use ethabi::ethereum_types::U256;
use hex::ToHex;
use serde::{Deserialize, Serialize};
use serde_json::{json, value::Value};
use std::fmt::Display;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxData {
    pub to: Option<Vec<u8>>,
    pub data: Vec<u8>,
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
        let display_value = display_value(&value_str).unwrap();

        write!(
            f,
            "to: {}, data: {}, value: {}",
            add_0x(hex::encode(self.to.as_ref().unwrap_or(&Vec::default()))),
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
        let to = parse_data(to).map(|v| if v.is_empty() { None } else { Some(v) })?;
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
        if self.to.is_none() {
            TxType::Create
        } else if self
            .to
            .as_ref()
            .unwrap_or(&Vec::default())
            .encode_hex::<String>()
            == remove_quotes_and_0x(STORE_ADDRESS)
        {
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
pub struct HashToCheck {
    pub hash: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Status {
    Unsend,
    Uncheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseData {
    pub request_key: String,
    pub chain_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendData {
    pub account: Account,
    pub tx_data: TxData,
}

pub struct InitTask {
    pub base_data: BaseData,
    pub send_data: SendData,
    pub timeout: u32,
}

pub struct SendTask {
    pub base_data: BaseData,
    pub send_data: SendData,
    pub timeout: Timeout,
    pub gas: Gas,
}

pub struct CheckTask {
    pub base_data: BaseData,
    pub hash_to_check: HashToCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoTxResult {
    SuccessInfo(SuccessInfo),
    FailedInfo(FailedInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessInfo {
    hash: String,
    contract_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedInfo {
    hash: Option<String>,
    info: String,
}

impl AutoTxResult {
    pub const fn success(hash: String, contract_address: Option<String>) -> Self {
        Self::SuccessInfo(SuccessInfo {
            hash,
            contract_address,
        })
    }

    pub const fn failed(hash: Option<String>, info: String) -> Self {
        Self::FailedInfo(FailedInfo { hash, info })
    }

    pub fn to_json(&self) -> Value {
        match self {
            AutoTxResult::SuccessInfo(s) => match s.contract_address.clone() {
                Some(addr) => json!({
                    "is_success": true,
                    "onchain_hash": add_0x(s.hash.clone()),
                    "contract_address": add_0x(addr),
                }),
                None => json!({
                    "is_success": true,
                    "onchain_hash": add_0x(s.hash.clone()),
                }),
            },
            AutoTxResult::FailedInfo(f) => match f.hash.clone() {
                Some(hash) => json!({
                    "is_success": false,
                    "last_hash": add_0x(hash),
                    "err": f.info,
                }),
                None => json!({
                    "is_success": false,
                    "err": f.info,
                }),
            },
        }
    }
}
