use super::{AutoTx, AutoTxInfo, AutoTxTag, AutoTxType};
use crate::chains::{ChainType, Chains};
use crate::kms::Kms;
use crate::AutoTxGlobalState;
use anyhow::{anyhow, Result};
use cita_cloud_proto::blockchain::{
    raw_transaction::Tx, RawTransaction, Transaction as CitaCloudlTransaction,
    UnverifiedTransaction, Witness,
};
use cita_cloud_proto::client::{ClientOptions, InterceptedSvc};
use cita_cloud_proto::common::{Empty, Hash};
use cita_cloud_proto::controller::{
    rpc_service_client::RpcServiceClient as ControllerRpcServiceClient, Flag,
};
use cita_cloud_proto::evm::rpc_service_client::RpcServiceClient as EvmRpcServiceClient;
use cita_cloud_proto::executor::CallRequest;
use cita_cloud_proto::retry::RetryClient;
use ethabi::ethereum_types::U256;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CitaCloudClient {
    pub controller_client: RetryClient<ControllerRpcServiceClient<InterceptedSvc>>,
    pub evm_client: RetryClient<EvmRpcServiceClient<InterceptedSvc>>,
}

impl CitaCloudClient {
    pub fn new(url: &str) -> Result<Self> {
        let controller_addr = url.to_string() + ":50004";
        let controller_client =
            ClientOptions::new("controller".to_string(), controller_addr).connect_rpc()?;
        let evm_addr = url.to_string() + ":50002";
        let evm_client = ClientOptions::new("evm".to_string(), evm_addr).connect_evm()?;
        Ok(Self {
            controller_client,
            evm_client,
        })
    }

    async fn get_gas_limit(&mut self) -> Result<u64> {
        let client = self.controller_client.get_client_mut();
        let system_config = client.get_system_config(Empty {}).await?.into_inner();
        let gas_limit = system_config.quota_limit as u64;
        Ok(gas_limit)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CitaCloudlTransactionForSerde {
    pub version: u32,
    pub to: Vec<u8>,
    pub nonce: String,
    pub quota: u64,
    pub valid_until_block: u64,
    pub data: Vec<u8>,
    pub value: Vec<u8>,
    pub chain_id: Vec<u8>,
}

impl From<CitaCloudlTransactionForSerde> for CitaCloudlTransaction {
    fn from(value: CitaCloudlTransactionForSerde) -> Self {
        Self {
            version: value.version,
            to: value.to,
            nonce: value.nonce,
            quota: value.quota,
            valid_until_block: value.valid_until_block,
            data: value.data,
            value: value.value,
            chain_id: value.chain_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CitaCloudAutoTx {
    auto_tx_info: AutoTxInfo,
    remain_time: u32,
    tx: CitaCloudlTransactionForSerde,
    hash: Vec<u8>,
    tag: AutoTxTag,
}

impl CitaCloudAutoTx {
    pub fn new(auto_tx_info: AutoTxInfo, remain_time: u32) -> Self {
        let tx = CitaCloudlTransactionForSerde {
            to: auto_tx_info.tx_info.to.clone(),
            data: auto_tx_info.tx_info.data.clone(),
            value: auto_tx_info.tx_info.value.0.clone(),
            nonce: auto_tx_info.req_key.clone(),
            ..Default::default()
        };
        Self {
            auto_tx_info,
            remain_time,
            tx,
            hash: vec![],
            tag: AutoTxTag::Unsend,
        }
    }

    pub const fn get_remain_time(&self) -> u32 {
        self.remain_time
    }
}

#[axum::async_trait]
impl AutoTx for CitaCloudAutoTx {
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
        hex::encode(self.hash.clone())
    }

    fn to_unified_type(&self) -> AutoTxType {
        AutoTxType::CitaCloud(self.clone())
    }

    async fn update_gas(&mut self, chains: &Chains, self_update: bool) -> Result<()> {
        let chain_info = chains.get_chain_info(&self.auto_tx_info.chain_name).await?;
        if let ChainType::CitaCloud(mut client) = chain_info.chain_type {
            if self_update {
                let quota_limit = client.get_gas_limit().await?;
                let new_quota = quota_limit.min(self.tx.quota / 2 * 3);
                self.tx.quota = new_quota
            } else {
                let to = if self.auto_tx_info.is_create() {
                    vec![0u8; 20]
                } else {
                    self.auto_tx_info.tx_info.to.clone()
                };
                let call = CallRequest {
                    from: self.auto_tx_info.account.address(),
                    to,
                    method: self.tx.data.clone(),
                    args: Vec::new(),
                    height: 0,
                };
                let bytes_quota = client
                    .evm_client
                    .get_client_mut()
                    .estimate_quota(call)
                    .await?
                    .into_inner()
                    .bytes_quota;
                let quota = U256::from_big_endian(bytes_quota.as_slice()).as_u64();
                self.tx.quota = quota
            }
        }

        Ok(())
    }

    async fn update_if_timeout(&mut self, state: &AutoTxGlobalState) -> Result<bool> {
        let chain_info = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await?;
        if let ChainType::CitaCloud(mut client) = chain_info.chain_type {
            let client = client.controller_client.get_client_mut();
            let system_config = client.get_system_config(Empty {}).await?.into_inner();
            let current_height = client
                .get_block_number(Flag { flag: false })
                .await?
                .into_inner()
                .block_number;

            // update remain_time
            if current_height > self.tx.valid_until_block && self.tx.valid_until_block != 0 {
                let offset = ((current_height - self.tx.valid_until_block)
                    * system_config.block_interval as u64) as u32;
                self.remain_time = if self.remain_time > offset {
                    self.remain_time - offset
                } else {
                    0
                };
            }

            // consume remain_time if timeout
            let is_timeout = self.tx.valid_until_block <= current_height
                || self.tx.valid_until_block > (current_height + system_config.block_limit as u64);
            let has_remain_time = self.remain_time != 0;

            match (is_timeout, has_remain_time) {
                (true, true) => {
                    let block_interval = system_config.block_interval;
                    let remain_block = self.remain_time / block_interval + 1;
                    let valid_until_block = if remain_block < system_config.block_limit {
                        self.remain_time = 0;
                        current_height + remain_block as u64
                    } else {
                        self.remain_time -= 20 * block_interval;
                        current_height + 20
                    };
                    self.tx.valid_until_block = valid_until_block;

                    self.tx.version = system_config.version;
                    self.tx.chain_id = system_config.chain_id;

                    Ok(true)
                }
                (true, false) => {
                    self.store_done(&state.storage, Some("Err: timeout".to_string()))
                        .await?;
                    Ok(false)
                }
                (false, _) => Ok(false),
            }
        } else {
            Err(anyhow!("wrong tx type"))
        }
    }

    async fn update_current_hash(&mut self, _chains: &Chains) -> Result<String> {
        let tx: CitaCloudlTransaction = self.tx.clone().into();
        let tx_bytes = {
            let mut buf = Vec::with_capacity(tx.encoded_len());
            tx.encode(&mut buf).unwrap();
            buf
        };
        let hash_vec = self.auto_tx_info.account.hash(&tx_bytes);
        self.hash = hash_vec.clone();

        Ok(self.get_current_hash())
    }

    async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        let res = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await;
        if let Ok(chain_info) = res {
            if let ChainType::CitaCloud(mut client) = chain_info.chain_type {
                let controller_client = client.controller_client.get_client_mut();

                // get sig
                let sig = self
                    .auto_tx_info
                    .account
                    .sign(&self.get_current_hash())
                    .await?;

                // organize RawTransaction
                let raw_tx = {
                    let witness = Witness {
                        sender: self.auto_tx_info.tx_info.from.clone(),
                        signature: sig,
                    };

                    let tx: CitaCloudlTransaction = self.tx.clone().into();

                    let unverified_tx = UnverifiedTransaction {
                        transaction: Some(tx),
                        transaction_hash: self.hash.clone(),
                        witness: Some(witness),
                    };

                    RawTransaction {
                        tx: Some(Tx::NormalTx(unverified_tx)),
                    }
                };

                match controller_client.send_raw_transaction(raw_tx).await {
                    Ok(_) => {
                        let hash = self.get_current_hash();
                        info!(
                            "unsend task: {} send success, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_uncheck(&state.storage).await?;
                    }
                    Err(e) if e.message() == "HistoryDupTx" => {
                        let hash = self.get_current_hash();
                        info!(
                            "unsend task: {} already success, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_done(&state.storage, None).await?;
                    }
                    Err(e) if e.message() == "DupTransaction" => {
                        let hash = self.get_current_hash();
                        info!(
                            "unsend task: {} already sent, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_uncheck(&state.storage).await?;
                    }
                    Err(e) => {
                        info!(
                            "unsend task: {} send failed: {}, remain_time: {}",
                            self.get_key(),
                            e.message(),
                            self.get_remain_time()
                        );
                        if self.update_if_timeout(&state).await? {
                            self.update_current_hash(&state.chains).await?;
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
            if let ChainType::CitaCloud(mut client) = chain_info.chain_type {
                let evm_client = client.evm_client.get_client_mut();

                // check receipt
                match evm_client
                    .get_transaction_receipt(Hash {
                        hash: self.hash.clone(),
                    })
                    .await
                {
                    Ok(resp) => {
                        let error_message = resp.into_inner().error_message;
                        match error_message.as_str() {
                            "" => {
                                // success
                                let hash = self.get_current_hash();
                                info!(
                                    "uncheck task: {} check success, hash: {}",
                                    self.get_key(),
                                    hash
                                );
                                self.store_done(&state.storage, None).await?;
                            }
                            "Out of quota." => {
                                // self_update and resend
                                let hash = self.get_current_hash();
                                warn!(
                                    "uncheck task: {} check failed: out of gas, hash: {}, self_update and resend",
                                    self.get_key(),
                                    hash
                                );
                                self.update_gas(&state.chains, true).await?;
                                self.update_current_hash(&state.chains).await?;
                                self.store_unsend(&state.storage).await?;
                            }
                            s => {
                                // record failed
                                let hash = self.get_current_hash();
                                warn!(
                                    "uncheck task: {} check failed: {}, hash: {}",
                                    self.get_key(),
                                    s,
                                    hash
                                );
                                self.store_done(&state.storage, Some("execute failed".to_string()))
                                    .await?;
                            }
                        }
                    }
                    Err(e) => {
                        // check if timeout
                        info!(
                            "uncheck task: {} check failed: {}, remain_time: {}",
                            self.get_key(),
                            e.message(),
                            self.get_remain_time()
                        );
                        if self.update_if_timeout(&state).await? {
                            self.update_current_hash(&state.chains).await?;
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
