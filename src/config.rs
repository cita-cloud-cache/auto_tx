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

use cloud_util::tracer::LogConfig;
use common_rs::consul::ConsulConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CitaCreateConfig {
    pub chain_name: String,

    pub create_url: String,

    pub accessid: String,
    pub access_secret: String,
    pub appid: String,
    pub appsecret: String,
    pub verify: String,
    pub dapp_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,

    pub kms_url: String,

    pub log_config: LogConfig,
    pub process_interval: u64,
    pub max_timeout: u32,
    pub data_dir: String,

    pub cita_create_config: Option<CitaCreateConfig>,

    pub consul_config: Option<ConsulConfig>,
    pub consul_dir: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 3000,

            kms_url: Default::default(),

            log_config: Default::default(),
            max_timeout: 600,
            process_interval: 5,
            data_dir: "./data".to_string(),

            cita_create_config: None,

            consul_config: Default::default(),
            consul_dir: "chain-cache/".to_string(),
        }
    }
}
