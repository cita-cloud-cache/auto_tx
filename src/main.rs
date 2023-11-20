// Copyright Rivtower Technologies LLC.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    unused_crate_dependencies,
    clippy::missing_const_for_fn,
    unused_extern_crates
)]

#[macro_use]
extern crate tracing;

mod chains;
mod config;
mod get_onchain_hash;
mod kms;
mod send_tx;
mod storage;
mod util;

use crate::{
    get_onchain_hash::get_onchain_hash,
    kms::set_kms,
    send_tx::{types::Status, AutoTx},
};
use anyhow::Result;
use axum::{
    http::StatusCode,
    middleware,
    routing::{any, get, post},
    Json, Router,
};
use chains::Chains;
use clap::Parser;
use common_rs::{
    consul,
    restful::{handle_http_error, ok_no_data, shutdown_signal},
};
use config::{CitaCreateConfig, Config};
use figment::{
    providers::{Format, Toml},
    Figment,
};
use send_tx::handle_send_tx;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use storage::Storage;
use tokio::sync::RwLock;

/// A subcommand for run
#[derive(Parser)]
struct RunOpts {
    /// Chain config path
    #[clap(short = 'c', long = "config", default_value = "config.toml")]
    config_path: String,
}

#[derive(Parser)]
enum SubCommand {
    /// run this service
    #[clap(name = "run")]
    Run(RunOpts),
}

pub fn clap_about() -> String {
    let name = env!("CARGO_PKG_NAME").to_string();
    let version = env!("CARGO_PKG_VERSION");
    let authors = env!("CARGO_PKG_AUTHORS");
    name + " " + version + "\n" + authors
}

#[derive(Parser)]
#[clap(version, about = clap_about())]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

fn main() {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let opts: Opts = Opts::parse();

    match opts.subcmd {
        SubCommand::Run(opts) => {
            if let Err(e) = run(opts) {
                warn!("err: {:?}", e);
            }
        }
    }
}

#[derive(Clone)]
pub struct AutoTxGlobalState {
    pub chains: Chains,
    pub storage: Storage,
    pub max_timeout: u32,
    pub cita_create_config: Option<CitaCreateConfig>,
    pub processing: Arc<RwLock<HashSet<String>>>,
    pub fast_mode: bool,
}

impl AutoTxGlobalState {
    fn new(config: Config) -> Self {
        Self {
            chains: Chains::new(
                config.consul_config.unwrap_or_default().consul_addr,
                config.consul_dir,
            ),
            storage: Storage::new(config.data_dir),
            max_timeout: config.max_timeout,
            cita_create_config: config.cita_create_config,
            processing: Arc::new(RwLock::new(HashSet::new())),
            fast_mode: config.fast_mode,
        }
    }
}

#[tokio::main]
async fn run(opts: RunOpts) -> Result<()> {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let config: Config = Figment::new()
        .join(Toml::file(&opts.config_path))
        .extract()?;
    set_kms(config.kms_url.clone());

    // init tracer
    cloud_util::tracer::init_tracer("auto_tx".to_string(), &config.log_config)
        .map_err(|e| println!("tracer init err: {e}"))
        .unwrap();

    if let Some(config) = config.cita_create_config.as_ref() {
        info!("CitaCreateConfig exist: chain_name: {}", config.chain_name);
    } else {
        info!("run without CitaCreateConfig")
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    let process_interval = config.process_interval;

    if let Some(consul_config) = &config.consul_config {
        consul::service_register(consul_config).await?;
    }

    let state = Arc::new(AutoTxGlobalState::new(config));

    let app = Router::new()
        .route("/api/:chain_name/send_tx", post(handle_send_tx))
        .route("/api/get_onchain_hash", get(get_onchain_hash))
        .route("/health", any(|| async { ok_no_data() }))
        // .route_layer(middleware::from_fn(log_req))
        .route_layer(middleware::from_fn(handle_http_error))
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "code": 404,
                    "message": "Not Found",
                })),
            )
        })
        .with_state(state.clone());

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(process_interval)).await;
            match state.storage.get_processing_tasks().await {
                Ok(processing) => {
                    for (request_key, status) in processing {
                        let state = state.clone();
                        let is_processing = {
                            let read = state.processing.read().await;
                            read.contains(&request_key)
                        };
                        if !is_processing {
                            tokio::spawn(async move {
                                let mut write = state.processing.write().await;
                                write.insert(request_key.clone());
                                match status {
                                    Status::Unsend => {
                                        let send_task = state
                                            .storage
                                            .load_send_task(&request_key)
                                            .await
                                            .unwrap();
                                        let chain_name = send_task.base_data.chain_name.as_ref();
                                        let mut client = state
                                            .chains
                                            .get_chain(chain_name)
                                            .await
                                            .unwrap()
                                            .chain_client;
                                        let _ = client
                                            .process_send_task(&send_task, &state.storage)
                                            .await;
                                    }
                                    Status::Uncheck => {
                                        let check_task = state
                                            .storage
                                            .load_check_task(&request_key)
                                            .await
                                            .unwrap();
                                        let chain_name = check_task.base_data.chain_name.as_ref();
                                        let mut client = state
                                            .chains
                                            .get_chain(chain_name)
                                            .await
                                            .unwrap()
                                            .chain_client;
                                        let _ = client
                                            .process_check_task(&check_task, &state.storage)
                                            .await;
                                    }
                                }
                                write.remove(&request_key);
                            });
                        }
                    }
                }
                Err(e) => warn!("get_processing_tasks failed: {}", e),
            }
        }
    });

    info!("auto_tx listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("axum serve failed: {e}"))
}

#[derive(Debug, Clone, Serialize, Default, Deserialize)]
#[serde(default)]
pub struct RequestParams {
    #[serde(skip_serializing_if = "String::is_empty")]
    to: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    data: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    value: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
}
