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

use crate::{
    config::get_config,
    send_tx::{cita::CitaClient, cita_cloud::CitaCloudClient, eth::EthClient},
};
use color_eyre::eyre::{eyre, Result};
use common_rs::redis::{AsyncCommands, Redis};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display, sync::Arc};
use tokio::sync::RwLock;

#[derive(Clone)]
pub enum ChainClient {
    CitaCloud(CitaCloudClient),
    Cita(CitaClient),
    Eth(EthClient),
}

impl Display for ChainClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            ChainClient::CitaCloud(_) => write!(f, "CitaCloud"),
            ChainClient::Cita(_) => write!(f, "Cita"),
            ChainClient::Eth(_) => write!(f, "Eth"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChainInfo {
    pub chain_type: String,
    pub crypto_type: String,
    pub chain_url: String,
}

impl Display for ChainInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "chain_type: {}, crypto_type: {}, chain_url: {}",
            self.chain_type, self.crypto_type, self.chain_url
        )
    }
}

#[derive(Clone)]
pub struct Chain {
    pub chain_name: String,
    pub chain_info: ChainInfo,
    pub chain_client: ChainClient,
}

impl Display for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "chain_name: {}, chain_info: {}",
            self.chain_name, self.chain_info
        )
    }
}

impl Chain {
    async fn new(chain_name: &str, chain_info: ChainInfo) -> Result<Self> {
        let chain_type = chain_info.chain_type.to_lowercase();
        let chain_client = match chain_type.as_str() {
            "cita-cloud" => {
                let mut client = CitaCloudClient::new(&chain_info.chain_url, chain_name)?;
                client
                    .get_gas_limit(None)
                    .await
                    .map_err(|_| eyre!("cita-cloud url check failed"))?;
                ChainClient::CitaCloud(client)
            }
            "cita" => {
                let client = CitaClient::new(&chain_info.chain_url, chain_name)?;
                client
                    .get_gas_limit(None)
                    .await
                    .map_err(|_| eyre!("cita url check failed"))?;
                ChainClient::Cita(client)
            }
            "eth" => {
                let client = EthClient::new(&chain_info.chain_url, chain_name)?;
                client
                    .get_gas_limit(None)
                    .await
                    .map_err(|_| eyre!("eth url check failed"))?;
                ChainClient::Eth(client)
            }
            s => unimplemented!("not support chain_type: {s}"),
        };
        let chain = Chain {
            chain_name: chain_name.to_string(),
            chain_info,
            chain_client,
        };

        info!("chains update: {chain}");

        Ok(chain)
    }
}

#[derive(Clone)]
pub struct Chains {
    serve_chains: Arc<RwLock<HashMap<String, Chain>>>,
    config_center: Redis,
}

impl Chains {
    pub async fn new(redis: Redis) -> Self {
        let chains = HashMap::new();
        Self {
            serve_chains: Arc::new(RwLock::new(chains)),
            config_center: redis,
        }
    }

    async fn request_chain_info(&self, chain_name: &str) -> Result<Chain> {
        let key = format!("{}/ChainInfo/{}", get_config().name, chain_name);
        let value: String = self.config_center.conn().get(key).await?;
        let chain_info: ChainInfo = serde_json::from_str(&value)?;
        Chain::new(chain_name, chain_info).await
    }

    pub async fn get_chain(&self, chain_name: &str) -> Result<Chain> {
        let read_guard = self.serve_chains.read().await;

        if let Some(info) = read_guard.get(chain_name) {
            return Ok(info.clone());
        }

        drop(read_guard);

        let mut write_guard = self.serve_chains.write().await;

        let chain_info = write_guard
            .entry(chain_name.to_string())
            .or_insert(self.request_chain_info(chain_name).await?)
            .to_owned();

        Ok(chain_info)
    }
}
