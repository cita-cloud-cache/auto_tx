use crate::{chains::ChainInfo, config::get_config, task::*};
use color_eyre::eyre::{eyre, OptionExt, Result};
use paste::paste;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Storage {
    pub url: String,
    pub prefix: String,
}

impl Storage {
    pub async fn post(&self, key: &str, data: &Value) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!("{}/kvs/w/{}/{}", self.url, self.prefix, key);

        let resp = client
            .post(url)
            .json(data)
            .send()
            .await
            .map_err(|e| eyre!("storage post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("storage post failed"))
        }
    }

    pub async fn get_struct<T: Default + bevy_reflect::Struct + for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<T> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!("{}/kvs/r/{}/{}", self.url, self.prefix, key);
        let mut fields = vec![];
        let d = T::default();
        for i in 0..d.field_len() {
            if let Some(field) = d.name_at(i) {
                fields.push(field);
            }
        }

        let resp = client
            .post(url)
            .json(&json!(fields))
            .send()
            .await
            .map_err(|e| eyre!("storage get http failed: {e}"))?;
        debug!("get_struct resp: {:?}", resp);
        resp.json::<T>()
            .await
            .map_err(|e| eyre!("storage get struct failed: {e}"))
    }
}

macro_rules! store_and_load {
    ($vis:ident, $data_type:ty, $var_name:expr) => {
        paste! {
            impl Storage {
                #[allow(unused)]
                pub($vis) async fn [<store_$var_name>](&self, init_hash: &str, data: &$data_type) -> Result<()> {
                    let url = format!("{}/kvs/w/{}/{}", self.url, self.prefix, init_hash);
                    let data = serde_json::json!({
                        $var_name: data,
                    });
                    let resp = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()?
                        .post(url)
                        .json(&data)
                        .send()
                        .await
                        .map_err(|e| eyre!("store_{} post failed: {e}", $var_name))?;
                    if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
                        Ok(())
                    } else {
                        Err(eyre!("store_{} failed", $var_name))
                    }
                }

                #[allow(unused)]
                pub($vis) async fn [<load_$var_name>](&self, init_hash: &str) -> Result<$data_type> {
                    let url = format!("{}/kv/{}/{}/{}", self.url, self.prefix, init_hash, $var_name);
                    let resp = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()?
                        .get(url)
                        .send()
                        .await
                        .map_err(|e| eyre!("load_{} get failed: {e}", $var_name))?;
                    if resp.status().is_success() {
                        let data = resp.json::<$data_type>().await
                            .map_err(|e| eyre!("load_{} failed: {e}", $var_name))?;
                        Ok(data)
                    } else {
                        Err(eyre!("load_{} failed", $var_name))
                    }
                }

                #[allow(unused)]
                pub($vis) async fn [<delete_$var_name>](&self, init_hash: &str) -> Result<()> {
                    let url = format!("{}/kvs/w/{}/{}", self.url, self.prefix,  init_hash);
                    let data = serde_json::json!({
                        $var_name: Value::Null,
                    });
                    let resp = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()?
                        .post(url)
                        .json(&data)
                        .send()
                        .await
                        .map_err(|e| eyre!("delete_{} post failed: {e}", $var_name))?;
                    if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
                        Ok(())
                    } else {
                        Err(eyre!("delete_{} failed", $var_name))
                    }
                }
            }
        }
    };
}

store_and_load!(crate, BaseData, "base_data");
store_and_load!(crate, Timeout, "timeout");
store_and_load!(crate, Gas, "gas");
store_and_load!(crate, Status, "status");
store_and_load!(crate, HashToCheck, "hash_to_check");
store_and_load!(crate, RawTransactionBytes, "raw_transaction_bytes");
store_and_load!(crate, TaskResult, "task_result");

impl Storage {
    pub async fn update_status(&self, init_hash: &str, status: &Status) -> Result<()> {
        self.store_status(init_hash, status).await?;
        // send event
        self.send_processing_task(init_hash, status).await
    }

    pub async fn send_processing_task(&self, init_hash: &str, status: &Status) -> Result<()> {
        let status_path = match status {
            Status::Unsend => "unsend",
            Status::Uncheck => "uncheck",
            Status::Completed => return Err(eyre!("task status is completed")),
        };
        let url = format!(
            "{}/kvs/w/{}/processing/{}",
            self.url, self.prefix, status_path
        );
        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?
            .post(url)
            .json(&json!({ init_hash: 0 }))
            .send()
            .await
            .map_err(|e| eyre!("send_processing_task post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("send_processing_task failed"))
        }
    }

    pub async fn read_processing_task(&self, status: &Status) -> Result<Vec<String>> {
        let config = get_config();
        let (max_num, status_path) = match status {
            Status::Unsend => (config.read_send_num, "unsend"),
            Status::Uncheck => (config.read_check_num, "uncheck"),
            Status::Completed => return Err(eyre!("task status is completed")),
        };

        let url = format!(
            "{}/scan/{}/{}/processing/{}",
            self.url, max_num, self.prefix, status_path
        );
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?
            .post(url)
            .send()
            .await
            .map_err(|e| eyre!("read_processing_task post http failed: {e}"))?
            .json::<Vec<String>>()
            .await
            .map_err(|e| eyre!("read_processing_task decode failed: {e}"))
    }

    pub async fn delete_processing_task(&self, init_hash: &str) -> Result<()> {
        let url = format!("{}/kvs/w/{}/processing", self.url, self.prefix);
        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?
            .post(url)
            .json(&json!({
                format!("unsend/{}", init_hash): Value::Null,
                format!("uncheck/{}", init_hash): Value::Null
            }))
            .send()
            .await
            .map_err(|e| eyre!("delete_processing_task post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("delete_processing_task failed"))
        }
    }

    pub async fn store_send_task(&self, init_hash: &str, task: &SendTask) -> Result<()> {
        let mut data = serde_json::to_value(task)?;
        if let Some(raw_transaction_bytes) = &task.raw_transaction_bytes {
            data[format!("raw_transaction_bytes/{}", init_hash)] = json!(raw_transaction_bytes);
        }
        self.post(init_hash, &data).await?;
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
            Err(eyre!("task {init_hash} is not Unsend"))
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
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!("{}/kvs/r/{}/{}", self.url, self.prefix, init_hash);

        let resp = client
            .post(url)
            .json(&json!([
                "base_data",
                "status",
                "timeout",
                "gas",
                "hash_to_check",
                "task_result",
            ]))
            .send()
            .await
            .map_err(|e| {
                error!("storage get http failed: {e}");
                eyre!("storage get http failed: {e}")
            })?;
        resp.json::<Value>()
            .await
            .map_err(|e| eyre!("storage get task failed: {e}"))
            .map(|j| Task {
                init_hash: init_hash.to_owned(),
                base_data: serde_json::from_value(j["base_data"].clone()).unwrap(),
                status: serde_json::from_value(j["status"].clone()).unwrap_or(Status::Completed),
                timeout: serde_json::from_value(j["timeout"].clone()).unwrap(),
                gas: serde_json::from_value(j["gas"]["gas"].clone()).unwrap(),
                result: serde_json::from_value(j["task_result"].clone()).ok(),
                hash_to_check: serde_json::from_value(j["hash_to_check"].clone()).ok(),
            })
    }

    pub async fn downgrade_to_unsend(&self, init_hash: &str) -> Result<()> {
        self.delete_hash_to_check(init_hash).await?;
        // need rebuild the transaction
        self.delete_raw_transaction_bytes(init_hash).await?;
        self.update_status(init_hash, &Status::Unsend).await?;
        Ok(())
    }

    pub async fn finalize_task(&self, init_hash: &str, auto_tx_result: &TaskResult) -> Result<()> {
        let url = format!("{}/kvs/w/{}/{}", self.url, self.prefix, init_hash);

        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?
            .post(url)
            .json(&json!({
                "hash_to_check": Value::Null,
                "raw_transaction_bytes": Value::Null,
                "status": Value::Null
            }))
            .send()
            .await
            .map_err(|e| eyre!("storage post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            self.store_task_result(init_hash, auto_tx_result).await?;
            self.delete_processing_task(init_hash).await
        } else {
            Err(eyre!("storage post failed"))
        }
    }

    pub async fn try_lock_task(&self, init_hash: &str) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!("{}/cas/{}/locked_task/{}", self.url, self.prefix, init_hash);

        let resp = client
            .post(url)
            .json(&json!({ "new": "true" }))
            .send()
            .await
            .map_err(|e| eyre!("try_lock_task post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("try_lock_task failed"))
        }
    }

    pub async fn unlock_task(&self, init_hash: &str) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!("{}/cas/{}/locked_task/{}", self.url, self.prefix, init_hash);

        let resp = client
            .post(url)
            .json(&json!({ "old": "true" }))
            .send()
            .await
            .map_err(|e| eyre!("try_lock_task post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("try_lock_task failed"))
        }
    }

    pub async fn store_init_hash_by_request_key(
        &self,
        request_key: &str,
        init_hash: &str,
    ) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!(
            "{}/kvs/w/{}/init_hash_by_request_key/{}",
            self.url, self.prefix, request_key
        );

        let resp = client
            .post(url)
            .json(&json!({ "init_hash": init_hash }))
            .send()
            .await
            .map_err(|e| eyre!("storage post http failed: {e}"))?;
        if resp.status().is_success() && resp.json::<bool>().await.is_ok() {
            Ok(())
        } else {
            Err(eyre!("storage post failed"))
        }
    }

    pub async fn load_init_hash_by_request_key(&self, request_key: &str) -> Result<String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!(
            "{}/kvs/r/{}/init_hash_by_request_key/{}",
            self.url, self.prefix, request_key
        );

        let resp = client
            .post(url)
            .json(&json!(["init_hash"]))
            .send()
            .await
            .map_err(|e| eyre!("storage get http failed: {e}"))?;
        resp.json::<Value>()
            .await
            .map_err(|e| eyre!("storage get struct failed: {e}"))?
            .get("init_hash")
            .ok_or_eyre("storage get struct failed")?
            .as_str()
            .map(|r| r.to_owned())
            .ok_or_eyre("storage get struct failed")
    }

    pub async fn get_chain_info(&self, chain_name: &str) -> Result<ChainInfo> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let url = format!(
            "{}/kvs/r/{}/@@/ChainInfo/{}",
            self.url, self.prefix, chain_name
        );
        let fields = vec!["chain_type", "crypto_type", "chain_url"];

        let resp = client
            .post(url)
            .json(&json!(fields))
            .send()
            .await
            .map_err(|e| eyre!("storage get http failed: {e}"))?;
        debug!("get_struct resp: {:?}", resp);
        resp.json::<ChainInfo>()
            .await
            .map_err(|e| eyre!("storage get struct failed: {e}"))
    }
}

#[tokio::test]
async fn test_json() -> Result<()> {
    let init_hash = "e1aee3a37fa4111bbccc4ba1b157ff4bb1b77f2336ef419e2213c3eccef07da6";
    let client = reqwest::Client::new();
    let url = format!(
        "http://localhost:3000/api/kvs/r/auto-api-latest/{}",
        init_hash
    );
    let resp = client
        .post(url)
        .json(&json!([
            "base_data",
            "status",
            "timeout",
            "gas",
            "hash_to_check",
            "result",
        ]))
        .send()
        .await
        .map_err(|e| {
            error!("storage get http failed: {e}");
            eyre!("storage get http failed: {e}")
        })?;
    let r = resp
        .json::<Value>()
        .await
        .map_err(|e| eyre!("storage get task failed: {e}"))
        .map(|j| Task {
            init_hash: init_hash.to_owned(),
            base_data: serde_json::from_value(j["base_data"].clone()).unwrap(),
            status: serde_json::from_value(j["status"].clone()).unwrap(),
            timeout: serde_json::from_value(j["timeout"].clone()).unwrap(),
            gas: serde_json::from_value(j["gas"]["gas"].clone()).unwrap(),
            result: serde_json::from_value(j["result"].clone()).ok(),
            hash_to_check: serde_json::from_value(j["hash_to_check"].clone()).ok(),
        });

    println!("{:#?}", r);
    Ok(())
}
