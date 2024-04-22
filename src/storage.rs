use crate::{config::get_config, hlc, task::*};
use color_eyre::eyre::{eyre, Result};
use common_rs::redis::{
    streams, AsyncCommands, ExistenceCheck, Redis, RedisConnection, SetExpiry, SetOptions,
};
use paste::paste;

#[derive(Clone)]
pub struct Storage {
    operator: Redis,
}

impl Storage {
    pub async fn new(operator: Redis) -> Self {
        Self { operator }
    }

    pub fn operator(&self) -> RedisConnection {
        self.operator.conn()
    }
}

macro_rules! store_and_load {
    ($vis:ident, $data_type:ty, $var_name:ident, $dir:expr) => {
        paste! {
            impl Storage {
                pub($vis) async fn [<store_$var_name>](&self, init_hash: &str, data: &$data_type) -> Result<()> {
                    let json = serde_json::to_string(&data)?;
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
                    self.operator()
                        .set(path, json)
                        .await
                        .map_err(|e| eyre!(e.to_string()))
                }

                pub($vis) async fn [<load_$var_name>](&self, init_hash: &str) -> Result<$data_type> {
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
                    let data_vec: String = self
                        .operator()
                        .get(path)
                        .await
                        .map_err(|e| eyre!(e.to_string()))?;
                    let data = serde_json::from_str::<$data_type>(&data_vec)?;
                    Ok(data)
                }

                #[allow(unused)]
                pub($vis) async fn [<delete_$var_name>](&self, init_hash: &str) -> Result<()> {
                    let path = format!("{}/{}/{}/{}", get_config().name, $dir, stringify!($var_name), init_hash);
                    self.operator()
                        .del(path)
                        .await
                        .map_err(|e| eyre!(e.to_string()))
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
    pub async fn update_status(&self, init_hash: &str, status: &Status) -> Result<()> {
        self.store_status(init_hash, status).await?;
        let mut conn = self.operator();
        let key = format!("{}/processing/{:?}/", get_config().name, status);
        conn.xadd::<&str, &str, &str, &str, ()>(&key, "*", &[(init_hash, "")])
            .await?;

        Ok(())
    }

    pub async fn read_processing_task(&self, status: &Status) -> Result<Vec<String>> {
        let mut conn = self.operator();
        let config = get_config();
        let read_num = match status {
            Status::Uncheck => config.read_check_num,
            Status::Unsend => config.read_send_num,
            _ => 0,
        };

        let keys = &[&format!("{}/processing/{:?}/", config.name, status)];

        let _xgroup_create_result: Result<(), _> = conn
            .xgroup_create_mkstream(keys[0], &config.name, "0-0")
            .await
            .map_err(|e| debug!("xgroup create error: {}", e));

        let opts = streams::StreamReadOptions::default()
            .group(config.name.clone(), format!("{}", hlc().get_id()))
            .count(read_num);

        let iter: streams::StreamReadReply =
            conn.xread_options(keys, &[">"], &opts).await.map_err(|e| {
                debug!("xread error: {}", e);
                e
            })?;

        let mut tasks = vec![];

        for key in iter.keys {
            if &key.key == keys[0] {
                tasks.extend(key.ids.iter().flat_map(|id| {
                    id.map
                        .keys()
                        .filter_map(|k| k.split('/').map(|k| k.to_string()).last())
                }));
                continue;
            }
        }

        Ok(tasks)
    }

    pub async fn store_send_task(&self, init_hash: &str, task: &SendTask) -> Result<()> {
        self.store_base_data(init_hash, &task.base_data).await?;
        self.store_timeout(init_hash, &task.timeout).await?;
        self.store_gas(init_hash, &task.gas).await?;
        if let Some(raw_transaction_bytes) = &task.raw_transaction_bytes {
            self.store_raw_transaction_bytes(init_hash, raw_transaction_bytes)
                .await?;
        }
        self.update_status(init_hash, &Status::Unsend).await?;

        Ok(())
    }

    pub async fn load_send_task(&self, init_hash: &str) -> Result<SendTask> {
        if let Ok(Status::Unsend) = self.load_status(init_hash).await {
            let send_task = SendTask {
                base_data: self.load_base_data(init_hash).await?,
                timeout: self.load_timeout(init_hash).await?,
                gas: self.load_gas(init_hash).await?,
                raw_transaction_bytes: self.load_raw_transaction_bytes(init_hash).await.ok(),
            };
            Ok(send_task)
        } else {
            Err(eyre!("task status is not Unsend"))
        }
    }

    pub async fn load_check_task(&self, init_hash: &str) -> Result<CheckTask> {
        if let Ok(Status::Uncheck) = self.load_status(init_hash).await {
            let base_data = self.load_base_data(init_hash).await?;
            let hash_to_check = self.load_hash_to_check(init_hash).await?;

            let check_task = CheckTask {
                base_data,
                hash_to_check,
            };
            Ok(check_task)
        } else {
            Err(eyre!("task status is not Uncheck"))
        }
    }

    pub async fn load_task(&self, init_hash: &str) -> Result<Task> {
        let base_data = self.load_base_data(init_hash).await?;
        let timeout = self.load_timeout(init_hash).await?;
        let gas = self.load_gas(init_hash).await?.gas;
        let hash_to_check = self.load_hash_to_check(init_hash).await.ok();
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
            hash_to_check,
        };

        Ok(task)
    }

    pub async fn downgrade_to_unsend(&self, init_hash: &str) -> Result<()> {
        self.delete_hash_to_check(init_hash).await?;
        // need rebuild the transaction
        self.delete_raw_transaction_bytes(init_hash).await?;
        self.update_status(init_hash, &Status::Unsend).await?;
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

    pub async fn try_lock_task(&self, init_hash: &str) -> Result<()> {
        let config = get_config();
        let mut conn = self.operator();
        let key = format!("{}/locked_task/{}", config.name, init_hash);
        let options = SetOptions::default()
            .with_expiration(SetExpiry::EX((config.rpc_timeout * 2) as usize))
            .conditional_set(ExistenceCheck::NX);
        Ok(conn.set_options::<String, u8, ()>(key, 0, options).await?)
    }

    pub async fn unlock_task(&self, init_hash: &str) -> Result<()> {
        let config = get_config();
        let mut conn = self.operator();
        let key = format!("{}/locked_task/{}", config.name, init_hash);
        Ok(conn.del(key).await?)
    }

    pub async fn store_init_hash_by_request_key(
        &self,
        request_key: &str,
        init_hash: &str,
    ) -> Result<()> {
        let mut storage = self.operator();
        let config = get_config();
        let path = format!("{}/init_hash_by_request_key/{}", config.name, request_key);
        storage
            .set_ex(path, init_hash, config.request_key_ttl)
            .await?;
        Ok(())
    }

    pub async fn load_init_hash_by_request_key(&self, request_key: &str) -> Result<String> {
        let path = format!(
            "{}/init_hash_by_request_key/{}",
            get_config().name,
            request_key
        );
        self.operator()
            .get(path)
            .await
            .map_err(|e| eyre!("data not found: {e}"))
    }
}
