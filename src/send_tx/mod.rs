pub mod cita;
pub mod cita_cloud;
pub mod cita_create;
pub mod eth;

use crate::{
    chains::*,
    config::{get_config, CitaCreateConfig},
    kms::{Account, Kms},
    storage::Storage,
    task::*,
    util::{add_0x, remove_quotes_and_0x},
    AutoTxGlobalState, RequestParams,
};
use color_eyre::eyre::Result;
use common_rs::{
    error::CALError,
    restful::{
        axum::{
            extract::{Path, State},
            http::HeaderMap,
            response::IntoResponse,
            Json,
        },
        err, ok, RESTfulError,
    },
};
use serde_json::json;
use std::sync::Arc;

pub const DEFAULT_QUOTA: u64 = 10_000_000;
pub const DEFAULT_QUOTA_LIMIT: u64 = 1_073_741_824;
pub const BASE_QUOTA: u64 = 21000;

pub trait AutoTx {
    async fn process_init_task(
        &mut self,
        init_task: &InitTaskParam,
        storage: &Storage,
    ) -> Result<(String, Timeout, Gas)>;

    async fn process_send_task(
        &mut self,
        init_hash: &str,
        send_task: &SendTask,
        storage: &Storage,
    ) -> Result<String>;

    async fn process_check_task(
        &mut self,
        init_hash: &str,
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<()>;

    async fn get_receipt(&mut self, hash: &str) -> Result<TaskResult>;
}

impl AutoTx for ChainClient {
    async fn process_init_task(
        &mut self,
        init_task: &InitTaskParam,
        storage: &Storage,
    ) -> Result<(String, Timeout, Gas)> {
        match self {
            ChainClient::CitaCloud(client) => client.process_init_task(init_task, storage).await,
            ChainClient::Cita(client) => client.process_init_task(init_task, storage).await,
            ChainClient::Eth(client) => client.process_init_task(init_task, storage).await,
        }
    }

    async fn process_send_task(
        &mut self,
        init_hash: &str,
        send_task: &SendTask,
        storage: &Storage,
    ) -> Result<String> {
        match self {
            ChainClient::CitaCloud(client) => {
                client
                    .process_send_task(init_hash, send_task, storage)
                    .await
            }
            ChainClient::Cita(client) => {
                client
                    .process_send_task(init_hash, send_task, storage)
                    .await
            }
            ChainClient::Eth(client) => {
                client
                    .process_send_task(init_hash, send_task, storage)
                    .await
            }
        }
    }

    async fn process_check_task(
        &mut self,
        init_hash: &str,
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<()> {
        match self {
            ChainClient::CitaCloud(client) => {
                client
                    .process_check_task(init_hash, check_task, storage)
                    .await
            }
            ChainClient::Cita(client) => {
                client
                    .process_check_task(init_hash, check_task, storage)
                    .await
            }
            ChainClient::Eth(client) => {
                client
                    .process_check_task(init_hash, check_task, storage)
                    .await
            }
        }
    }

    async fn get_receipt(&mut self, hash: &str) -> Result<TaskResult> {
        match self {
            ChainClient::CitaCloud(client) => client.get_receipt(hash).await,
            ChainClient::Cita(client) => client.get_receipt(hash).await,
            ChainClient::Eth(client) => client.get_receipt(hash).await,
        }
    }
}

pub async fn handle_send_tx(
    State(state): State<Arc<AutoTxGlobalState>>,
    headers: HeaderMap,
    Path(chain_name): Path<String>,
    Json(body): Json<RequestParams>,
) -> Result<impl IntoResponse, RESTfulError> {
    debug!("handle_send_tx");
    let request_key = if let Some(request_key) = headers.get("request_key") {
        request_key.to_str()?
    } else {
        return err(CALError::BadRequest, "request_key missing");
    };
    let user_code = if let Some(user_code) = headers.get("user_code") {
        user_code.to_str()?
    } else {
        return err(CALError::BadRequest, "user_code missing");
    };

    handle(&state, request_key, user_code, chain_name, body)
        .await
        .map_err(|e| {
            warn!("request: {} failed: {:?}", request_key, e);
            e
        })
}

pub async fn handle(
    state: &Arc<AutoTxGlobalState>,
    request_key: &str,
    user_code: &str,
    chain_name: String,
    params: RequestParams,
) -> Result<impl IntoResponse, RESTfulError> {
    debug!("chain_name: {chain_name}, user_code: {user_code}, request_key: {request_key}, params: {params:?}");

    let config = get_config();
    // check params
    if params.data.is_empty() {
        return err(CALError::BadRequest, "field \"data\" missing");
    }

    let request_key = format!("{user_code}-{request_key}");

    // check if request_key exists
    if let Ok(init_hash) = state
        .storage
        .load_init_hash_by_request_key(&request_key)
        .await
    {
        return ok(json!({
            "hash": init_hash
        }));
    }

    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;

    // check if cita_create
    if chain.chain_name
        == config
            .cita_create_config
            .as_ref()
            .unwrap_or(&CitaCreateConfig::default())
            .chain_name
        && params.to.is_empty()
    {
        info!("receive cita create request: request_key: {}", request_key);
        let resp = cita_create::send_cita_create(
            config.cita_create_config.as_ref().unwrap(),
            &params.data,
            &request_key,
        )
        .await?;
        match resp.data {
            Some(mut data) => {
                if data.errMsg.is_empty() {
                    data.contractAddress = remove_quotes_and_0x(&data.contractAddress);
                    data.deployTxHash = remove_quotes_and_0x(&data.deployTxHash);
                    debug!("cita create request data: {:?}", &data);
                    let result =
                        TaskResult::success(data.deployTxHash.clone(), Some(data.contractAddress));
                    state.storage.finalize_task(&request_key, &result).await?;
                    info!("cita create request success: request_key: {}", request_key);
                    return ok(json!({
                        "hash": add_0x(data.deployTxHash),
                    }));
                }
                return err(CALError::CitaCMCCreateFailed, &data.errMsg);
            }
            None => {
                return err(
                    CALError::CitaCMCCreateFailed,
                    &format!("{}, {}", resp.code, resp.msg),
                )
            }
        }
    }

    // get timeout
    let timeout = {
        if let Some(timeout) = params.timeout {
            (timeout.min(config.max_timeout).max(20) as f64 * 0.8) as u32
        } else {
            config.max_timeout
        }
    };

    // get Account
    let account = Account::new(user_code.to_string(), chain.chain_info.crypto_type.clone()).await?;

    // get TxData
    let tx_data = TxData::new(&params.to, &params.data, &params.value)?;

    // get InitTask
    let init_task = InitTaskParam {
        base_data: BaseData {
            request_key: request_key.clone(),
            chain_name: chain_name.clone(),
            account: account.clone(),
            tx_data: tx_data.clone(),
        },
        timeout,
        gas: params.gas.unwrap_or_default(),
    };

    let (init_hash, timeout, gas) = chain
        .chain_client
        .process_init_task(&init_task, &state.storage)
        .await?;

    info!(
        "receive send_tx request: request_key: {}, user_code: {}\n\tchain: {}\n\tfrom: {}, tx_data: {}\n\ttimeout: {}, gas: {}\n\tinitial hash: 0x{}",
        request_key, user_code, chain, account.address_str(), tx_data, timeout, gas.gas, init_hash
    );

    // cache init_hash by request_key
    state
        .storage
        .store_init_hash_by_request_key(&request_key, &init_hash)
        .await
        .ok();

    ok(json!({
        "hash": init_hash
    }))
}
