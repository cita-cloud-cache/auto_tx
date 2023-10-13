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
        let tx = TransactionRequest {
            from: Address::from_slice(&auto_tx_info.tx_info.from),
            to: Some(Address::from_slice(&auto_tx_info.tx_info.to)),
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

    async fn estimate_gas(&mut self, chains: &Chains) -> Result<()> {
        let chain_info = chains.get_chain_info(&self.auto_tx_info.chain_name).await?;
        if let ChainType::Eth(client) = chain_info.chain_type {
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

        Ok(())
    }

    async fn update_args(&mut self, state: &AutoTxGlobalState) -> Result<Option<String>> {
        let chain_info = state
            .chains
            .get_chain_info(&self.auto_tx_info.chain_name)
            .await?;
        if let ChainType::Eth(client) = chain_info.chain_type {
            let current_nonce = self.tx.nonce;
            let from = self.auto_tx_info.account.address();
            let target_nonce = client.web3.eth().transaction_count(from, None).await?;

            // update if timeout
            if current_nonce.is_none() || current_nonce.unwrap() < target_nonce {
                // update nonce
                self.tx.nonce = Some(target_nonce);

                // update hash
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
                warn!("signed_tx: {:?}", signed_tx);
                let hash = hex::encode(signed_tx.transaction_hash.0);
                self.hash = signed_tx.into();

                Ok(Some(hash))
            } else {
                Ok(None)
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
            if let ChainType::Eth(client) = chain_info.chain_type {
                if client
                    .web3
                    .eth()
                    .transaction_receipt(self.hash.transaction_hash)
                    .await?
                    .is_some()
                {
                    let hash = self.get_current_hash();
                    info!(
                        "uncheck task: {} check success, hash: {}",
                        self.get_key(),
                        hash
                    );
                    self.store_done(&state.storage, None).await?;
                }
            } else {
                // check if timeout
                info!("uncheck task: {} check failed", self.get_key(),);
                if self.update_args(&state).await?.is_some() {
                    self.store_unsend(&state.storage).await?;
                }
            }

            Ok(())
        } else {
            Err(anyhow!("check failed: get_chain_info failed"))
        }
    }
}
