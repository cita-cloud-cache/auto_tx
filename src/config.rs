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

use cloud_util::{common::read_toml, tracer::LogConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    pub kms_url: String,
    pub config_center_url: String,
    pub log_config: LogConfig,

    pub max_timeout: u32,
    pub process_interval: u64,
    pub datadir: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 3000,
            kms_url: Default::default(),
            config_center_url: Default::default(),
            log_config: Default::default(),
            max_timeout: 600,
            process_interval: 2,
            datadir: "./data".to_string(),
        }
    }
}

impl Config {
    pub fn new(config_str: &str) -> Self {
        read_toml(config_str, "auto_tx")
    }
}