use anyhow::{anyhow, Result};
use opendal::{services::Sled, EntryMode, Operator};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::Value};

use crate::{send_tx::AutoTxType, util::add_0x};

fn tx_path(key: &str) -> String {
    "processing/".to_string() + key
}

fn done_path(key: &str) -> String {
    "done/".to_string() + key
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoTxResult {
    SuccessInfo(SuccessInfo),
    FailedInfo(FailedInfo),
}
impl AutoTxResult {
    pub fn success(hash: String, contract_address: Option<String>) -> Self {
        Self::SuccessInfo(SuccessInfo {
            hash,
            contract_address,
        })
    }

    pub fn failed(hash: String, info: String) -> Self {
        Self::FailedInfo(FailedInfo { hash, info })
    }

    pub fn to_json(self) -> Value {
        match self {
            AutoTxResult::SuccessInfo(s) => match s.contract_address {
                Some(addr) => json!({
                    "is_success": true,
                    "onchain_hash": add_0x(s.hash),
                    "contract_address": add_0x(addr),
                }),
                None => json!({
                    "is_success": true,
                    "onchain_hash": add_0x(s.hash),
                }),
            },
            AutoTxResult::FailedInfo(f) => json!({
                "is_success": false,
                "last_hash": add_0x(f.hash),
                "err": f.info,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessInfo {
    hash: String,
    contract_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedInfo {
    hash: String,
    info: String,
}

#[axum::async_trait]
pub trait AutoTxStorage {
    async fn insert_processing(&self, key: &str, tx: AutoTxType) -> Result<()>;
    async fn get_all_processing(&self) -> Result<Vec<AutoTxType>>;
    async fn insert_done(&self, key: &str, result: AutoTxResult) -> Result<()>;
    async fn get_done(&self, key: &str) -> Result<AutoTxResult>;
}

#[derive(Clone, Debug)]
pub struct Storage {
    operator: Operator,
}

impl Storage {
    pub fn new(datadir: String) -> Self {
        info!("auto_tx datadir: {}", datadir);
        let mut builder = Sled::default();
        builder.datadir(&datadir);
        let operator = Operator::new(builder).unwrap().finish();
        Self { operator }
    }
}

#[axum::async_trait]
impl AutoTxStorage for Storage {
    async fn insert_processing(&self, key: &str, tx: AutoTxType) -> Result<()> {
        let tx_vec = serde_json::to_vec(&tx)?;
        self.operator.write(&tx_path(key), tx_vec).await?;
        Ok(())
    }

    async fn get_all_processing(&self) -> Result<Vec<AutoTxType>> {
        let operator = self.operator.blocking();
        let paths = self
            .operator
            .list(&tx_path(""))
            .await?
            .into_iter()
            .filter(|e| e.metadata().mode() == EntryMode::FILE)
            .map(|e| e.path().to_string())
            .collect::<Vec<String>>();
        let mut auto_txs = vec![];
        for tx_path in paths {
            let tx_vec = operator.read(&tx_path)?;
            let tx = serde_json::from_slice(&tx_vec)?;
            auto_txs.push(tx);
        }
        Ok(auto_txs)
    }

    async fn insert_done(&self, key: &str, result: AutoTxResult) -> Result<()> {
        self.operator.delete(&tx_path(key)).await?;

        let result_vec = serde_json::to_vec(&result)?;
        self.operator.write(&done_path(key), result_vec).await?;

        Ok(())
    }

    async fn get_done(&self, key: &str) -> Result<AutoTxResult> {
        let result_vec = self
            .operator
            .read(&done_path(key))
            .await
            .map_err(|_| anyhow!("not found"))?;
        let result = serde_json::from_slice(&result_vec)?;
        Ok(result)
    }
}
