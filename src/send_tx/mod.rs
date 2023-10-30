pub mod cita;
pub mod cita_cloud;
pub mod eth;

use crate::{
    chains::*,
    kms::Account,
    send_tx::{cita::CitaAutoTx, cita_cloud::CitaCloudAutoTx, eth::EthAutoTx},
    storage::{AutoTxStorage, Storage},
    util::{add_0x, display_value, parse_data, parse_value},
    AutoTxGlobalState, RequestParams,
};
use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    Json,
};
use common_rs::restful::{ok, RESTfulError};
use ethabi::ethereum_types::U256;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fmt::Display, sync::Arc};

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
            storage.insert_done(key, e).await?;
        } else {
            let hash = self.get_current_hash();
            storage.insert_done(key, hash).await?;
        }
        Ok(())
    }

    async fn update_gas(&mut self, chains: &Chains, self_update: bool) -> Result<()>;

    async fn update_tx_if_timeout(&mut self, state: &AutoTxGlobalState) -> Result<bool>;

    async fn update_current_hash(&mut self, chains: &Chains) -> Result<String>;

    async fn init_unsend(&mut self, state: Arc<AutoTxGlobalState>) -> Result<String> {
        self.update_gas(&state.chains, false).await?;
        if self.update_tx_if_timeout(&state).await? {
            let hash = self.update_current_hash(&state.chains).await?;
            self.store_unsend(&state.storage).await?;
            Ok(hash)
        } else {
            Err(anyhow!("init failed: update_tx_if_timeout is false"))
        }
    }

    async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()>;

    async fn check(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxData {
    to: Vec<u8>,
    data: Vec<u8>,
    value: (Vec<u8>, U256),
}

impl Display for TxData {
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
            "to: {}, data: {}, value: {}",
            add_0x(hex::encode(self.to.clone())),
            data,
            display_value,
        )
    }
}

impl TxData {
    fn new(to: Vec<u8>, data: Vec<u8>, value_u256: U256, value: Vec<u8>) -> Self {
        Self {
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
}

impl AutoTxInfo {
    pub const fn new(req_key: String, chain_name: String, account: Account) -> Self {
        Self {
            req_key,
            chain_name,
            account,
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
    pub fn get_key(&self) -> String {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.get_key(),
            AutoTxType::Cita(auto_tx) => auto_tx.get_key(),
            AutoTxType::Eth(auto_tx) => auto_tx.get_key(),
        }
    }

    fn get_tag(&self) -> &AutoTxTag {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.get_tag(),
            AutoTxType::Cita(auto_tx) => auto_tx.get_tag(),
            AutoTxType::Eth(auto_tx) => auto_tx.get_tag(),
        }
    }

    async fn send(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.send(state).await,
            AutoTxType::Cita(auto_tx) => auto_tx.send(state).await,
            AutoTxType::Eth(auto_tx) => auto_tx.send(state).await,
        }
    }

    async fn check(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        match self {
            AutoTxType::CitaCloud(auto_tx) => auto_tx.check(state).await,
            AutoTxType::Cita(auto_tx) => auto_tx.check(state).await,
            AutoTxType::Eth(auto_tx) => auto_tx.check(state).await,
        }
    }

    pub async fn process(&mut self, state: Arc<AutoTxGlobalState>) -> Result<()> {
        let req_key = self.get_key();
        let processing = state.processing.clone();
        let mut write = processing.write().await;
        write.insert(req_key.clone());
        let tag = self.get_tag();
        let result = match tag {
            AutoTxTag::Unsend => self.send(state).await,
            AutoTxTag::Uncheck => self.check(state).await,
        };
        write.remove(&req_key);
        result
    }
}

pub async fn handle_send_tx(
    headers: HeaderMap,
    Path(chain_name): Path<String>,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    let req_key = headers
        .get("key")
        .ok_or_else(|| {
            let e = anyhow::anyhow!("no req_key in header");
            warn!("request failed: {}", e);
            e
        })?
        .to_str()?;

    handle(req_key, Path(chain_name), State(state), Json(params))
        .await
        .map_err(|e| {
            warn!("request: {} failed: {:?}", req_key, e);
            e
        })
}

pub async fn handle(
    req_key: &str,
    Path(chain_name): Path<String>,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    debug!("params: {:?}", params);

    // check params
    if params.user_code.is_empty() {
        return Err(anyhow::anyhow!("user_code missing").into());
    }
    if params.data.is_empty() {
        return Err(anyhow::anyhow!("field \"data\" missing").into());
    }

    let req_key = params.user_code.clone() + req_key;

    // get timeout
    let timeout = {
        if let Some(timeout) = params.timeout {
            (timeout.min(state.max_timeout).max(20) as f64 * 0.8) as u32
        } else {
            state.max_timeout
        }
    };

    // get ChainInfo
    let chain = state.chains.get_chain(&chain_name).await?;

    // get account
    let account = Account::new(
        params.user_code.clone(),
        chain.chain_info.crypto_type.clone(),
    )
    .await?;

    // convert tx field
    let to = parse_data(&params.to)?;
    let data = parse_data(&params.data)?;
    let value_u256 = U256::from_dec_str(&params.value)?;
    let value = parse_value(&params.value)?;
    let tx_data = TxData::new(to, data, value_u256, value);

    let auto_tx_info = AutoTxInfo::new(req_key.clone(), chain_name, account);

    let (hash, mut auto_tx) = match chain.chain_client {
        ChainClient::CitaCloud(_) => {
            let mut cita_cloud_auto_tx =
                CitaCloudAutoTx::new(auto_tx_info, tx_data.clone(), timeout);
            let hash = cita_cloud_auto_tx.init_unsend(state.clone()).await?;
            let auto_tx = cita_cloud_auto_tx.to_unified_type();
            (hash, auto_tx)
        }
        ChainClient::Cita(_) => {
            let mut cita_auto_tx = CitaAutoTx::new(auto_tx_info, tx_data.clone(), timeout);
            let hash = cita_auto_tx.init_unsend(state.clone()).await?;
            let auto_tx = cita_auto_tx.to_unified_type();
            (hash, auto_tx)
        }
        ChainClient::Eth(_) => {
            let mut eth_auto_tx = EthAutoTx::new(auto_tx_info, tx_data.clone());
            let hash = eth_auto_tx.init_unsend(state.clone()).await?;
            let auto_tx = eth_auto_tx.to_unified_type();
            (hash, auto_tx)
        }
    };

    // process at once
    tokio::spawn(async move {
        let _ = auto_tx.process(state).await;
    });

    info!(
        "receive send_tx request: req_key: {}, user_code: {}\n\tChainInfo: {}\n\tTxInfo: {}\n\tinitial hash: 0x{}",
        req_key, params.user_code, chain, tx_data, hash.clone()
    );

    ok(json!({
        "hash": add_0x(hash)
    }))
}
