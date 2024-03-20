use super::{AutoTx, BASE_QUOTA, DEFAULT_QUOTA, DEFAULT_QUOTA_LIMIT};
use crate::config::get_config;
use crate::kms::{Account, Kms};
use crate::storage::Storage;
use crate::task::*;
use cita_cloud_proto::blockchain::{
    raw_transaction::Tx, RawTransaction, Transaction as CitaCloudTransaction,
    UnverifiedTransaction, Witness,
};
use cita_cloud_proto::client::{ClientOptions, InterceptedSvc};
use cita_cloud_proto::common::{Empty, Hash};
use cita_cloud_proto::controller::{
    rpc_service_client::RpcServiceClient as ControllerRpcServiceClient, BlockNumber, Flag,
    SystemConfig,
};
use cita_cloud_proto::evm::rpc_service_client::RpcServiceClient as EvmRpcServiceClient;
use cita_cloud_proto::evm::{ByteQuota, Receipt};
use cita_cloud_proto::executor::CallRequest;
use cita_cloud_proto::retry::RetryClient;
use cita_cloud_proto::status_code::StatusCodeEnum;
use color_eyre::eyre::{eyre, Result};
use ethabi::ethereum_types::U256;
use hex::ToHex;
use prost::Message;

impl From<&SendTask> for CitaCloudTransaction {
    fn from(value: &SendTask) -> Self {
        let tx_data = value.base_data.tx_data.clone();
        let nonce = value.base_data.request_key.clone();
        let valid_until_block = value.timeout.get_cita_timeout().valid_until_block;
        let gas = value.gas.gas;
        Self {
            version: 0,
            to: tx_data.to.unwrap_or_default(),
            nonce,
            quota: gas,
            valid_until_block,
            data: tx_data.data,
            value: tx_data.value.0,
            chain_id: vec![],
        }
    }
}

async fn get_raw_tx(
    cita_cloud_tx: &CitaCloudTransaction,
    account: &Account,
) -> Result<RawTransaction> {
    // get hash
    let tx_bytes = {
        let mut buf = Vec::with_capacity(cita_cloud_tx.encoded_len());
        cita_cloud_tx.encode(&mut buf)?;
        buf
    };
    let hash_vec = account.hash(&tx_bytes);
    let hash = hash_vec.encode_hex::<String>();

    // get signature
    let signature = account.sign(&hash).await?;

    // get raw_tx
    let witness = Witness {
        sender: account.address(),
        signature,
    };

    let unverified_tx = UnverifiedTransaction {
        transaction: Some(cita_cloud_tx.to_owned()),
        transaction_hash: hash_vec,
        witness: Some(witness),
    };

    Ok(RawTransaction {
        tx: Some(Tx::NormalTx(unverified_tx)),
    })
}

#[derive(Clone, Debug)]
pub struct CitaCloudClient {
    pub chain_name: String,
    pub controller_client: RetryClient<ControllerRpcServiceClient<InterceptedSvc>>,
    pub evm_client: RetryClient<EvmRpcServiceClient<InterceptedSvc>>,
}

macro_rules! cita_cloud_method {
    ($fn_name:ident, $ret:ty, $client:ident, $input:ty) => {
        impl CitaCloudClient {
            async fn $fn_name(&mut self, arg: $input) -> Result<$ret> {
                let client = self.$client.get_client_mut();
                let fn_name = stringify!($fn_name);
                client
                    .$fn_name(arg)
                    .await
                    .map(|response| response.into_inner())
                    .map_err(|e| eyre!("{} failed: {}", fn_name, e.message()))
            }
        }
    };
}
cita_cloud_method!(get_block_number, BlockNumber, controller_client, Flag);
// cita_cloud_method!(get_system_config, SystemConfig, controller_client, Empty);
cita_cloud_method!(
    send_raw_transaction,
    Hash,
    controller_client,
    RawTransaction
);
cita_cloud_method!(get_transaction_receipt, Receipt, evm_client, Hash);
cita_cloud_method!(estimate_quota, ByteQuota, evm_client, CallRequest);

impl CitaCloudClient {
    pub fn new(url: &str, name: &str) -> Result<Self> {
        let controller_addr = url.to_string() + ":50004";
        let controller_client =
            ClientOptions::new("controller".to_string(), controller_addr).connect_rpc()?;
        let evm_addr = url.to_string() + ":50002";
        let evm_client = ClientOptions::new("evm".to_string(), evm_addr).connect_evm()?;
        Ok(Self {
            chain_name: name.to_owned(),
            controller_client,
            evm_client,
        })
    }

    async fn get_system_config(&mut self, storage: Option<&Storage>) -> Result<SystemConfig> {
        let key = format!("{}/ChainSysConfig/{}", get_config().name, self.chain_name);
        if let Some(storage) = storage {
            if let Ok(system_config_bytes) = storage.get(key.clone()).await {
                let system_config = SystemConfig::decode::<std::collections::VecDeque<u8>>(
                    system_config_bytes.into(),
                )?;
                return Ok(system_config);
            }
        }
        let client = self.controller_client.get_client_mut();
        let system_config = client
            .get_system_config(Empty {})
            .await
            .map(|response| response.into_inner())?;
        if let Some(storage) = storage {
            let system_config_bytes = {
                let mut buf = Vec::with_capacity(system_config.encoded_len());
                system_config.encode(&mut buf)?;
                buf
            };
            storage
                .put_with_lease(key, system_config_bytes, get_config().chain_config_ttl)
                .await
                .ok();
        }
        Ok(system_config)
    }

    pub async fn get_gas_limit(&mut self, storage: Option<&Storage>) -> Result<u64> {
        let system_config = self.get_system_config(storage).await?;
        let gas_limit = system_config.quota_limit as u64;
        Ok(gas_limit)
    }

    pub async fn try_update_timeout(
        &mut self,
        timeout: Timeout,
        storage: &Storage,
    ) -> Result<Timeout> {
        let mut timeout = timeout.get_cita_timeout();

        let system_config = self.get_system_config(Some(storage)).await?;
        let block_interval = system_config.block_interval;
        let block_limit = system_config.block_limit;
        let current_height = self
            .get_block_number(Flag { flag: false })
            .await?
            .block_number;

        // offset remain_time
        if current_height > timeout.valid_until_block && timeout.valid_until_block != 0 {
            let offset =
                ((current_height - timeout.valid_until_block) * block_interval as u64) as u32;
            timeout.remain_time = if timeout.remain_time > offset {
                timeout.remain_time - offset
            } else {
                0
            };
        }

        // consume remain_time if timeout
        let is_timeout = timeout.valid_until_block <= current_height
            || timeout.valid_until_block > (current_height + block_limit as u64);
        let has_remain_time = timeout.remain_time != 0;
        match (is_timeout, has_remain_time) {
            (true, true) => {
                let remain_block = timeout.remain_time / block_interval + 1;
                let valid_until_block = if remain_block < block_limit {
                    timeout.remain_time = 0;
                    current_height + remain_block as u64
                } else {
                    timeout.remain_time -= 20 * block_interval;
                    current_height + 20
                };
                timeout.valid_until_block = valid_until_block;

                Ok(Timeout::Cita(timeout))
            }
            (true, false) => Err(eyre!("timeout")),
            (false, _) => Ok(Timeout::Cita(timeout)),
        }
    }

    pub async fn estimate_gas(&mut self, init_task: &InitTaskParam, storage: &Storage) -> Gas {
        match init_task.base_data.tx_data.tx_type() {
            TxType::Store => Gas {
                // 200 gas per byte
                // 1.5 times
                gas: ((init_task.base_data.tx_data.data.len() * 200) as u64 + BASE_QUOTA) / 2 * 3,
            },
            t => {
                let quota_limit = self
                    .get_gas_limit(Some(storage))
                    .await
                    .unwrap_or(DEFAULT_QUOTA_LIMIT);
                let to = match t {
                    TxType::Create => vec![0u8; 20],
                    TxType::Store => unreachable!(),
                    TxType::Normal => init_task.base_data.tx_data.to.clone().unwrap_or_default(),
                };
                let call = CallRequest {
                    from: init_task.base_data.account.address(),
                    to,
                    method: init_task.base_data.tx_data.data.clone(),
                    args: Vec::new(),
                    height: 0,
                };
                let estimate_quota_timeout = tokio::time::timeout(
                    std::time::Duration::from_millis(get_config().rpc_timeout),
                    self.estimate_quota(call),
                );
                match estimate_quota_timeout.await {
                    Ok(Ok(byte_quota)) => {
                        let quota =
                            U256::from_big_endian(byte_quota.bytes_quota.as_slice()).as_u64();
                        let gas = quota_limit.min(quota / 2 * 3);
                        Gas { gas }
                    }
                    Ok(Err(e)) => {
                        warn!("estimate_quota failed: {}", e);
                        Gas { gas: DEFAULT_QUOTA }
                    }
                    Err(e) => {
                        warn!("estimate_quota timeout: {}", e);
                        Gas { gas: DEFAULT_QUOTA }
                    }
                }
            }
        }
    }

    pub async fn self_update_gas(&mut self, gas: Gas, storage: &Storage) -> Result<Gas> {
        let quota_limit = self.get_gas_limit(Some(storage)).await?;
        let gas = gas.gas;
        if quota_limit == gas {
            Err(eyre!("reach quota_limit"))
        } else {
            let new_gas = quota_limit.min(gas / 2 * 3);
            Ok(Gas { gas: new_gas })
        }
    }
}

impl AutoTx for CitaCloudClient {
    async fn process_init_task(
        &mut self,
        init_task: &InitTaskParam,
        storage: &Storage,
    ) -> Result<(String, Timeout, Gas)> {
        debug!("process_init_task: {:?}", init_task);
        // get timeout
        let timeout = Timeout::Cita(CitaTimeout {
            remain_time: init_task.timeout,
            valid_until_block: 0,
        });
        let timeout = self.try_update_timeout(timeout, storage).await?;

        // get Gas
        let gas = self.estimate_gas(init_task, storage).await;

        // get tx
        let mut send_task = SendTask {
            base_data: init_task.base_data.clone(),
            timeout,
            gas,
            raw_transaction_bytes: None,
        };
        let mut cita_cloud_tx = CitaCloudTransaction::from(&send_task);

        // update args
        let system_config = self.get_system_config(Some(storage)).await?;
        cita_cloud_tx.version = system_config.version;
        cita_cloud_tx.chain_id = system_config.chain_id;

        // get raw_tx
        let account = &init_task.base_data.account;
        let raw_tx = get_raw_tx(&cita_cloud_tx, account).await?;

        // get tx_hash
        let tx_hash = get_tx_hash(&raw_tx)?;
        let tx_hash_str = tx_hash.encode_hex::<String>();

        // get tx_bytes
        let tx_bytes = {
            let mut buf = Vec::with_capacity(raw_tx.encoded_len());
            raw_tx.encode(&mut buf)?;
            buf
        };

        // store all
        send_task.raw_transaction_bytes = Some(RawTransactionBytes { bytes: tx_bytes });
        storage.store_send_task(&tx_hash_str, &send_task).await?;

        Ok((tx_hash_str, timeout, gas))
    }

    async fn process_send_task(
        &mut self,
        init_hash: &str,
        task: &SendTask,
        storage: &Storage,
    ) -> Result<String> {
        debug!("process_send_task: {:?}", task);
        let raw_tx = if let Some(raw_tx_bytes) = &task.raw_transaction_bytes {
            RawTransaction::decode::<std::collections::VecDeque<u8>>(
                raw_tx_bytes.bytes.clone().into(),
            )?
        } else {
            // get tx
            let mut cita_cloud_tx = CitaCloudTransaction::from(task);
            // update args
            let system_config = self.get_system_config(Some(storage)).await?;
            cita_cloud_tx.version = system_config.version;
            cita_cloud_tx.chain_id = system_config.chain_id;

            // get raw_tx
            let account = &task.base_data.account;
            get_raw_tx(&cita_cloud_tx, account).await?
        };

        // send
        let send_result = self.send_raw_transaction(raw_tx).await;
        match send_result {
            Ok(hash) => {
                let hash_to_check = hash.hash;
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
                let timeout = task.timeout;
                warn!(
                    "unsend task: {} send failed: {}, remain_time: {}",
                    init_hash,
                    e.to_string(),
                    timeout.get_cita_timeout().remain_time
                );
                match self.try_update_timeout(timeout, storage).await {
                    Ok(new_timeout) => {
                        debug!("{init_hash} new_timeout: {new_timeout} ");
                        if timeout != new_timeout {
                            storage.store_timeout(init_hash, &new_timeout).await?;
                            // need rebuild the transaction
                            storage.delete_raw_transaction_bytes(init_hash).await?;
                            info!(
                                "unsend task: {} update timeout, remain_time: {}",
                                init_hash,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                    Err(e) => {
                        if e.to_string().as_str() == "timeout" {
                            let auto_tx_result = TaskResult::failed(None, e.to_string());
                            storage.finalize_task(init_hash, &auto_tx_result).await?;
                            warn!(
                                "unsend task: {} failed: timeout, remain_time: {}",
                                init_hash,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
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
        debug!("process_check_task: {:?}", check_task);
        let hash = check_task.hash_to_check.hash.clone();
        let hash_str = hash.encode_hex::<String>();
        let get_transaction_receipt_timeout = tokio::time::timeout(
            std::time::Duration::from_millis(get_config().rpc_timeout),
            self.get_transaction_receipt(Hash { hash }),
        );
        match get_transaction_receipt_timeout.await {
            Ok(Ok(receipt)) => match receipt.error_message.as_str() {
                "" => {
                    // success
                    let contract_address = receipt.contract_address;
                    let contract_address = if contract_address == vec![0; 20] {
                        None
                    } else {
                        Some(contract_address.encode_hex::<String>())
                    };
                    let auto_tx_result = TaskResult::success(hash_str.clone(), contract_address);
                    storage.finalize_task(init_hash, &auto_tx_result).await?;
                    info!(
                        "uncheck task: {} check success, hash: {}",
                        init_hash, hash_str
                    );

                    Ok(auto_tx_result)
                }
                "Out of quota." => {
                    // self_update and resend
                    let gas = storage.load_gas(init_hash).await?;
                    match self.self_update_gas(gas, storage).await {
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
                                warn!("uncheck task: {} failed: reach quota_limit", init_hash,);
                            }
                        }
                    }

                    Err(eyre!(receipt.error_message))
                }
                e => {
                    // record fail
                    let auto_tx_result = TaskResult::failed(Some(hash_str.clone()), e.to_string());
                    storage.finalize_task(init_hash, &auto_tx_result).await?;
                    warn!(
                        "uncheck task: {} failed: {}, hash: {}",
                        init_hash, e, hash_str,
                    );

                    Err(eyre!(e.to_owned()))
                }
            },
            Ok(Err(e)) => {
                if !e.to_string().contains("Not get the receipt") {
                    return Err(e);
                }
                let timeout = storage.load_timeout(init_hash).await?;
                warn!(
                    "uncheck task: {} check failed: Not get the receipt, remain_time: {}",
                    init_hash,
                    timeout.get_cita_timeout().remain_time
                );
                match self.try_update_timeout(timeout, storage).await {
                    Ok(new_timeout) => {
                        if timeout != new_timeout {
                            storage.store_timeout(init_hash, &new_timeout).await?;
                            // resend uncheck task if timeout
                            storage.downgrade_to_unsend(init_hash).await?;
                            warn!(
                                "uncheck task: {} downgrade to unsend, remain_time: {}",
                                init_hash,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                    Err(e) => {
                        if e.to_string().as_str() == "timeout" {
                            let auto_tx_result = TaskResult::failed(Some(hash_str), e.to_string());
                            storage.finalize_task(init_hash, &auto_tx_result).await?;
                            warn!(
                                "uncheck task: {} failed: timeout, remain_time: {}",
                                init_hash,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                }

                Err(e)
            }
            Err(_) => {
                warn!(
                    "uncheck task: {} failed: get_transaction_receipt rpc timeout, hash: {}",
                    init_hash, hash_str,
                );

                Err(eyre!("get_transaction_receipt rpc timeout"))
            }
        }
    }

    async fn get_receipt(&mut self, hash: &str) -> Result<TaskResult> {
        match self
            .get_transaction_receipt(Hash {
                hash: hex::decode(hash)?,
            })
            .await
        {
            Ok(receipt) => match receipt.error_message.as_str() {
                "" => {
                    let contract_address = if receipt.contract_address == vec![0; 20] {
                        None
                    } else {
                        Some(receipt.contract_address.encode_hex::<String>())
                    };
                    info!(
                        "get receipt success, hash: {}, contract_address: {:?}",
                        hash, contract_address
                    );
                    let auto_tx_result = TaskResult::success(hash.to_string(), contract_address);

                    Ok(auto_tx_result)
                }
                error => {
                    warn!("get receipt failed, hash: {}, error: {}", hash, error);
                    Err(eyre!(receipt.error_message))
                }
            },
            Err(e) => {
                warn!("get receipt failed, hash: {}, error: {}", hash, e);
                Err(e)
            }
        }
    }
}

pub fn get_tx_hash(raw_tx: &RawTransaction) -> Result<&[u8], StatusCodeEnum> {
    match raw_tx.tx {
        Some(Tx::NormalTx(ref normal_tx)) => Ok(&normal_tx.transaction_hash),
        Some(Tx::UtxoTx(ref utxo_tx)) => Ok(&utxo_tx.transaction_hash),
        None => Err(StatusCodeEnum::NoTransaction),
    }
}
