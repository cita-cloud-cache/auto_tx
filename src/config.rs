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

use common_rs::{log::LogConfig, redis::RedisConfig, service_register::ServiceRegisterConfig};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

pub static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) {
    CONFIG.get_or_init(|| config);
}

pub fn get_config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub name: String,

    pub port: u16,

    pub kms_url: String,

    pub log_config: LogConfig,

    pub read_send_num: usize,

    pub read_check_num: usize,

    pub task_retry_interval: u64,

    pub recycle_task_interval: u64,

    pub check_workers_num: u64,

    pub max_timeout: u32,

    pub chain_config_ttl: u64,

    pub request_key_ttl: u64,

    pub rpc_timeout: u64,

    pub redis_config: RedisConfig,

    pub cita_create_config: Option<CitaCreateConfig>,

    pub service_register_config: Option<ServiceRegisterConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "auto_tx".to_string(),
            port: 3000,
            kms_url: Default::default(),
            log_config: Default::default(),
            max_timeout: 600,
            read_send_num: 100,
            read_check_num: 10,
            task_retry_interval: 1,
            recycle_task_interval: 120,
            check_workers_num: 5,
            chain_config_ttl: 3,
            rpc_timeout: 1000,
            cita_create_config: None,
            redis_config: Default::default(),
            service_register_config: Default::default(),
            request_key_ttl: 600,
        }
    }
}
