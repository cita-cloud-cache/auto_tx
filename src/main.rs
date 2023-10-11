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

use crate::{get_onchain_hash::get_onchain_hash, kms::set_kms, storage::AutoTxStorage};
use anyhow::Result;
use axum::{http::StatusCode, middleware, routing::any, Json, Router};
use chains::Chains;
use clap::Parser;
use common_rs::restful::handle_http_error;
use config::Config;
use send_tx::handle_send_tx;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
use storage::Storage;

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
}

impl AutoTxGlobalState {
    fn new(config: Config) -> Self {
        Self {
            chains: Chains::new(config.config_center_url),
            storage: Storage::new(config.datadir),
            max_timeout: config.max_timeout,
        }
    }
}

#[tokio::main]
async fn run(opts: RunOpts) -> Result<()> {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let config = Config::new(&opts.config_path);
    set_kms(config.kms_url.clone());

    // init tracer
    cloud_util::tracer::init_tracer("auto_tx".to_string(), &config.log_config)
        .map_err(|e| println!("tracer init err: {e}"))
        .unwrap();

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    let process_interval = config.process_interval;

    // async fn log_req<B>(req: axum::http::Request<B>, next: middleware::Next<B>) -> impl IntoResponse
    // where
    //     B: std::fmt::Debug,
    // {
    //     info!("req: {:?}", req);
    //     next.run(req).await
    // }

    let state = Arc::new(AutoTxGlobalState::new(config));

    let app = Router::new()
        .route("/api/send_tx", any(handle_send_tx))
        .route("/api/get_onchain_hash", any(get_onchain_hash))
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

    let state_clone = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(process_interval)).await;
            match state_clone.storage.get_all_processing().await {
                Ok(auto_txs) => {
                    warn!("get_all_processing: {:?}", auto_txs);
                    for mut auto_tx in auto_txs {
                        let state = state_clone.clone();
                        let tag = auto_tx.get_tag();
                        match tag {
                            send_tx::AutoTxTag::Unsend => {
                                tokio::spawn(async move {
                                    let _ = auto_tx.send(state).await;
                                });
                            }
                            send_tx::AutoTxTag::Uncheck => {
                                tokio::spawn(async move {
                                    let _ = auto_tx.check(state).await;
                                });
                            }
                        }
                    }
                }
                Err(e) => warn!("get_all_processing failed: {}", e),
            }
        }
    });

    info!("auto_tx listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .map_err(|e| anyhow::anyhow!("axum serve failed: {e}"))?;
    anyhow::bail!("unreachable!")
}

#[derive(Debug, Clone, Serialize, Default, Deserialize)]
#[serde(default)]
pub struct RequestParams {
    #[serde(skip_serializing_if = "String::is_empty")]
    user_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    chain_name: String,

    #[serde(skip_serializing_if = "String::is_empty")]
    to: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    data: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    value: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
}
