use super::{types::*, AutoTx, BASE_QUOTA, DEFAULT_QUOTA, DEFAULT_QUOTA_LIMIT, RPC_TIMEOUT};
use crate::kms::{Account, Kms};
use crate::storage::Storage;
use cita_cloud_proto::blockchain::{
    raw_transaction::Tx, RawTransaction, Transaction as CitaCloudlTransaction,
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
use color_eyre::eyre::{eyre, Result};
use ethabi::ethereum_types::U256;
use hex::ToHex;
use prost::Message;

impl From<&SendTask> for CitaCloudlTransaction {
    fn from(value: &SendTask) -> Self {
        let tx_data = value.send_data.tx_data.clone();
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
    cita_cloud_tx: &CitaCloudlTransaction,
    account: &Account,
) -> Result<RawTransaction> {
    // get hash
    let tx_bytes = {
        let mut buf = Vec::with_capacity(cita_cloud_tx.encoded_len());
        cita_cloud_tx.encode(&mut buf).unwrap();
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
                    .map_err(|e| eyre!(format!("{} failed: {}", fn_name, e.message())))
            }
        }
    };
}
cita_cloud_method!(get_block_number, BlockNumber, controller_client, Flag);
cita_cloud_method!(get_system_config, SystemConfig, controller_client, Empty);
cita_cloud_method!(
    send_raw_transaction,
    Hash,
    controller_client,
    RawTransaction
);
cita_cloud_method!(get_transaction_receipt, Receipt, evm_client, Hash);
cita_cloud_method!(estimate_quota, ByteQuota, evm_client, CallRequest);

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

    pub async fn get_gas_limit(&mut self) -> Result<u64> {
        let system_config = self.get_system_config(Empty {}).await?;
        let gas_limit = system_config.quota_limit as u64;
        Ok(gas_limit)
    }

    pub async fn try_update_timeout(&mut self, timeout: Timeout) -> Result<Timeout> {
        let mut timeout = timeout.get_cita_timeout();

        let system_config = self.get_system_config(Empty {}).await?;
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

    pub async fn estimate_gas(&mut self, send_data: SendData) -> Gas {
        match send_data.tx_data.tx_type() {
            TxType::Store => Gas {
                // 200 gas per byte
                // 1.5 times
                gas: ((send_data.tx_data.data.len() * 200) as u64 + BASE_QUOTA) / 2 * 3,
            },
            t => {
                let quota_limit = self.get_gas_limit().await.unwrap_or(DEFAULT_QUOTA_LIMIT);
                let to = match t {
                    TxType::Create => vec![0u8; 20],
                    TxType::Store => unreachable!(),
                    TxType::Normal => send_data.tx_data.to.clone().unwrap_or_default(),
                };
                let call = CallRequest {
                    from: send_data.account.address(),
                    to,
                    method: send_data.tx_data.data.clone(),
                    args: Vec::new(),
                    height: 0,
                };
                let estimate_quota_timeout = tokio::time::timeout(
                    std::time::Duration::from_secs(RPC_TIMEOUT),
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

    pub async fn self_update_gas(&mut self, gas: Gas) -> Result<Gas> {
        let quota_limit = self.get_gas_limit().await?;
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
        init_task: &InitTask,
        storage: &Storage,
    ) -> Result<(Timeout, Gas)> {
        // get timeout
        let timeout = Timeout::Cita(CitaTimeout {
            remain_time: init_task.timeout,
            valid_until_block: 0,
        });
        let timeout = self.try_update_timeout(timeout).await?;

        // get Gas
        let gas = self.estimate_gas(init_task.send_data.clone()).await;

        // store all
        let request_key = &init_task.base_data.request_key;

        storage.store_timeout(request_key, &timeout).await?;
        storage.store_gas(request_key, &gas).await?;
        storage.store_init_task(request_key, init_task).await?;

        Ok((timeout, gas))
    }

    async fn process_send_task(
        &mut self,
        send_task: &SendTask,
        storage: &Storage,
    ) -> Result<String> {
        // get tx
        let mut cita_cloud_tx = CitaCloudlTransaction::from(send_task);
        // update args
        let system_config = self.get_system_config(Empty {}).await?;
        cita_cloud_tx.version = system_config.version;
        cita_cloud_tx.chain_id = system_config.chain_id;

        // get raw_tx
        let account = &send_task.send_data.account;
        let raw_tx = get_raw_tx(&cita_cloud_tx, account).await?;

        // send
        let request_key = &send_task.base_data.request_key;
        let send_result = self.send_raw_transaction(raw_tx).await;
        match send_result {
            Ok(hash) => {
                let hash_to_check = hash.hash;
                storage.store_status(request_key, &Status::Uncheck).await?;
                storage
                    .store_hash_to_check(
                        request_key,
                        &HashToCheck {
                            hash: hash_to_check.clone(),
                        },
                    )
                    .await?;

                let hash_str = hash_to_check.encode_hex::<String>();
                info!(
                    "unsend task: {} send success, hash: {}",
                    request_key, hash_str
                );
                Ok(hash_str)
            }
            Err(e) => {
                let timeout = send_task.timeout;
                warn!(
                    "unsend task: {} send failed: {}, remain_time: {}",
                    request_key,
                    e.to_string(),
                    timeout.get_cita_timeout().remain_time
                );
                match self.try_update_timeout(timeout).await {
                    Ok(new_timeout) => {
                        if timeout != new_timeout {
                            storage.store_timeout(request_key, &new_timeout).await?;
                            info!(
                                "unsend task: {} update timeout, remain_time: {}",
                                request_key,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                    Err(e) => {
                        if e.to_string().as_str() == "timeout" {
                            let auto_tx_result = AutoTxResult::failed(None, e.to_string());
                            storage.finalize_task(request_key, &auto_tx_result).await?;
                            warn!(
                                "unsend task: {} failed: timeout, remain_time: {}",
                                request_key,
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
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<AutoTxResult> {
        let hash = check_task.hash_to_check.hash.clone();
        let hash_str = hash.encode_hex::<String>();
        let request_key = &check_task.base_data.request_key;

        match self.get_transaction_receipt(Hash { hash }).await {
            Ok(receipt) => match receipt.error_message.as_str() {
                "" => {
                    // success
                    let contract_address = receipt.contract_address;
                    let contract_address = if contract_address == vec![0; 20] {
                        None
                    } else {
                        Some(contract_address.encode_hex::<String>())
                    };
                    let auto_tx_result = AutoTxResult::success(hash_str.clone(), contract_address);
                    storage.finalize_task(request_key, &auto_tx_result).await?;
                    info!(
                        "uncheck task: {} check success, hash: {}",
                        request_key, hash_str
                    );

                    Ok(auto_tx_result)
                }
                "Out of quota." => {
                    // self_update and resend
                    let gas = storage.load_gas(request_key).await?;
                    match self.self_update_gas(gas).await {
                        Ok(gas) => {
                            storage.store_gas(request_key, &gas).await?;
                            storage.downgrade_to_unsend(request_key).await?;
                            warn!(
                                "uncheck task: {} check failed: out of gas, hash: {}, self_update and resend, gas: {}",
                                request_key, hash_str, gas.gas
                            );
                        }
                        Err(e) => {
                            if e.to_string().as_str() == "reach quota_limit" {
                                let auto_tx_result =
                                    AutoTxResult::failed(Some(hash_str), e.to_string());
                                storage.finalize_task(request_key, &auto_tx_result).await?;
                                warn!("uncheck task: {} failed: reach quota_limit", request_key,);
                            }
                        }
                    }

                    Err(eyre!(receipt.error_message))
                }
                e => {
                    // record fail
                    let auto_tx_result =
                        AutoTxResult::failed(Some(hash_str.clone()), e.to_string());
                    storage.finalize_task(request_key, &auto_tx_result).await?;
                    warn!(
                        "uncheck task: {} failed: {}, hash: {}",
                        request_key, e, hash_str,
                    );

                    Err(eyre!(e.to_owned()))
                }
            },
            Err(e) => {
                if !e.to_string().contains("Not get the receipt") {
                    return Err(e);
                }
                let timeout = storage.load_timeout(request_key).await?;
                warn!(
                    "uncheck task: {} check failed: Not get the receipt, remain_time: {}",
                    request_key,
                    timeout.get_cita_timeout().remain_time
                );
                match self.try_update_timeout(timeout).await {
                    Ok(new_timeout) => {
                        if timeout != new_timeout {
                            storage.store_timeout(request_key, &new_timeout).await?;
                            // resend uncheck task if timeout
                            storage.downgrade_to_unsend(request_key).await?;
                            warn!(
                                "uncheck task: {} downgrade to unsend, remain_time: {}",
                                request_key,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                    Err(e) => {
                        if e.to_string().as_str() == "timeout" {
                            let auto_tx_result =
                                AutoTxResult::failed(Some(hash_str), e.to_string());
                            storage.finalize_task(request_key, &auto_tx_result).await?;
                            warn!(
                                "uncheck task: {} failed: timeout, remain_time: {}",
                                request_key,
                                timeout.get_cita_timeout().remain_time
                            );
                        }
                    }
                }

                Err(e)
            }
        }
    }

    async fn get_receipt(&mut self, hash: &str) -> Result<AutoTxResult> {
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
                    let auto_tx_result = AutoTxResult::success(hash.to_string(), contract_address);

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
