pub mod cita;
pub mod cita_cloud;
pub mod cita_create;
pub mod eth;
pub mod types;

use crate::{
    chains::*, kms::Account, storage::Storage, util::add_0x, AutoTxGlobalState, RequestParams,
};
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    Json,
};
use common_rs::restful::{ok, RESTfulError};
use serde_json::json;
use std::sync::Arc;
use types::*;

#[axum::async_trait]
pub trait AutoTx {
    async fn process_init_task(
        &mut self,
        init_task: &InitTask,
        storage: &Storage,
    ) -> Result<(Timeout, Gas)>;

    async fn process_send_task(
        &mut self,
        send_task: &SendTask,
        storage: &Storage,
    ) -> Result<String>;

    async fn process_check_task(&mut self, check_task: &CheckTask, storage: &Storage)
        -> Result<()>;
}

#[axum::async_trait]
impl AutoTx for ChainClient {
    async fn process_init_task(
        &mut self,
        init_task: &InitTask,
        storage: &Storage,
    ) -> Result<(Timeout, Gas)> {
        match self {
            ChainClient::CitaCloud(client) => client.process_init_task(init_task, storage).await,
            ChainClient::Cita(client) => client.process_init_task(init_task, storage).await,
            ChainClient::Eth(client) => client.process_init_task(init_task, storage).await,
        }
    }

    async fn process_send_task(
        &mut self,
        send_task: &SendTask,
        storage: &Storage,
    ) -> Result<String> {
        match self {
            ChainClient::CitaCloud(client) => client.process_send_task(send_task, storage).await,
            ChainClient::Cita(client) => client.process_send_task(send_task, storage).await,
            ChainClient::Eth(client) => client.process_send_task(send_task, storage).await,
        }
    }

    async fn process_check_task(
        &mut self,
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<()> {
        match self {
            ChainClient::CitaCloud(client) => client.process_check_task(check_task, storage).await,
            ChainClient::Cita(client) => client.process_check_task(check_task, storage).await,
            ChainClient::Eth(client) => client.process_check_task(check_task, storage).await,
        }
    }
}

pub async fn handle_send_tx(
    headers: HeaderMap,
    Path(chain_name): Path<String>,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    let request_key = headers
        .get("request_key")
        .ok_or_else(|| {
            let e = anyhow::anyhow!("no request_key in header");
            warn!("request failed: {}", e);
            e
        })?
        .to_str()?;
    let user_code = headers
        .get("user_code")
        .ok_or(anyhow::anyhow!("user_code missing"))?
        .to_str()?;

    handle(
        request_key,
        user_code,
        Path(chain_name),
        State(state),
        Json(params),
    )
    .await
    .map_err(|e| {
        warn!("request: {} failed: {:?}", request_key, e);
        e
    })
}

pub async fn handle(
    request_key: &str,
    user_code: &str,
    Path(chain_name): Path<String>,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    debug!("params: {:?}", params);

    // check params
    if params.data.is_empty() {
        return Err(anyhow::anyhow!("field \"data\" missing").into());
    }

    let request_key = params.user_code.clone() + "-" + req_key;

    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;

    // get timeout
    let timeout = {
        if let Some(timeout) = params.timeout {
            (timeout.min(state.max_timeout).max(20) as f64 * 0.8) as u32
        } else {
            state.max_timeout
        }
    };

    // get Account
    let account = Account::new(
        params.user_code.clone(),
        chain.chain_info.crypto_type.clone(),
    )
    .await?;

    // get TxData
    let tx_data = TxData::new(&params.to, &params.data, &params.value)?;

    // get InitTask
    let init_task = InitTask {
        base_data: BaseData {
            request_key: request_key.clone(),
            chain_name,
        },
        send_data: SendData {
            account,
            tx_data: tx_data.clone(),
        },
        timeout,
    };
    let (timeout, gas) = chain
        .chain_client
        .process_init_task(&init_task, &state.storage)
        .await?;

    if state.fast_mode {
        info!(
            "receive send_tx request: req_key: {}, user_code: {}\n\tchain: {}\n\ttx_data: {}\n\ttimeout: {}, gas: {}",
            req_key, params.user_code, chain, tx_data, timeout, gas.gas
        );
        return ok(json!({}));
    }

    // optim: not read from storage
    let send_task = SendTask {
        base_data: init_task.base_data,
        send_data: init_task.send_data,
        timeout,
        gas,
    };

    let hash = {
        let mut write = state.processing.write().await;
        write.insert(request_key.clone());
        let hash = chain
            .chain_client
            .process_send_task(&send_task, &state.storage)
            .await?;
        write.remove(&request_key);
        hash
    };

    info!(
        "receive send_tx request: req_key: {}, user_code: {}\n\tchain: {}\n\ttx_data: {}\n\ttimeout: {}, gas: {}\n\tinitial hash: 0x{}",
        req_key, params.user_code, chain, tx_data, timeout, gas.gas, hash.clone()
    );

    ok(json!({
        "hash": add_0x(hash)
    }))
}
