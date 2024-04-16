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
mod get_receipt;
mod get_task;
mod kms;
mod send_tx;
mod storage;
mod task;
mod util;

use crate::{
    config::set_config, get_receipt::get_receipt as get_receipt_handler,
    get_task::get_task as get_task_handler, kms::set_kms, send_tx::AutoTx, task::Status,
};
use chains::Chains;
use clap::Parser;
use color_eyre::eyre::Result;
use common_rs::{
    configure::file_config,
    log,
    redis::{self, AsyncCommands, Redis},
    restful::http_serve,
};
use config::{CitaCreateConfig, Config};
use salvo::prelude::*;
use send_tx::handle_send_tx;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
                println!("err: {:?}", e);
            }
        }
    }
}

#[derive(Clone)]
pub struct AutoTxGlobalState {
    chains: Chains,
    storage: Storage,
    max_timeout: u32,
    cita_create_config: Option<CitaCreateConfig>,
}

impl AutoTxGlobalState {
    async fn new(config: Config) -> Self {
        let redis = Redis::new(&config.redis_config)
            .await
            .map_err(|e| error!("redis connect failed: {e}"))
            .unwrap();

        Self {
            chains: Chains::new(redis.clone()).await,
            storage: Storage::new(redis).await,
            max_timeout: config.max_timeout,
            cita_create_config: config.cita_create_config,
        }
    }
}

#[tokio::main]
async fn run(opts: RunOpts) -> Result<()> {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let config: Config = file_config(&opts.config_path)?;
    set_config(config.clone());
    set_kms(config.kms_url.clone());

    // init tracer
    log::init_tracing(&config.name, &config.log_config)?;

    info!("fast_mode: {}", config.fast_mode);
    info!("process_interval: {}", config.process_interval);
    info!("max_timeout: {}", config.max_timeout);
    info!("use kms: {}", config.kms_url);

    if let Some(config) = config.cita_create_config.as_ref() {
        info!("CitaCreateConfig exist: chain_name: {}", config.chain_name);
    } else {
        info!("run without CitaCreateConfig")
    }

    let service_name = config.name.clone();

    let Config {
        name,
        port,
        process_interval,
        check_workers_num,
        send_workers_num,
        ..
    } = config.clone();

    if let Some(service_register_config) = &config.service_register_config {
        let redis = redis::Redis::new(&config.redis_config).await?;
        redis
            .service_register(&service_name, service_register_config.clone())
            .await
            .ok();
    }

    let state = Arc::new(AutoTxGlobalState::new(config).await);

    let router = Router::new()
        .hoop(affix::inject(state.clone()))
        .push(Router::with_path("/api/<chain_name>/send_tx").post(handle_send_tx))
        .push(Router::with_path("/api/<chain_name>/receipt/<hash>").get(get_receipt_handler))
        .push(Router::with_path("/api/task/<hash>").get(get_task_handler));

    let (send_task_tx, send_task_rx) = flume::unbounded::<String>();
    let (check_task_tx, check_task_rx) = flume::unbounded::<String>();

    // spawn check task workers
    for _ in 0..check_workers_num {
        let state = state.clone();
        let check_task_rx = check_task_rx.clone();
        tokio::spawn(async move {
            while let Ok(init_hash) = check_task_rx.recv_async().await {
                debug!("checking task: {}", &init_hash);
                if let Ok(check_task) = state.storage.load_check_task(&init_hash).await {
                    let chain_name = check_task.base_data.chain_name.as_ref();
                    if let Ok(mut chain) = state.chains.get_chain(chain_name).await {
                        if let Err(e) = chain
                            .chain_client
                            .process_check_task(&init_hash, &check_task, &state.storage)
                            .await
                        {
                            if e.to_string().contains("rpc timeout") {
                                check_task_rx.drain();
                            }
                        }
                    }
                }
            }
        });
    }

    // spawn send task workers
    for _ in 0..send_workers_num {
        let state = state.clone();
        let send_task_rx = send_task_rx.clone();
        tokio::spawn(async move {
            while let Ok(init_hash) = send_task_rx.recv_async().await {
                if let Ok(send_task) = state.storage.load_send_task(&init_hash).await {
                    if state.storage.try_lock_task(&init_hash).await.is_ok() {
                        let chain_name = send_task.base_data.chain_name.as_ref();
                        if let Ok(mut chain) = state.chains.get_chain(chain_name).await {
                            chain
                                .chain_client
                                .process_send_task(&init_hash, &send_task, &state.storage)
                                .await
                                .ok();
                        }
                        state.storage.unlock_task(&init_hash).await.ok();
                    }
                }
            }
        });
    }

    // read and distribute tasks proactively
    tokio::spawn(async move {
        let mut process_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(process_interval));
        let mut conn = state.storage.operator();
        loop {
            process_interval.tick().await;
            if let Ok(mut iter) = conn
                .scan_match::<String, String>(format!("{}/task/status/*", name))
                .await
            {
                while let Some(key_str) = iter.next_item().await {
                    if let Some(init_hash) = key_str.split('/').last() {
                        if let Ok(status) = state.storage.load_status(init_hash).await {
                            match status {
                                Status::Unsend => {
                                    send_task_tx.send_async(init_hash.to_string()).await.ok();
                                }
                                Status::Uncheck => {
                                    check_task_tx.send_async(init_hash.to_string()).await.ok();
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    });

    http_serve(&service_name, port, router).await;

    Ok(())
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

    #[serde(skip_serializing_if = "Option::is_none")]
    gas: Option<u64>,
}
