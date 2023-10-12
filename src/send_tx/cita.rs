use super::AutoTx;
use super::AutoTxInfo;
use super::AutoTxTag;
use super::AutoTxType;
use crate::chains::ChainType;
use crate::chains::Chains;
use crate::kms::Kms;
use crate::util::add_0x;
use crate::util::remove_0x;
use crate::AutoTxGlobalState;
use anyhow::anyhow;
use anyhow::Result;
use cita_tool::client::basic::{Client, ClientExt};
use cita_tool::Crypto;
use cita_tool::LowerHex;
use cita_tool::ParamsValue;
use cita_tool::ProtoMessage;
use cita_tool::ResponseValue;
use cita_tool::Transaction as CitaTransaction;
use cita_tool::UnverifiedTransaction;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const CITA_BLOCK_INTERVAL: u64 = 3;
const CITA_BLOCK_LIMIT: u64 = 88;

#[derive(Clone)]
pub struct CitaClient {
    pub client: Client,
}

impl CitaClient {
    pub fn new(url: &str) -> Result<Self> {
        let client = Client::new();
        let client = client.set_uri(url);
        Ok(Self { client })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CitaTransactionForSerde {
    pub data: Vec<u8>,
    pub value: Vec<u8>,
    pub nonce: String,
    pub quota: u64,
    pub valid_until_block: u64,
    pub version: u32,
    pub to: String,
    pub to_v1: Vec<u8>,
    pub chain_id: u32,
    pub chain_id_v1: Vec<u8>,
}

impl From<CitaTransactionForSerde> for CitaTransaction {
    fn from(value: CitaTransactionForSerde) -> Self {
        CitaTransaction {
            data: value.data,
            value: value.value,
            nonce: value.nonce,
            quota: value.quota,
            valid_until_block: value.valid_until_block,
            version: value.version,
            to: value.to,
            to_v1: value.to_v1,
            chain_id: value.chain_id,
            chain_id_v1: value.chain_id_v1,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CitaSigned {
    pub hash: Vec<u8>,
    pub unverified: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CitaAutoTx {
    auto_tx_info: AutoTxInfo,
    remain_time: u32,
    tx: CitaTransactionForSerde,
    hash: CitaSigned,
    tag: AutoTxTag,
}

impl CitaAutoTx {
    pub fn new(auto_tx_info: AutoTxInfo, remain_time: u32) -> Self {
        let tx = CitaTransactionForSerde {
            data: auto_tx_info.tx_info.data.clone(),
            value: auto_tx_info.tx_info.value.0.clone(),
            nonce: auto_tx_info.req_key.clone(),
            ..Default::default()
        };
        Self {
            auto_tx_info,
            remain_time,
            tx,
            hash: CitaSigned::default(),
            tag: AutoTxTag::Unsend,
        }
    }

    pub const fn get_remain_time(&self) -> u32 {
        self.remain_time
    }
}

#[axum::async_trait]
impl AutoTx for CitaAutoTx {
    fn get_tag(&self) -> &AutoTxTag {
        &self.tag
    }

    fn set_tag(&mut self, tag: AutoTxTag) {
        self.tag = tag
    }

    fn get_key(&self) -> String {
        self.auto_tx_info.req_key.clone()
    }

    fn get_current_hash(&self) -> String {
        hex::encode(self.hash.clone().hash)
    }

    fn to_unified_type(&self) -> AutoTxType {
        AutoTxType::Cita(self.clone())
    }

    async fn estimate_gas(&mut self, chains: &Chains) -> Result<()> {
        let chain_info = chains.get_chain_info(&self.auto_tx_info.chain_name).await?;
        if let ChainType::Cita(cita_client) = chain_info.chain_type {
            let from_str = add_0x(hex::encode(self.auto_tx_info.tx_info.from.clone()));
            let from = Some(from_str.as_str());
            let to_str = add_0x(hex::encode(self.auto_tx_info.tx_info.to.clone()));
            let to = to_str.as_str();
            let data_str = add_0x(hex::encode(self.auto_tx_info.tx_info.data.clone()));
            let data = Some(data_str.as_str());

            let resp = cita_client
                .client
                .get_block_number()
                .map_err(|_| anyhow!("estimate_gas get_block_number failed"))?;

            if let Some(ResponseValue::Singe(ParamsValue::String(height))) = resp.result() {
                let resp = cita_client
                    .client
                    .estimate_quota(from, to, data, &height)
                    .map_err(|_| anyhow!("estimate_gas estimate_quota failed"))?;
                if let Some(ResponseValue::Singe(ParamsValue::String(quota))) = resp.result() {
                    let quota = u64::from_str_radix(remove_0x(&quota), 16)?;
                    self.tx.quota = quota / 2 * 3;
                }
            }
        }

        Ok(())
    }

    async fn update_args(&mut self, state: &AutoTxGlobalState) -> Result<Option<String>> {
        let chain_info = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await?;
        if let ChainType::Cita(mut cita_client) = chain_info.chain_type {
            let current_height = cita_client
                .client
                .get_current_height()
                .map_err(|_| anyhow!("update_args get_current_height failed"))?;

            // update remain_time
            if current_height > self.tx.valid_until_block && self.tx.valid_until_block != 0 {
                let offset =
                    ((current_height - self.tx.valid_until_block) * CITA_BLOCK_INTERVAL) as u32;
                self.remain_time = if self.remain_time > offset {
                    self.remain_time - offset
                } else {
                    0
                };
            }

            // consume remain_time if timeout
            let is_timeout = self.tx.valid_until_block <= current_height
                || self.tx.valid_until_block > (current_height + CITA_BLOCK_LIMIT);
            let has_remain_time = self.remain_time != 0;

            match (is_timeout, has_remain_time) {
                (true, true) => {
                    let remain_block = self.remain_time / CITA_BLOCK_INTERVAL as u32 + 1;
                    let valid_until_block = if (remain_block as u64) < CITA_BLOCK_LIMIT {
                        self.remain_time = 0;
                        current_height + remain_block as u64
                    } else {
                        self.remain_time -= 20 * CITA_BLOCK_INTERVAL as u32;
                        current_height + 20
                    };
                    self.tx.valid_until_block = valid_until_block;

                    let version = cita_client.client.get_version().unwrap();
                    if version == 0 {
                        let to = hex::encode(&self.auto_tx_info.tx_info.to);
                        self.tx.to = to;
                        self.tx.chain_id = cita_client
                            .client
                            .get_chain_id()
                            .map_err(|_| anyhow!("update_args get_chain_id failed"))?;
                    } else if version < 3 {
                        self.tx.to_v1 = self.auto_tx_info.tx_info.to.clone();
                        self.tx.chain_id_v1 = hex::decode(
                            cita_client
                                .client
                                .get_chain_id_v1()
                                .map_err(|_| anyhow!("update_args get_chain_id_v1 failed"))?
                                .completed_lower_hex(),
                        )?;
                    } else {
                        return Err(anyhow!("Invalid version"));
                    }
                    self.tx.version = version;
                    
                    //update hash
                    let tx: CitaTransaction = self.tx.clone().into();
                    let tx_bytes: Vec<u8> = tx.write_to_bytes()?;
                    let message_hash = hex::encode(self.auto_tx_info.account.hash(&tx_bytes));

                    // get sig
                    let sig = self.auto_tx_info.account.sign(&message_hash).await?;

                    // organize UnverifiedTransaction
                    let mut unverified_tx = UnverifiedTransaction::new();
                    let tx: CitaTransaction = self.tx.clone().into();
                    unverified_tx.set_transaction(tx);
                    unverified_tx.set_signature(sig);
                    unverified_tx.set_crypto(Crypto::DEFAULT);
                    let unverified_tx_vec = unverified_tx.write_to_bytes()?;
                    let tx_hash_vec = self.auto_tx_info.account.hash(&unverified_tx_vec);
                    self.hash.hash = tx_hash_vec.clone();
                    self.hash.unverified = unverified_tx_vec;

                    Ok(Some(self.get_current_hash()))
                }
                (true, false) => {
                    self.store_done(&state.storage, Some("Err: timeout".to_string()))
                        .await?;
                    Ok(None)
                }
                (false, _) => Ok(None),
            }
        } else {
            Err(anyhow!("wrong tx type"))
        }
    }

    async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        let res = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await;
        if let Ok(chain_info) = res {
            if let ChainType::Cita(mut cita_client) = chain_info.chain_type {
                let signed_tx = add_0x(hex::encode(self.hash.unverified.clone()));

                match cita_client.client.send_signed_transaction(&signed_tx) {
                    Ok(hash_resp) => {
                        warn!("hash_resp: {}", hash_resp);
                        let hash = self.get_current_hash();
                        info!(
                            "unsend task: {} send success, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_uncheck(&state.storage).await?;
                    }
                    Err(e) => {
                        info!(
                            "unsend task: {} send failed: {}, remain_time: {}",
                            self.get_key(),
                            e,
                            self.get_remain_time()
                        );
                        if self.update_args(&state).await?.is_some() {
                            self.store_unsend(&state.storage).await?;
                        }
                    }
                }
            }

            Ok(())
        } else {
            Err(anyhow!("send failed: get_chain_info failed"))
        }
    }

    async fn check(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        let res = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await;
        if let Ok(chain_info) = res {
            if let ChainType::Cita(cita_client) = chain_info.chain_type {
                let hash = add_0x(self.get_current_hash());
                // check receipt
                let result = cita_client
                    .client
                    .get_transaction_receipt(&hash)
                    .map_err(|_| anyhow!("check get_transaction_receipt failed"))?;
                match result.result() {
                    Some(_) => {
                        info!(
                            "uncheck task: {} check success, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_done(&state.storage, None).await?;
                    }
                    None => {
                        // check if timeout
                        info!(
                            "uncheck task: {} check failed: {:?}, remain_time: {}",
                            self.get_key(),
                            result.error(),
                            self.get_remain_time()
                        );
                        if self.update_args(&state).await?.is_some() {
                            self.store_unsend(&state.storage).await?;
                        }
                    }
                }
            }

            Ok(())
        } else {
            Err(anyhow!("check failed: get_chain_info failed"))
        }
    }
}
