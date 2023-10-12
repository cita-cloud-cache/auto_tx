pub mod cita;
pub mod cita_cloud;
pub mod eth;

use crate::kms::{Account, Kms};
use crate::send_tx::cita::CitaAutoTx;
use crate::send_tx::cita_cloud::CitaCloudAutoTx;
use crate::send_tx::eth::EthAutoTx;
use crate::storage::{AutoTxStorage, Storage};
use crate::util::{add_0x, display_value, parse_data, parse_value};
use crate::RequestParams;
use crate::{chains::*, AutoTxGlobalState};
use anyhow::anyhow;
use anyhow::Result;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use common_rs::restful::{ok, RESTfulError};
use ethabi::ethereum_types::U256;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Display;
use std::sync::Arc;

#[axum::async_trait]
pub trait AutoTx: Clone {
    fn get_tag(&self) -> &AutoTxTag;

    fn set_tag(&mut self, tag: AutoTxTag);

    fn get_key(&self) -> String;

    fn get_current_hash(&self) -> String;

    fn to_unified_type(&self) -> AutoTxType;

    async fn store_unsend(&mut self, storage: &Storage) -> Result<()> {
        self.set_tag(AutoTxTag::Unsend);
        let key = &self.get_key();
        let unified_type = self.to_unified_type();
        storage.insert_processing(key, unified_type.clone()).await?;
        warn!("store_unsend: {:?}", unified_type);
        Ok(())
    }

    async fn store_uncheck(&mut self, storage: &Storage) -> Result<()> {
        self.set_tag(AutoTxTag::Uncheck);
        let key = &self.get_key();
        let unified_type = self.to_unified_type();
        storage.insert_processing(key, unified_type).await?;
        Ok(())
    }

    async fn store_done(&mut self, storage: &Storage, err: Option<String>) -> Result<()> {
        let key = &self.get_key();
        if let Some(e) = err {
            warn!("task: {} failed: {}", self.get_key(), e);
            storage.insert_done(key, e).await?;
        } else {
            let hash = self.get_current_hash();
            info!("task: {} success: {}", self.get_key(), hash);
            storage.insert_done(key, hash).await?;
        }
        Ok(())
    }

    async fn estimate_gas(&mut self, chains: &Chains) -> Result<()>;

    async fn update_args(&mut self, state: &AutoTxGlobalState) -> Result<Option<String>>;

    async fn init_unsend(&mut self, state: Arc<AutoTxGlobalState>) -> Result<String> {
        self.estimate_gas(&state.chains).await?;
        let hash = self
            .update_args(&state)
            .await?
            .ok_or(anyhow!("init failed"))?;
        self.store_unsend(&state.storage).await?;

        Ok(hash)
    }

    async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()>;

    async fn check(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInfo {
    from: Vec<u8>,
    to: Vec<u8>,
    data: Vec<u8>,
    value: (Vec<u8>, U256),
}

impl Display for TxInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data_len = self.data.len();
        let data = if data_len > 10 {
            add_0x(hex::encode(&self.data.clone()[..4]))
                + "..."
                + &hex::encode(&self.data.clone()[(data_len - 4)..data_len])
        } else {
            add_0x(hex::encode(self.data.clone()))
        };

        let value_str = hex::encode(self.value.0.clone());
        let display_value = display_value(&value_str).unwrap();

        write!(
            f,
            "from: {}, to: {}, data: {}, value: {}",
            add_0x(hex::encode(self.from.clone())),
            add_0x(hex::encode(self.to.clone())),
            data,
            display_value,
        )
    }
}

impl TxInfo {
    fn new(from: Vec<u8>, to: Vec<u8>, data: Vec<u8>, value_u256: U256, value: Vec<u8>) -> Self {
        Self {
            from,
            to,
            data,
            value: (value, value_u256),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoTxInfo {
    req_key: String,
    chain_name: String,
    account: Account,
    tx_info: TxInfo,
}

impl AutoTxInfo {
    pub const fn new(
        req_key: String,
        chain_name: String,
        account: Account,
        tx_info: TxInfo,
    ) -> Self {
        Self {
            req_key,
            chain_name,
            account,
            tx_info,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoTxTag {
    Unsend,
    Uncheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum AutoTxType {
    CitaCloud(CitaCloudAutoTx),
    Cita(CitaAutoTx),
    Eth(EthAutoTx),
}

impl AutoTxType {
    pub fn get_tag(&self) -> &AutoTxTag {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.get_tag(),
            AutoTxType::Cita(auto_tx) => auto_tx.get_tag(),
            AutoTxType::Eth(auto_tx) => auto_tx.get_tag(),
        }
    }

    pub async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.send(state).await,
            AutoTxType::Cita(auto_tx) => auto_tx.send(state).await,
            AutoTxType::Eth(auto_tx) => {
                warn!("get eth unsend");
                auto_tx.send(state).await
            }
        }
    }

    pub async fn check(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.check(state).await,
            AutoTxType::Cita(auto_tx) => auto_tx.check(state).await,
            AutoTxType::Eth(auto_tx) => auto_tx.check(state).await,
        }
    }
}

pub async fn handle_send_tx(
    headers: HeaderMap,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    warn!("params: {:?}", params);

    // get req_key
    let req_key = headers
        .get("key")
        .ok_or(anyhow::anyhow!("no key in header"))?
        .to_str()?;

    // check params
    if params.user_code.is_empty() {
        return Err(anyhow::anyhow!("user_code missing").into());
    }
    if params.chain_name.is_empty() {
        return Err(anyhow::anyhow!("chain_name missing").into());
    }
    if params.to.is_empty() {
        return Err(anyhow::anyhow!("field \"to\" missing").into());
    }
    if params.data.is_empty() {
        return Err(anyhow::anyhow!("field \"data\" missing").into());
    }

    // get timeout
    let timeout = {
        if let Some(timeout) = params.timeout {
            timeout.min(state.max_timeout)
        } else {
            state.max_timeout
        }
    };

    // get ChainInfo
    let chain_name = params.chain_name.clone();
    let chain_info = state.chains.get_chain_info(&chain_name).await?;

    // get account
    let account = Account::new(params.user_code.clone(), chain_info.crypto_type.clone()).await?;

    // convert tx field
    let from = account.address();
    let to = parse_data(&params.to)?;
    let data = parse_data(&params.data)?;
    let value_u256 = U256::from_dec_str(&params.value)?;
    let value = parse_value(&params.value)?;
    let tx_info = TxInfo::new(from, to, data, value_u256, value);

    let auto_tx_info = AutoTxInfo::new(req_key.to_string(), chain_name, account, tx_info.clone());

    let hash = match chain_info.chain_type {
        ChainType::CitaCloud(_) => {
            let mut cita_cloud_auto_tx = CitaCloudAutoTx::new(auto_tx_info, timeout);
            cita_cloud_auto_tx.init_unsend(state.clone()).await.unwrap()
        }
        ChainType::Cita(_) => {
            let mut cita_auto_tx = CitaAutoTx::new(auto_tx_info, timeout);
            cita_auto_tx.init_unsend(state.clone()).await.unwrap()
        }
        ChainType::Eth(_) => {
            let mut eth_auto_tx = EthAutoTx::new(auto_tx_info);
            eth_auto_tx.init_unsend(state.clone()).await.unwrap()
        }
    };

    info!(
        "receive send_tx request: req_key: {}, user_code: {}\n\tChainInfo: {}\n\tTxInfo: {}\n\tinitial hash: 0x{}",
        req_key, params.user_code, chain_info, tx_info, hash.clone()
    );

    ok(json!({
        "hash": add_0x(hash)
    }))
}
