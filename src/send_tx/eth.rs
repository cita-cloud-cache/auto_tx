use super::{AutoTx, DEFAULT_QUOTA};
use crate::config::get_config;
use crate::kms::Account;
use crate::storage::Storage;
use crate::task::*;
use color_eyre::eyre::{eyre, Result};
use common_rs::error::CALError;
use common_rs::redis::AsyncCommands;
use ethabi::ethereum_types::{H256, U64};
use hex::ToHex;
use web3::types::TransactionReceipt;
use web3::{
    signing::Key,
    transports::Http,
    types::{Address, Bytes, CallRequest, SignedTransaction, TransactionParameters, U256},
    types::{BlockId, BlockNumber},
    Web3,
};

const TRASNACTION_TYPE: u64 = 2;

impl From<&SendTask> for TransactionParameters {
    fn from(value: &SendTask) -> Self {
        let to = if value.base_data.tx_data.to.is_empty() {
            None
        } else {
            Some(Address::from_slice(&value.base_data.tx_data.to))
        };
        let gas = value.gas.gas;
        let nonce = value.timeout.get_eth_timeout().nonce;
        Self {
            to,
            value: value.base_data.tx_data.value.1,
            data: Bytes(value.base_data.tx_data.data.clone()),
            gas: U256::from(gas),
            nonce: Some(nonce),
            transaction_type: Some(U64::from(2)),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct EthClient {
    chain_name: String,
    web3: Web3<Http>,
}

impl EthClient {
    pub fn new(url: &str, name: &str) -> Result<Self> {
        let transport = web3::transports::Http::new(url)?;
        let web3 = web3::Web3::new(transport);
        Ok(Self {
            web3,
            chain_name: name.to_string(),
        })
    }

    pub async fn get_gas_limit(&self, storage: Option<&Storage>) -> Result<u64> {
        let key = format!(
            "{}/ChainSysConfig/{}/gas_limit",
            get_config().name,
            self.chain_name
        );
        if let Some(storage) = storage {
            if let Ok(gas_limit_bytes) = storage.operator().get(key.clone()).await {
                let gas_limit_bytes: Vec<u8> = gas_limit_bytes;
                if !gas_limit_bytes.is_empty() {
                    let gas_limit = u64::from_be_bytes(gas_limit_bytes.try_into().unwrap());
                    return Ok(gas_limit);
                }
            }
        }
        let gas_limit = (self
            .web3
            .eth()
            .block(BlockId::Number(BlockNumber::Latest))
            .await?
            .ok_or(eyre!("get_gas_limit get block failed"))?
            .gas_limit
            / 2)
        .as_u64();
        if let Some(storage) = storage {
            let gas_limit_bytes = gas_limit.to_be_bytes();
            storage
                .operator()
                .set_ex(key, &gas_limit_bytes, get_config().chain_config_ttl)
                .await?;
        }
        Ok(gas_limit)
    }

    async fn web3_estimate_gas(&self, call_request: CallRequest) -> Result<u64> {
        Ok(self
            .web3
            .eth()
            .estimate_gas(call_request, None)
            .await?
            .as_u64())
    }

    async fn get_nonce(&self, address: Address) -> Result<U256> {
        Ok(self.web3.eth().transaction_count(address, None).await?)
    }

    async fn sign_transaction(
        &self,
        tx: TransactionParameters,
        signer: Account,
    ) -> Result<SignedTransaction> {
        Ok(self.web3.accounts().sign_transaction(tx, signer).await?)
    }

    async fn send_raw_transaction(&self, rlp: Bytes) -> Result<H256> {
        Ok(self.web3.eth().send_raw_transaction(rlp).await?)
    }

    async fn transaction_receipt(&self, hash: H256) -> Result<Option<TransactionReceipt>> {
        Ok(self.web3.eth().transaction_receipt(hash).await?)
    }
}

impl EthClient {
    pub async fn try_update_timeout(&mut self, from: Address, timeout: Timeout) -> Result<Timeout> {
        let mut timeout = timeout.get_eth_timeout();

        let current_nonce = timeout.nonce;
        let target_nonce = self.get_nonce(from).await?;
        timeout.nonce = current_nonce.max(target_nonce);

        Ok(Timeout::Eth(timeout))
    }

    pub async fn estimate_gas(&mut self, init_task: &InitTaskParam) -> Gas {
        let call_request = CallRequest {
            from: Some(init_task.base_data.account.address()),
            to: if init_task.base_data.tx_data.to.is_empty() {
                None
            } else {
                Some(Address::from_slice(&init_task.base_data.tx_data.to))
            },
            value: Some(init_task.base_data.tx_data.value.1),
            data: Some(Bytes(init_task.base_data.tx_data.data.clone())),
            transaction_type: Some(TRASNACTION_TYPE.into()),
            ..Default::default()
        };
        let gas = self
            .web3_estimate_gas(call_request)
            .await
            .unwrap_or(DEFAULT_QUOTA);

        Gas { gas }
    }

    pub async fn self_update_gas(&mut self, gas: Gas, storage: Option<&Storage>) -> Result<Gas> {
        let quota_limit = self.get_gas_limit(storage).await?;
        let gas = gas.gas;
        if quota_limit == gas {
            Err(eyre!("reach quota_limit"))
        } else {
            let new_gas = quota_limit.min(gas / 2 * 3);
            Ok(Gas { gas: new_gas })
        }
    }
}

impl AutoTx for EthClient {
    async fn process_init_task(
        &mut self,
        init_task: &InitTaskParam,
        storage: &Storage,
    ) -> Result<(String, Timeout, Gas)> {
        // get timeout
        let timeout = Timeout::Eth(EthTimeout {
            nonce: U256::default(),
        });
        let from = init_task.base_data.account.address();
        let timeout = self.try_update_timeout(from, timeout).await?;

        // get Gas
        let gas = self.estimate_gas(init_task).await;
        // get tx
        let mut send_task = SendTask {
            base_data: init_task.base_data.clone(),
            timeout,
            gas,
            raw_transaction_bytes: None,
        };
        let eth_tx = TransactionParameters::from(&send_task);

        // get signed
        let account = init_task.base_data.account.clone();
        let signed_tx = self.sign_transaction(eth_tx, account.clone()).await?;
        // get tx_hash
        let tx_hash = signed_tx.transaction_hash.0;
        let tx_hash_str = tx_hash.encode_hex::<String>();

        // store all
        send_task.raw_transaction_bytes = Some(RawTransactionBytes {
            bytes: signed_tx.raw_transaction.0,
        });
        storage.store_send_task(&tx_hash_str, &send_task).await?;

        Ok((tx_hash_str, timeout, gas))
    }

    async fn process_send_task(
        &mut self,
        init_hash: &str,
        task: &SendTask,
        storage: &Storage,
    ) -> Result<String> {
        // get signed
        let account = task.base_data.account.clone();

        // get raw_transaction
        let raw_transaction = if let Some(raw_tx_bytes) = &task.raw_transaction_bytes {
            Bytes(raw_tx_bytes.bytes.clone())
        } else {
            // get tx
            let eth_tx = TransactionParameters::from(task);
            let signed_tx = self.sign_transaction(eth_tx, account.clone()).await?;
            signed_tx.raw_transaction
        };
        // send
        match self.send_raw_transaction(raw_transaction).await {
            Ok(hash) => {
                let hash_to_check = hash.0.to_vec();
                storage.store_status(init_hash, &Status::Uncheck).await?;
                storage
                    .store_hash_to_check(
                        init_hash,
                        &HashToCheck {
                            hash: hash_to_check.clone(),
                        },
                    )
                    .await?;

                let hash_str = hash_to_check.encode_hex::<String>();
                info!(
                    "unsend task: {} send success, hash: {}",
                    init_hash, hash_str
                );
                Ok(hash_str)
            }
            Err(e) => {
                warn!("unsend task: {} send failed: {}", init_hash, e.to_string(),);
                let address = account.address();
                let timeout = task.timeout;
                let new_timeout = self.try_update_timeout(address, timeout).await?;
                if timeout != new_timeout {
                    storage.store_timeout(init_hash, &new_timeout).await?;
                    // need rebuild the transaction
                    storage.delete_raw_transaction_bytes(init_hash).await?;
                    info!(
                        "unsend task: {} update timeout, nonce: {}",
                        init_hash,
                        timeout.get_eth_timeout().nonce
                    );
                }

                Err(e)
            }
        }
    }

    async fn process_check_task(
        &mut self,
        init_hash: &str,
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<TaskResult> {
        let hash = &check_task.hash_to_check.hash;
        let hash_str = hash.encode_hex::<String>();
        match self.transaction_receipt(H256::from_slice(hash)).await {
            Ok(result) => match result {
                Some(receipt) => {
                    let gas = storage.load_gas(init_hash).await?;
                    match (receipt.status, receipt.gas_used) {
                        (Some(status), _) if status == U64::from(1) => {
                            // success
                            let contract_address =
                                receipt.contract_address.map(|s| s.encode_hex::<String>());
                            let auto_tx_result =
                                TaskResult::success(hash_str.clone(), contract_address);
                            storage.finalize_task(init_hash, &auto_tx_result).await?;
                            info!(
                                "uncheck task: {} check success, hash: {}",
                                init_hash, hash_str
                            );

                            Ok(auto_tx_result)
                        }
                        (Some(status), Some(used))
                            if status == U64::from(0) && used.as_u64() == gas.gas =>
                        {
                            // self_update and resend
                            match self.self_update_gas(gas, Some(storage)).await {
                                Ok(gas) => {
                                    storage.store_gas(init_hash, &gas).await?;
                                    storage.downgrade_to_unsend(init_hash).await?;
                                    warn!(
                                    "uncheck task: {} check failed: out of gas, hash: {}, self_update and resend, gas: {}",
                                    init_hash, hash_str, gas.gas
                                );
                                }
                                Err(e) => {
                                    if e.to_string().as_str() == "reach quota_limit" {
                                        let auto_tx_result =
                                            TaskResult::failed(Some(hash_str), e.to_string());
                                        storage.finalize_task(init_hash, &auto_tx_result).await?;
                                        warn!(
                                            "uncheck task: {} failed: reach quota_limit",
                                            init_hash,
                                        );
                                    }
                                }
                            }

                            Err(eyre!("Out of quota."))
                        }
                        _ => {
                            // record failed
                            let err_info = "execute failed".to_string();
                            let auto_tx_result =
                                TaskResult::failed(Some(hash_str.clone()), err_info.clone());
                            storage.finalize_task(init_hash, &auto_tx_result).await?;
                            warn!(
                                "uncheck task: {} failed: {}, hash: {}",
                                init_hash, err_info, hash_str,
                            );

                            Err(eyre!(err_info.to_owned()))
                        }
                    }
                }
                None => {
                    warn!("uncheck task: {} check failed: not found", init_hash,);
                    let timeout = storage.load_timeout(init_hash).await?;
                    let address = check_task.base_data.account.address();
                    let new_timeout = self.try_update_timeout(address, timeout).await?;
                    if timeout != new_timeout {
                        storage.store_timeout(init_hash, &new_timeout).await?;
                        info!(
                            "uncheck task: {} update timeout, nonce: {}",
                            init_hash,
                            timeout.get_eth_timeout().nonce
                        );
                    }

                    Err(CALError::NotFound.into())
                }
            },
            Err(e) => {
                warn!(
                    "uncheck task: {} check failed: {}",
                    init_hash,
                    e.to_string(),
                );
                Err(e)
            }
        }
    }

    async fn get_receipt(&mut self, hash: &str) -> Result<TaskResult> {
        match self
            .transaction_receipt(H256::from_slice(&hex::decode(hash)?))
            .await
        {
            Ok(result) => match result {
                Some(receipt) => {
                    match (receipt.status, receipt.gas_used) {
                        (Some(status), _) if status == U64::from(1) => {
                            // success
                            let contract_address =
                                receipt.contract_address.map(|s| s.encode_hex::<String>());
                            info!(
                                "get receipt success, hash: {}, contract_address: {:?}",
                                hash, contract_address
                            );
                            let auto_tx_result =
                                TaskResult::success(hash.to_string(), contract_address);
                            Ok(auto_tx_result)
                        }
                        _ => {
                            // record failed
                            let err_info = "execute failed".to_string();

                            warn!("get receipt failed, hash: {}, error: {}", hash, err_info);
                            Err(eyre!(err_info.to_owned()))
                        }
                    }
                }
                None => Err(CALError::NotFound.into()),
            },
            Err(e) => {
                warn!("get receipt failed, hash: {}, error: {}", hash, e);
                Err(e)
            }
        }
    }
}
