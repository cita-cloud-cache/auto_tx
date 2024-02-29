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
use common_rs::etcd::ServiceRegisterConfig;
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

    pub fast_mode: bool,
    pub max_timeout: u32,
    pub etcd_endpoints: Vec<String>,

    pub cita_create_config: Option<CitaCreateConfig>,

    pub service_register_config: Option<ServiceRegisterConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 3000,

            kms_url: Default::default(),

            log_config: Default::default(),
            max_timeout: 600,
            process_interval: 5,
            etcd_endpoints: vec!["127.0.0.1:2379".to_string()],

            cita_create_config: None,

            service_register_config: Default::default(),

            fast_mode: false,
        }
    }
}
