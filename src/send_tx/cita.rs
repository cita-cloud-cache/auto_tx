use super::{types::*, AutoTx};
use crate::kms::Kms;
use crate::storage::Storage;
use crate::util::{add_0x, remove_quotes_and_0x};
use cita_tool::{
    client::basic::{Client, ClientExt},
    Crypto, LowerHex, ParamsValue, ProtoMessage, ResponseValue, Transaction as CitaTransaction,
    UnverifiedTransaction,
};
use color_eyre::eyre::{eyre, Result};
use hex::ToHex;

const CITA_BLOCK_LIMIT: u64 = 88;

impl From<&SendTask> for CitaTransaction {
    fn from(value: &SendTask) -> Self {
        let tx_data = value.send_data.tx_data.clone();
        let nonce = value.base_data.request_key.clone();
        let valid_until_block = value.timeout.get_cita_timeout().valid_until_block;
        let gas = value.gas.gas;
        let to_v1 = tx_data.to.clone().unwrap_or_default();
        let to = to_v1.encode_hex::<String>();
        Self {
            to,
            nonce,
            quota: gas,
            valid_until_block,
            data: tx_data.data,
            value: tx_data.value.0,
            to_v1,
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub struct CitaClient {
    pub client: Client,
}

pub struct ReceiptInfo {
    pub error_message: Option<String>,
    pub contract_address: Option<String>,
}

impl CitaClient {
    pub fn new(url: &str) -> Result<Self> {
        let client = Client::new().set_uri(url);
        Ok(Self { client })
    }

    fn get_block_interval(&self) -> Result<u64> {
        let resp = self
            .client
            .get_metadata("latest")
            .map_err(|_| eyre!("get_metadata failed"))?;

        match resp.is_ok() {
            true => {
                let ResponseValue::Map(map) = resp.result().unwrap() else {
                    unreachable!()
                };
                let block_interval_str = map
                    .get("blockInterval")
                    .map(|p| p.to_string())
                    .unwrap_or_default();
                let block_interval = block_interval_str.parse::<u64>()? / 1000;
                Ok(block_interval)
            }
            false => Err(eyre!(format!(
                "get_block_interval failed: {}",
                resp.error().unwrap().message()
            ))),
        }
    }

    pub fn get_gas_limit(&self) -> Result<u64> {
        let resp = self
            .client
            .call(
                None,
                "0xffffffffffffffffffffffffffffffffff020003",
                Some("0x0bc8982f"),
                "latest",
                false,
            )
            .map_err(|_| eyre!("get_gas_limit failed"))?;

        match resp.is_ok() {
            true => {
                let ResponseValue::Singe(ParamsValue::String(gas_limit)) = resp.result().unwrap()
                else {
                    unreachable!()
                };
                let gas_limit = u64::from_str_radix(&remove_quotes_and_0x(&gas_limit), 16)?;
                Ok(gas_limit)
            }
            false => Err(eyre!(format!(
                "get_gas_limit failed: {}",
                resp.error().unwrap().message()
            ))),
        }
    }

    fn estimate_quota(
        &self,
        from: Option<&str>,
        to: &str,
        data: Option<&str>,
        height: &str,
    ) -> Result<u64> {
        let resp = self
            .client
            .estimate_quota(from, to, data, height)
            .map_err(|_| eyre!("estimate_quota failed"))?;

        match resp.is_ok() {
            true => {
                let ResponseValue::Singe(ParamsValue::String(quota)) = resp.result().unwrap()
                else {
                    unreachable!()
                };
                let quota = u64::from_str_radix(&remove_quotes_and_0x(&quota), 16)?;
                Ok(quota)
            }
            false => Err(eyre!(format!(
                "estimate_quota failed: {}",
                resp.error().unwrap().message()
            ))),
        }
    }

    fn send_signed_transaction(&mut self, signed_tx: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .send_signed_transaction(signed_tx)
            .map_err(|_| eyre!("send_signed_transaction failed"))?;

        match resp.is_ok() {
            true => {
                let ResponseValue::Map(map) = resp.result().unwrap() else {
                    unreachable!()
                };
                let hash = remove_quotes_and_0x(
                    &map.get("hash").map(|e| e.to_string()).unwrap_or_default(),
                );
                let hash_vec = hex::decode(hash)?;
                Ok(hash_vec)
            }
            false => Err(eyre!(format!(
                "send_signed_transaction failed: {}",
                resp.error().unwrap().message()
            ))),
        }
    }

    fn get_transaction_receipt(&self, hash: &str) -> Result<ReceiptInfo> {
        let resp = self
            .client
            .get_transaction_receipt(hash)
            .map_err(|_| eyre!("get_transaction_receipt failed"))?;
        match resp.is_ok() {
            true => {
                let ResponseValue::Map(map) = resp.result().unwrap() else {
                    unreachable!()
                };
                let error_message = map
                    .get("errorMessage")
                    .ok_or(eyre!("receipt no errorMessage"))
                    .map(|v| {
                        let s = remove_quotes_and_0x(&v.to_string());
                        if &s == "null" {
                            None
                        } else {
                            Some(s)
                        }
                    })?;
                let contract_address = map
                    .get("contractAddress")
                    .ok_or(eyre!("receipt no errorMessage"))
                    .map(|v| {
                        let s = remove_quotes_and_0x(&v.to_string());
                        if &s == "null" {
                            None
                        } else {
                            Some(s)
                        }
                    })?;

                Ok(ReceiptInfo {
                    error_message,
                    contract_address,
                })
            }
            false => match resp.error() {
                Some(e) => Err(eyre!(format!(
                    "get_transaction_receipt failed: {}",
                    e.message()
                ))),
                None => Err(eyre!(format!("get_transaction_receipt failed: not found",))),
            },
        }
    }
}

impl CitaClient {
    pub async fn try_update_timeout(&mut self, timeout: Timeout) -> Result<Timeout> {
        let mut timeout = timeout.get_cita_timeout();

        let block_interval = self.get_block_interval()? as u32;
        let block_limit = CITA_BLOCK_LIMIT as u32;
        let current_height = self
            .client
            .get_current_height()
            .map_err(|_| eyre!("update_args get_current_height failed"))?;

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

    pub async fn estimate_gas(&mut self, send_data: SendData) -> Result<Gas> {
        match send_data.tx_data.tx_type() {
            TxType::Store | TxType::Create => Ok(Gas { gas: 3_000_000 }),
            TxType::Normal => {
                let quota_limit = self.get_gas_limit()?;
                let to_vec = send_data.tx_data.to.clone().unwrap_or_default();
                let to = &add_0x(to_vec.encode_hex::<String>());
                let from = add_0x(send_data.account.address().encode_hex::<String>());
                let from = Some(from.as_str());
                let data = add_0x(send_data.tx_data.data.encode_hex::<String>());
                let data = Some(data.as_str());

                let quota = self.estimate_quota(from, to, data, "latest")?;
                let gas = quota_limit.min(quota / 2 * 3);
                Ok(Gas { gas })
            }
        }
    }

    pub async fn self_update_gas(&mut self, gas: Gas) -> Result<Gas> {
        let quota_limit = self.get_gas_limit()?;
        let gas = gas.gas;
        if quota_limit == gas {
            Err(eyre!("reach quota_limit"))
        } else {
            let new_gas = quota_limit.min(gas / 2 * 3);
            Ok(Gas { gas: new_gas })
        }
    }
}

impl AutoTx for CitaClient {
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
        let gas = self.estimate_gas(init_task.send_data.clone()).await?;

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
        let mut cita_tx = CitaTransaction::from(send_task);

        // update args
        let version = self
            .client
            .get_version()
            .map_err(|_| eyre!("process_send_task get_version failed"))?;
        match version {
            0 => {
                // new to must be empty
                cita_tx.to_v1 = Vec::new();
                cita_tx.chain_id = self
                    .client
                    .get_chain_id()
                    .map_err(|_| eyre!("update_args get_chain_id failed"))?;
            }
            version if version < 3 => {
                // old to must be empty
                cita_tx.to = String::new();
                cita_tx.chain_id_v1 = hex::decode(
                    self.client
                        .get_chain_id_v1()
                        .map_err(|_| eyre!("update_args get_chain_id_v1 failed"))?
                        .completed_lower_hex(),
                )?;
            }
            _ => unreachable!(),
        }
        cita_tx.version = version;

        // get signed
        let tx_bytes: Vec<u8> = cita_tx.write_to_bytes()?;
        let account = &send_task.send_data.account;
        let message_hash = hex::encode(account.hash(&tx_bytes));
        // get sig
        let sig = account.sign(&message_hash).await?;
        // organize UnverifiedTransaction
        let mut unverified_tx = UnverifiedTransaction::new();
        unverified_tx.set_transaction(cita_tx);
        unverified_tx.set_signature(sig);
        unverified_tx.set_crypto(Crypto::DEFAULT);
        let unverified_tx_vec = unverified_tx.write_to_bytes()?;
        let signed_tx = add_0x(hex::encode(&unverified_tx_vec));

        // send
        let request_key = &send_task.base_data.request_key;
        let send_result = self.send_signed_transaction(&signed_tx);
        match send_result {
            Ok(hash_to_check) => {
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
        let hash = &check_task.hash_to_check.hash;
        let hash_str = hash.encode_hex::<String>();
        let request_key = &check_task.base_data.request_key;
        match self.get_transaction_receipt(&add_0x(hash_str.clone())) {
            Ok(receipt) => {
                match receipt.error_message {
                    None => {
                        // success
                        let contract_address = receipt.contract_address;
                        let auto_tx_result =
                            AutoTxResult::success(hash_str.clone(), contract_address);
                        storage.finalize_task(request_key, &auto_tx_result).await?;
                        info!(
                            "uncheck task: {} check success, hash: {}",
                            request_key, hash_str
                        );

                        Ok(auto_tx_result)
                    }
                    Some(error) => {
                        match error.as_str() {
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
                                            storage
                                                .finalize_task(request_key, &auto_tx_result)
                                                .await?;
                                            warn!(
                                                "uncheck task: {} failed: reach quota_limit",
                                                request_key,
                                            );
                                        }
                                    }
                                }

                                Err(eyre!(error))
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

                                Err(eyre!(error))
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let timeout = storage.load_timeout(request_key).await?;
                warn!(
                    "uncheck task: {} check failed: {}, remain_time: {}",
                    request_key,
                    e.to_string(),
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
        match self.get_transaction_receipt(hash) {
            Ok(receipt) => match receipt.error_message {
                None => {
                    info!(
                        "get receipt success, hash: {}, contract_address: {:?}",
                        hash, receipt.contract_address
                    );
                    let auto_tx_result =
                        AutoTxResult::success(hash.to_string(), receipt.contract_address);

                    Ok(auto_tx_result)
                }
                Some(error) => {
                    warn!("get receipt failed, hash: {}, error: {}", hash, error);
                    Err(eyre!(error))
                }
            },
            Err(e) => {
                warn!("get receipt failed, hash: {}, error: {}", hash, e);
                Err(e)
            }
        }
    }
}
