use std::time::Duration;

use crate::{config::get_config, task::*};
use color_eyre::eyre::{eyre, Result};
use etcd_client::{Client, ConnectOptions, GetOptions, LockOptions, PutOptions};
use paste::paste;

#[derive(Clone)]
pub struct Storage {
    operator: Client,
}

impl Storage {
    pub async fn new(endpoints: Vec<String>) -> Self {
        info!(" etcd endpoints: {:?}", endpoints);
        let operator = Client::connect(
            &endpoints,
            Some(
                ConnectOptions::new()
                    .with_connect_timeout(Duration::from_secs(get_config().rpc_timeout))
                    .with_keep_alive(
                        Duration::from_secs(300),
                        Duration::from_secs(get_config().rpc_timeout),
                    )
                    .with_keep_alive_while_idle(true)
                    .with_timeout(Duration::from_secs(get_config().rpc_timeout)),
            ),
        )
        .await
        .map_err(|e| println!("etcd connect failed: {e}"))
        .unwrap();
        Self { operator }
    }

    pub async fn put_with_lease(
        &self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
        ttl: i64,
    ) -> Result<()> {
        let mut storage = self.operator.clone();
        let lease = storage.lease_grant(ttl, None).await?;
        let option = PutOptions::new().with_lease(lease.id());
        storage.put(key, value, Some(option)).await?;
        Ok(())
    }

    pub async fn get(&self, key: impl Into<Vec<u8>>) -> Result<Vec<u8>> {
        let mut storage = self.operator.clone();
        let data_vec = storage.get(key, None).await?;
        if let Some(kv) = data_vec.kvs().first() {
            Ok(kv.value().to_vec())
        } else {
            Err(eyre!("data not found"))
        }
    }
}

macro_rules! store_and_load {
    ($vis:ident, $data_type:ty, $var_name:ident, $dir:expr) => {
        paste! {
            impl Storage {
                pub($vis) async fn [<store_$var_name>](&self, init_hash: &str, data: &$data_type) -> Result<()> {
                    let json = serde_json::to_string(&data)?;
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
                    self.operator
                        .clone()
                        .put(path, json, None)
                        .await
                        .map_err(|e| eyre!(e.to_string())).map(|_| ())
                }

                pub($vis) async fn [<load_$var_name>](&self, init_hash: &str) -> Result<$data_type> {
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
                    let data_vec = self
                        .operator
                        .clone()
                        .get(path, None)
                        .await
                        .map_err(|e| eyre!(e.to_string()))?;
                    if let Some(kv) = data_vec.kvs().first() {
                        let data = serde_json::from_slice::<$data_type>(kv.value())?;
                        Ok(data)
                    } else {
                        Err(eyre!("data not found"))
                    }
                }

                #[allow(unused)]
                pub($vis) async fn [<delete_$var_name>](&self, init_hash: &str) -> Result<()> {
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
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

store_and_load!(crate, BaseData, base_data, "task");
store_and_load!(crate, Timeout, timeout, "task");
store_and_load!(crate, Gas, gas, "task");
store_and_load!(crate, Status, status, "task");
store_and_load!(crate, HashToCheck, hash_to_check, "processing");
store_and_load!(
    crate,
    RawTransactionBytes,
    raw_transaction_bytes,
    "processing"
);
store_and_load!(crate, TaskResult, task_result, "result");

impl Storage {
    pub async fn store_send_task(&self, init_hash: &str, task: &SendTask) -> Result<()> {
        self.store_base_data(init_hash, &task.base_data).await?;
        self.store_timeout(init_hash, &task.timeout).await?;
        self.store_gas(init_hash, &task.gas).await?;
        if let Some(raw_transaction_bytes) = &task.raw_transaction_bytes {
            self.store_raw_transaction_bytes(init_hash, raw_transaction_bytes)
                .await?;
        }
        self.store_status(init_hash, &Status::Unsend).await?;

        Ok(())
    }

    pub async fn load_send_task(&self, init_hash: &str) -> Result<SendTask> {
        let send_task = SendTask {
            base_data: self.load_base_data(init_hash).await?,
            timeout: self.load_timeout(init_hash).await?,
            gas: self.load_gas(init_hash).await?,
            raw_transaction_bytes: self.load_raw_transaction_bytes(init_hash).await.ok(),
        };

        Ok(send_task)
    }

    pub async fn load_check_task(&self, init_hash: &str) -> Result<CheckTask> {
        let base_data = self.load_base_data(init_hash).await?;
        let hash_to_check = self.load_hash_to_check(init_hash).await?;

        let check_task = CheckTask {
            base_data,
            hash_to_check,
        };

        Ok(check_task)
    }

    pub async fn load_task(&self, init_hash: &str) -> Result<Task> {
        let base_data = self.load_base_data(init_hash).await?;
        let timeout = self.load_timeout(init_hash).await?;
        let gas = self.load_gas(init_hash).await?;
        let status = self
            .load_status(init_hash)
            .await
            .unwrap_or(Status::Completed);

        let task = Task {
            base_data,
            init_hash: init_hash.to_owned(),
            status,
            timeout,
            gas,
            result: self.load_task_result(init_hash).await.ok(),
        };

        Ok(task)
    }

    pub async fn downgrade_to_unsend(&self, init_hash: &str) -> Result<()> {
        self.delete_hash_to_check(init_hash).await?;
        // need rebuild the transaction
        self.delete_raw_transaction_bytes(init_hash).await?;
        self.store_status(init_hash, &Status::Unsend).await?;
        Ok(())
    }

    pub async fn finalize_task(&self, init_hash: &str, auto_tx_result: &TaskResult) -> Result<()> {
        // delete all processing path
        self.delete_hash_to_check(init_hash).await?;
        self.delete_raw_transaction_bytes(init_hash).await?;
        self.delete_status(init_hash).await?;

        self.store_task_result(init_hash, auto_tx_result).await?;

        Ok(())
    }

    pub async fn get_processing_tasks(&self) -> Result<Vec<String>> {
        let config = get_config();
        // add limit for OutOfRange error
        let option = GetOptions::new()
            .with_prefix()
            .with_keys_only()
            .with_limit(config.etcd_get_limit);
        let entries = self
            .operator
            .clone()
            .get(format!("{}/task/status/", config.name), Some(option))
            .await?;
        let keys = entries
            .kvs()
            .iter()
            .filter_map(|e| {
                if let Ok(key_str) = e.key_str() {
                    key_str.split('/').last().map(|s| s.to_owned())
                } else {
                    None
                }
            })
            .collect();

        Ok(keys)
    }

    pub async fn try_lock_task(&self, init_hash: &str) -> Result<Vec<u8>> {
        let config = get_config();
        let mut write = self.operator.clone();
        let lease = write.lease_grant(config.max_timeout.into(), None).await?;
        let option = LockOptions::new().with_lease(lease.id());
        let key = format!("{}/locked_task/{}", config.name, init_hash);
        let lock_key = write.lock(key, Some(option)).await?.key().to_vec();
        Ok(lock_key)
    }

    pub async fn unlock_task(&self, lock_key: &[u8]) -> Result<()> {
        let mut write = self.operator.clone();
        write.unlock(lock_key).await?;
        Ok(())
    }
}
