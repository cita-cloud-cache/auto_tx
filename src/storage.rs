use anyhow::Result;
use opendal::{services::Sled, EntryMode, Operator};

use crate::send_tx::AutoTxType;

fn tx_path(key: &str) -> String {
    "processing/".to_string() + key
}

fn done_path(key: &str) -> String {
    "done/".to_string() + key
}

#[axum::async_trait]
pub trait AutoTxStorage {
    async fn insert_processing(&self, key: &str, tx: AutoTxType) -> Result<()>;
    async fn get_all_processing(&self) -> Result<Vec<AutoTxType>>;
    async fn insert_done(&self, key: &str, info: String) -> Result<()>;
    async fn get_done(&self, key: &str) -> Result<String>;
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

    async fn insert_done(&self, key: &str, info: String) -> Result<()> {
        self.operator.delete(&tx_path(key)).await?;

        self.operator
            .write(&done_path(key), info.as_bytes().to_owned())
            .await?;

        Ok(())
    }

    async fn get_done(&self, key: &str) -> Result<String> {
        let info_vec = self.operator.read(&done_path(key)).await?;
        let info = String::from_utf8_lossy(&info_vec).to_string();
        Ok(info)
    }
}
