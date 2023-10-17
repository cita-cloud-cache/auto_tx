use super::{AutoTx, AutoTxInfo, AutoTxTag, AutoTxType};
use crate::chains::{ChainType, Chains};
use crate::AutoTxGlobalState;
use anyhow::{anyhow, Result};
use ethabi::ethereum_types::{H256, U64};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use web3::{
    signing::Key,
    transports::Http,
    types::{
        Address, Bytes, CallRequest, SignedTransaction, TransactionParameters, TransactionRequest,
    },
    Error, Web3,
};

#[derive(Clone, Debug)]
pub struct EthClient {
    pub web3: Web3<Http>,
}

impl EthClient {
    pub fn new(url: &str) -> Result<Self> {
        let transport = web3::transports::Http::new(url)?;
        let web3 = web3::Web3::new(transport);
        Ok(Self { web3 })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SignedTransactionForSerde {
    pub raw_transaction: Bytes,
    pub transaction_hash: H256,
}

impl From<SignedTransaction> for SignedTransactionForSerde {
    fn from(value: SignedTransaction) -> Self {
        Self {
            raw_transaction: value.raw_transaction,
            transaction_hash: value.transaction_hash,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EthAutoTx {
    auto_tx_info: AutoTxInfo,
    tx: TransactionRequest,
    hash: SignedTransactionForSerde,
    tag: AutoTxTag,
}

impl EthAutoTx {
    pub fn new(auto_tx_info: AutoTxInfo) -> Self {
        let to = if auto_tx_info.is_create() {
            None
        } else {
            Some(Address::from_slice(&auto_tx_info.tx_info.to))
        };
        let tx = TransactionRequest {
            from: Address::from_slice(&auto_tx_info.tx_info.from),
            to,
            value: Some(auto_tx_info.tx_info.value.1),
            data: Some(Bytes(auto_tx_info.tx_info.data.clone())),
            transaction_type: Some(U64::from(2)),
            ..Default::default()
        };
        Self {
            auto_tx_info,
            tx,
            hash: SignedTransactionForSerde::default(),
            tag: AutoTxTag::Unsend,
        }
    }
}

#[axum::async_trait]
impl AutoTx for EthAutoTx {
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
        hex::encode(self.hash.transaction_hash)
    }

    fn to_unified_type(&self) -> AutoTxType {
        AutoTxType::Eth(self.clone())
    }

    async fn update_gas(&mut self, chains: &Chains, self_update: bool) -> Result<()> {
        let chain_info = chains.get_chain_info(&self.auto_tx_info.chain_name).await?;
        if let ChainType::Eth(client) = chain_info.chain_type {
            if self_update {
                let new_gas = self.tx.gas.unwrap() / 2 * 3;
                self.tx.gas = Some(new_gas)
            } else {
                let call_req = CallRequest {
                    from: Some(self.tx.from),
                    to: self.tx.to,
                    value: self.tx.value,
                    data: self.tx.data.clone(),
                    transaction_type: self.tx.transaction_type,
                    ..Default::default()
                };
                let gas = client.web3.eth().estimate_gas(call_req, None).await?;
                self.tx.gas = Some(gas / 2 * 3)
            }
        }

        Ok(())
    }

    async fn update_if_timeout(&mut self, state: &AutoTxGlobalState) -> Result<bool> {
        warn!("update_if_timeout");
        let chain_info = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await?;
        if let ChainType::Eth(client) = chain_info.chain_type {
            let current_nonce = self.tx.nonce;
            warn!("current_nonce: {current_nonce:?}");
            let from = self.auto_tx_info.account.address();
            let target_nonce = client.web3.eth().transaction_count(from, None).await?;
            warn!("target_nonce: {target_nonce}");

            // update if timeout
            if current_nonce.is_none() || current_nonce.unwrap() < target_nonce {
                // update nonce
                self.tx.nonce = Some(target_nonce);
                warn!("true new_nonce: {:?}", self.tx.nonce);
                Ok(true)
            } else {
                warn!("false");
                Ok(false)
            }
        } else {
            Err(anyhow!("wrong tx type"))
        }
    }

    async fn update_current_hash(&mut self, chains: &Chains) -> Result<String> {
        let chain_info = chains.get_chain_info(&self.auto_tx_info.chain_name).await?;
        if let ChainType::Eth(client) = chain_info.chain_type {
            let tx_params = TransactionParameters {
                nonce: self.tx.nonce,
                to: self.tx.to,
                gas: self.tx.gas.unwrap_or_default(),
                value: self.tx.value.unwrap_or_default(),
                data: self.tx.data.clone().unwrap_or_default(),
                transaction_type: self.tx.transaction_type,
                ..Default::default()
            };

            let signed_tx = client
                .web3
                .accounts()
                .sign_transaction(tx_params, self.auto_tx_info.account.clone())
                .await?;
            self.hash = signed_tx.into();
            Ok(self.get_current_hash())
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
            if let ChainType::Eth(client) = chain_info.chain_type {
                match client
                    .web3
                    .eth()
                    .send_raw_transaction(self.hash.raw_transaction.clone())
                    .await
                {
                    Ok(hash_return) => {
                        let hash = self.get_current_hash();
                        assert_eq!(hex::encode(hash_return), hash);
                        info!(
                            "unsend task: {} send success, hash: {}",
                            self.get_key(),
                            hash
                        );
                        self.store_uncheck(&state.storage).await?
                    }
                    Err(e) => {
                        if let Error::Rpc(e) = e.clone() {
                            if e.message == "nonce too low"
                                && client
                                    .web3
                                    .eth()
                                    .transaction_receipt(self.hash.transaction_hash)
                                    .await?
                                    .is_some()
                            {
                                info!("unsend task: {} already success: {}", self.get_key(), e);
                                self.store_done(&state.storage, None).await?;
                                return Ok(());
                            }
                        }
                        info!("unsend task: {} send failed: {}", self.get_key(), e);
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
            if let ChainType::Eth(client) = chain_info.chain_type {
                match client
                    .web3
                    .eth()
                    .transaction_receipt(self.hash.transaction_hash)
                    .await
                {
                    Ok(result) => match result {
                        Some(r) => {
                            match (r.status, r.gas_used) {
                                (Some(status), _) if status == U64::from(1) => {
                                    // success
                                    let hash = self.get_current_hash();
                                    info!(
                                        "uncheck task: {} check success, hash: {}",
                                        self.get_key(),
                                        hash
                                    );
                                    self.store_done(&state.storage, None).await?;
                                }
                                (Some(status), Some(used))
                                    if status == U64::from(0) && used == self.tx.gas.unwrap() =>
                                {
                                    // self_update and resend
                                    let hash = self.get_current_hash();
                                    warn!(
                                        "uncheck task: {} check failed: out of gas, hash: {}, self_update and resend",
                                        self.get_key(),
                                        hash
                                    );
                                    self.update_gas(&state.chains, true).await?;
                                    // self.update_if_timeout(&state).await?;
                                    self.update_current_hash(&state.chains).await?;
                                    self.store_unsend(&state.storage).await?;
                                }
                                _ => {
                                    // record failed
                                    let hash = self.get_current_hash();
                                    warn!(
                                        "uncheck task: {} check failed: Err: execute failed, hash: {}",
                                        self.get_key(),
                                        hash
                                    );
                                    self.store_done(
                                        &state.storage,
                                        Some("execute failed".to_string()),
                                    )
                                    .await?;
                                }
                            }
                        }
                        None => {
                            // check if timeout
                            info!("uncheck task: {} check failed: not found", self.get_key());
                            if self.update_if_timeout(&state).await? {
                                self.update_current_hash(&state.chains).await?;
                                self.store_unsend(&state.storage).await?;
                            }
                        }
                    },
                    Err(e) => {
                        // check if timeout
                        info!(
                            "uncheck task: {} transaction_receipt failed: {}",
                            self.get_key(),
                            e
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