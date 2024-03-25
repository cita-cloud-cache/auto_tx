pub mod cita;
pub mod cita_cloud;
pub mod cita_create;
pub mod eth;
pub mod types;

use crate::{
    chains::*,
    config::CitaCreateConfig,
    kms::{Account, Kms},
    storage::Storage,
    util::{add_0x, remove_quotes_and_0x},
    AutoTxGlobalState, RequestParams,
};
use color_eyre::eyre::{eyre, Result};
use common_rs::{
    error::CALError,
    restful::{err, ok, RESTfulError},
};
use salvo::prelude::*;
use serde_json::json;
use std::sync::Arc;
use types::*;

pub const DEFAULT_QUOTA: u64 = 10_000_000;
pub const DEFAULT_QUOTA_LIMIT: u64 = 1_073_741_824;
pub const BASE_QUOTA: u64 = 21000;

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

    async fn process_check_task(
        &mut self,
        check_task: &CheckTask,
        storage: &Storage,
    ) -> Result<AutoTxResult>;

    async fn get_receipt(&mut self, hash: &str) -> Result<AutoTxResult>;
}

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
    ) -> Result<AutoTxResult> {
        match self {
            ChainClient::CitaCloud(client) => client.process_check_task(check_task, storage).await,
            ChainClient::Cita(client) => client.process_check_task(check_task, storage).await,
            ChainClient::Eth(client) => client.process_check_task(check_task, storage).await,
        }
    }

    async fn get_receipt(&mut self, hash: &str) -> Result<AutoTxResult> {
        match self {
            ChainClient::CitaCloud(client) => client.get_receipt(hash).await,
            ChainClient::Cita(client) => client.get_receipt(hash).await,
            ChainClient::Eth(client) => client.get_receipt(hash).await,
        }
    }
}

#[handler]
pub async fn handle_send_tx(depot: &Depot, req: &mut Request) -> Result<impl Writer, RESTfulError> {
    let headers = req.headers().clone();
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
    let chain_name = if let Some(chain_name) = req.param::<String>("chain_name") {
        chain_name
    } else {
        return err(CALError::BadRequest, "chain_name missing");
    };

    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    let params = req.parse_body().await?;
    handle(state, request_key, user_code, chain_name, params)
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
) -> Result<impl Writer, RESTfulError> {
    debug!("chain_name: {chain_name}, user_code: {user_code}, request_key: {request_key}, params: {params:?}");

    // check params
    if params.data.is_empty() {
        return err(CALError::BadRequest, "field \"data\" missing");
    }

    let request_key = user_code.to_string() + "-" + request_key;

    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;

    // check if cita_create
    if chain.chain_name
        == state
            .cita_create_config
            .as_ref()
            .unwrap_or(&CitaCreateConfig::default())
            .chain_name
        && params.to.is_empty()
    {
        info!("receive cita create request: request_key: {}", request_key);
        let resp = cita_create::send_cita_create(
            state.cita_create_config.as_ref().unwrap(),
            &params.data,
            &request_key,
        )
        .await?;
        match resp.data {
            Some(mut data) => {
                if data.errMsg.is_empty() {
                    data.contractAddress = remove_quotes_and_0x(&data.contractAddress);
                    data.deployTxHash = remove_quotes_and_0x(&data.deployTxHash);
                    let result = AutoTxResult::success(
                        data.deployTxHash.clone(),
                        Some(data.contractAddress),
                    );
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
            (timeout.min(state.max_timeout).max(20) as f64 * 0.8) as u32
        } else {
            state.max_timeout
        }
    };

    // get Account
    let account = Account::new(user_code.to_string(), chain.chain_info.crypto_type.clone()).await?;

    // get TxData
    let tx_data = TxData::new(&params.to, &params.data, &params.value)?;

    // get InitTask
    let init_task = InitTask {
        base_data: BaseData {
            request_key: request_key.clone(),
            chain_name,
        },
        send_data: SendData {
            account: account.clone(),
            tx_data: tx_data.clone(),
        },
        timeout,
    };
    let lock_key = state
        .storage
        .try_lock_task(&request_key)
        .await
        .map_err(|e| eyre!("Duplicate Tx: {e}"))?;
    let (timeout, gas) = chain
        .chain_client
        .process_init_task(&init_task, &state.storage)
        .await?;

    if state.fast_mode {
        info!(
            "receive send_tx request: request_key: {}, user_code: {}\n\tchain: {}\n\tfrom: {}, tx_data: {}\n\ttimeout: {}, gas: {}",
            request_key, user_code, chain, account.address_str(), tx_data, timeout, gas.gas
        );
        state.storage.unlock_task(&lock_key).await.ok();
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
        let hash = chain
            .chain_client
            .process_send_task(&send_task, &state.storage)
            .await?;
        state.storage.unlock_task(&lock_key).await.ok();
        hash
    };

    info!(
        "receive send_tx request: request_key: {}, user_code: {}\n\tchain: {}\n\tfrom: {}, tx_data: {}\n\ttimeout: {}, gas: {}\n\tinitial hash: 0x{}",
        request_key, user_code, chain, account.address_str(), tx_data, timeout, gas.gas, hash.clone()
    );

    ok(json!({
        "hash": add_0x(hash)
    }))
}
