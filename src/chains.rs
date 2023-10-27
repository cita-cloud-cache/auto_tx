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

use crate::send_tx::{cita::CitaClient, cita_cloud::CitaCloudClient, eth::EthClient};
use anyhow::Result;
use std::{collections::HashMap, fmt::Display, sync::Arc};
use tokio::sync::RwLock;

#[derive(Clone)]
pub enum ChainType {
    CitaCloud(CitaCloudClient),
    Cita(CitaClient),
    Eth(EthClient),
}

impl Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            ChainType::CitaCloud(_) => write!(f, "CitaCloud"),
            ChainType::Cita(_) => write!(f, "Cita"),
            ChainType::Eth(_) => write!(f, "Eth"),
        }
    }
}

#[derive(Clone)]
pub struct ChainInfo {
    pub chain_name: String,
    pub chain_type: ChainType,
    pub crypto_type: String,
    pub chain_url: String,
}

impl Display for ChainInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "chain_name: {}, chain_type: {}, crypto_type: {}, chain_url: {}",
            self.chain_name, self.chain_type, self.crypto_type, self.chain_url
        )
    }
}

impl ChainInfo {
    pub fn new(
        chain_name: &str,
        chain_type: ChainType,
        crypto_type: &str,
        chain_url: &str,
    ) -> Self {
        Self {
            chain_name: chain_name.to_string(),
            chain_type,
            crypto_type: crypto_type.into(),
            chain_url: chain_url.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
struct ConfigCenter {
    _url: String,
}

impl ConfigCenter {
    const fn new(url: String) -> Self {
        Self { _url: url }
    }

    fn request_chain_info(&self, _chain_name: &str) -> Result<ChainInfo> {
        todo!()
    }
}

#[derive(Clone)]
pub struct Chains {
    serve_chains: Arc<RwLock<HashMap<String, ChainInfo>>>,
    config_center: ConfigCenter,
}

impl Chains {
    pub fn new(url: String) -> Self {
        let mut chains = HashMap::new();
        // for test
        chains.insert(
            "cita-cloud".to_string(),
            ChainInfo::new(
                "cita-cloud",
                ChainType::CitaCloud(CitaCloudClient::new("http://127.0.0.1").unwrap()),
                "SM2",
                "http://127.0.0.1",
            ),
        );
        chains.insert(
            "cita".to_string(),
            ChainInfo::new(
                "cita",
                ChainType::Cita(CitaClient::new("http://192.168.160.27:1337").unwrap()),
                "SM2",
                "http://192.168.160.27:1337",
            ),
        );
        chains.insert(
            "eth".to_string(),
            ChainInfo::new(
                "eth",
                ChainType::Eth(EthClient::new("http://192.168.120.0:8545").unwrap()),
                "Secp256k1",
                "http://192.168.120.0:8545",
            ),
        );
        Self {
            serve_chains: Arc::new(RwLock::new(chains)),
            config_center: ConfigCenter::new(url),
        }
    }

    pub async fn get_chain_info(&self, chain_name: &str) -> Result<ChainInfo> {
        let read_guard = self.serve_chains.read().await;

        if let Some(info) = read_guard.get(chain_name) {
            return Ok(info.clone());
        }

        drop(read_guard);

        let mut write_guard = self.serve_chains.write().await;

        let chain_info = write_guard
            .entry(chain_name.to_string())
            .or_insert(self.config_center.request_chain_info(chain_name)?)
            .to_owned();

        Ok(chain_info)
    }
}
