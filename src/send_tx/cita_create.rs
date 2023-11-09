use crate::config::CitaCreateConfig;
use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const FAKE_ABI: &str = r#"[{"constant":true,"inputs":[],"name":"count","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"}]"#;
const FAKE_LIST: [&str; 1] = ["06661abd: count()"];

fn sign(
    appid: &str,
    appsecret: &str,
    accessid: &str,
    access_secret: &str,
    timestamp: u64,
) -> (String, String) {
    let mut param_vec = Vec::new();
    param_vec.push(format!("accessId={}", accessid));
    param_vec.push(format!("accessSecret={}", access_secret));
    param_vec.push(format!("timestamp={}", timestamp));

    let joined_params = param_vec.join("&");
    let sign = format!("{:x}", md5::compute(joined_params.as_bytes()));

    let mut api_vec = Vec::new();
    api_vec.push(format!("appid={}", appid));
    api_vec.push(format!("secret={}", appsecret));
    api_vec.push(format!("sign={}", sign));
    api_vec.push(format!("timestamp={}", timestamp));

    api_vec.sort();
    let api_joined = api_vec.join("&");
    let api_sign = format!("{:x}", md5::compute(api_joined.as_bytes()));

    (sign, api_sign)
}

#[derive(Deserialize, Debug)]
pub struct CitaCreateResponse {
    pub code: i32,
    pub data: Option<CitaCreateResponseData>,
    pub msg: String,
}

#[allow(non_snake_case)]
#[derive(Deserialize, Debug)]
pub struct CitaCreateResponseData {
    pub deployTxHash: String,
    pub contractId: String,
    pub errMsg: String,
    pub contractAddress: String,
}

pub async fn send_cita_create(
    config: &CitaCreateConfig,
    data: &str,
    request_key: &str,
) -> Result<CitaCreateResponse> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (sign, api_sign) = sign(
        &config.appid,
        &config.appsecret,
        &config.accessid,
        &config.access_secret,
        timestamp,
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("accessid"),
        HeaderValue::from_str(&config.accessid).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("sign"),
        HeaderValue::from_str(&sign).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("timestamp"),
        HeaderValue::from_str(&timestamp.to_string()).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("appid"),
        HeaderValue::from_str(&config.appid).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("verify"),
        HeaderValue::from_str(&config.verify).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("apisign"),
        HeaderValue::from_str(&api_sign).unwrap(),
    );
    let contract_name = if request_key.len() > 40 {
        &request_key[..40]
    } else {
        request_key
    };

    let body = serde_json::json!({
        "dappName": config.dapp_name,
        "contractName": contract_name,
        "bin": data,
        "abi": FAKE_ABI,
        "functionHashList": FAKE_LIST,
    });

    let resp = reqwest::Client::new()
        .post(&config.create_url)
        .headers(headers)
        .json(&body)
        .send()
        .await?
        .json::<CitaCreateResponse>()
        .await?;

    Ok(resp)
}
