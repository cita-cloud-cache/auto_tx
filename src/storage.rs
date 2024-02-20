use std::collections::HashSet;

use crate::{kms::Account, send_tx::types::*};
use color_eyre::eyre::{eyre, Result};
use etcd_client::{Client, GetOptions};
use paste::paste;

#[derive(Clone)]
pub struct Storage {
    operator: Client,
}

impl Storage {
    pub async fn new(endpoints: Vec<String>) -> Self {
        info!(" etcd endpoints: {:?}", endpoints);
        let operator = Client::connect(&endpoints, None)
            .await
            .map_err(|e| println!("etcd connect failed: {e}"))
            .unwrap();
        Self { operator }
    }
}

macro_rules! store_and_load {
    ($vis:ident, $data_type:ty, $var_name:ident, $dir:expr) => {
        paste! {
            impl Storage {
                pub($vis) async fn [<store_$var_name>](&self, request_key: &str, data: &$data_type) -> Result<()> {
                    let data_vec = bincode::serialize(&data)?;
                    let path = format!("{}/{}/{}", $dir, request_key, stringify!($var_name));
                    self.operator
                        .clone()
                        .put(path, data_vec, None)
                        .await
                        .map_err(|e| eyre!(e.to_string())).map(|_| ())
                }

                pub($vis) async fn [<load_$var_name>](&self, request_key: &str) -> Result<$data_type> {
                    let path = format!("{}/{}/{}", $dir, request_key, stringify!($var_name));
                    let data_vec = self
                        .operator
                        .clone()
                        .get(path, None)
                        .await
                        .map_err(|e| eyre!(e.to_string()))?;
                    if let Some(kv) = data_vec.kvs().first() {
                        let data = bincode::deserialize::<$data_type>(kv.value())?;
                        Ok(data)
                    } else {
                        Err(eyre!("data not found"))
                    }
                }

                #[allow(unused)]
                async fn [<delete_$var_name>](&self, request_key: &str) -> Result<()> {
                    let path = format!("{}/{}/{}", $dir, request_key, stringify!($var_name));
                    self.operator
                        .clone()
                        .delete(path, None)
                        .await
                        .map_err(|e| eyre!(e.to_string())).map(|_| ())
                }
            }
        }
    };
}

store_and_load!(self, BaseData, base_data, "processing");
store_and_load!(crate, Account, account, "processing");
store_and_load!(self, TxData, tx_data, "processing");
store_and_load!(crate, Timeout, timeout, "processing");
store_and_load!(crate, Gas, gas, "processing");
store_and_load!(crate, HashToCheck, hash_to_check, "processing");
store_and_load!(crate, Status, status, "processing");
store_and_load!(crate, AutoTxResult, auto_tx_result, "result");

impl Storage {
    pub async fn store_send_data(&self, request_key: &str, send_data: &SendData) -> Result<()> {
        self.store_account(request_key, &send_data.account).await?;
        self.store_tx_data(request_key, &send_data.tx_data).await?;

        Ok(())
    }

    pub async fn load_send_data(&self, request_key: &str) -> Result<SendData> {
        let account = self.load_account(request_key).await?;
        let tx_data = self.load_tx_data(request_key).await?;

        Ok(SendData { account, tx_data })
    }

    pub async fn store_init_task(&self, request_key: &str, init_task: &InitTask) -> Result<()> {
        self.store_base_data(request_key, &init_task.base_data)
            .await?;
        self.store_send_data(request_key, &init_task.send_data)
            .await?;
        self.store_status(request_key, &Status::Unsend).await?;

        Ok(())
    }

    pub async fn load_send_task(&self, request_key: &str) -> Result<SendTask> {
        let base_data = self.load_base_data(request_key).await?;
        let send_data = self.load_send_data(request_key).await?;
        let timeout = self.load_timeout(request_key).await?;
        let gas = self.load_gas(request_key).await?;

        let send_task = SendTask {
            base_data,
            send_data,
            timeout,
            gas,
        };

        Ok(send_task)
    }

    pub async fn load_check_task(&self, request_key: &str) -> Result<CheckTask> {
        let base_data = self.load_base_data(request_key).await?;
        let hash_to_check = self.load_hash_to_check(request_key).await?;

        let check_task = CheckTask {
            base_data,
            hash_to_check,
        };

        Ok(check_task)
    }

    pub async fn downgrade_to_unsend(&self, request_key: &str) -> Result<()> {
        self.delete_hash_to_check(request_key).await?;
        self.store_status(request_key, &Status::Unsend).await?;
        Ok(())
    }

    pub async fn finalize_task(
        &self,
        request_key: &str,
        auto_tx_result: &AutoTxResult,
    ) -> Result<()> {
        // delete all processing path
        self.delete_base_data(request_key).await?;
        self.delete_account(request_key).await?;
        self.delete_tx_data(request_key).await?;
        self.delete_timeout(request_key).await?;
        self.delete_gas(request_key).await?;
        self.delete_hash_to_check(request_key).await?;
        self.delete_status(request_key).await?;

        self.store_auto_tx_result(request_key, auto_tx_result)
            .await?;

        Ok(())
    }

    pub async fn get_processing_tasks(&self) -> Result<Vec<String>> {
        let option = GetOptions::new().with_prefix();
        let entries = self
            .operator
            .clone()
            .get("processing/", Some(option))
            .await?;
        let mut result = HashSet::new();
        for e in entries.kvs().iter() {
            if let Some(key) = e.key_str()?.split('/').take(2).last() {
                result.insert(key.to_owned());
            }
        }

        Ok(result.into_iter().collect())
    }
}
