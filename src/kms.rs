use std::collections::HashMap;

use bevy_reflect::Reflect;
use cita_tool::{Encryption, Hashable};
use color_eyre::eyre::{eyre, Result};
use ethabi::ethereum_types::H256;
use hex::ToHex;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use web3::{
    signing::{Key, Signature, SigningError},
    types::Address,
};

use crate::util::{add_0x, parse_data};

static KMS: OnceCell<String> = OnceCell::new();

pub fn set_kms(s: String) {
    KMS.get_or_init(|| s);
}

#[derive(Deserialize, Debug)]
struct AddrResponse {
    code: i32,
    data: Option<AddrResponseData>,
    message: String,
}

#[derive(Deserialize, Debug)]
struct AddrResponseData {
    address: String,
}

async fn get_user_address(user_code: &str, crypto_type: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let kms_url = format!("{}/key", KMS.get().unwrap());

    let mut data = HashMap::new();
    data.insert("user_code", user_code);
    data.insert("crypto_type", crypto_type);

    debug!("get_user_address data: {:?}", data);

    let resp = client
        .post(kms_url)
        .json(&data)
        .send()
        .await
        .map_err(|e| eyre!("get_user_address post err: {e}"))?
        .json::<AddrResponse>()
        .await
        .map_err(|e| eyre!("get_user_address err: {e}"))?;

    debug!("get_user_address resp: {:?}", resp);

    if resp.code != 200 {
        return Err(eyre!(
            "kms get_user_address failed: {}, {}",
            resp.code,
            resp.message
        ));
    }

    if let Some(data) = resp.data {
        parse_data(&data.address)
    } else {
        Err(eyre!(
            "kms get_user_address failed: no data, {}, {}",
            resp.code,
            resp.message
        ))
    }
}

#[test]
fn test_get_user_address() -> Result<()> {
    let user_code = "123";
    let crypto_type = "SM2";
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let kms_url = "http://192.168.194.181/api/keys/key";

    let mut data = HashMap::new();
    data.insert("user_code", user_code);
    data.insert("crypto_type", crypto_type);

    debug!("get_user_address data: {:?}", data);

    let _ = client
        .post(kms_url)
        .json(&data)
        .send()
        .map_err(|e| eyre!("get_user_address post err: {e}"))?
        .json::<AddrResponse>()
        .map_err(|e| eyre!("get_user_address err: {e}"))?;
    Ok(())
}

#[derive(Deserialize, Debug)]
struct SignResponse {
    code: i32,
    data: Option<SignResponseData>,
    message: String,
}

#[derive(Deserialize, Debug)]
struct SignResponseData {
    signature: String,
    public_key: Option<String>,
}

fn sign_message(user_code: &str, crypto_type: &str, message: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::new();
    let kms_url = format!("{}/sign", KMS.get().unwrap());

    let data = serde_json::json!({
        "user_code": user_code,
        "crypto_type": crypto_type,
        "message": message
    });

    let resp = client
        .post(kms_url)
        .json(&data)
        .send()
        .map_err(|e| eyre!("sign_message err: {e}"))?
        .json::<SignResponse>()
        .map_err(|e| eyre!("sign_message err: {e}"))?;

    debug!("sign_message resp: {:?}", resp);

    if resp.code != 200 {
        return Err(eyre!(
            "kms sign_message failed: {},{}",
            resp.code,
            resp.message
        ));
    }

    let data = if let Some(data) = resp.data {
        data
    } else {
        return Err(eyre!(
            "kms sign_message failed: no data, {},{}",
            resp.code,
            resp.message
        ));
    };

    let sig = data.signature;

    let mut sig_vec = parse_data(&sig)?;
    match crypto_type.to_lowercase().as_str() {
        "secp256k1" => {
            match sig_vec[64] {
                27 => sig_vec[64] = 0,
                28 => sig_vec[64] = 1,
                _ => {}
            };
        }
        "sm2" => {
            let public_key = parse_data(&data.public_key.unwrap())?[1..].to_vec();
            sig_vec.extend(public_key);
        }
        _ => unreachable!(),
    }

    Ok(sig_vec)
}

pub trait Kms {
    fn hash(&self, msg: &[u8]) -> Vec<u8>;
    fn address(&self) -> Vec<u8>;
    fn address_str(&self) -> String;
    async fn sign(&self, msg: &str) -> Result<Vec<u8>>;
}

#[derive(Clone, Debug, Default, Reflect, Serialize, Deserialize)]
pub struct Account {
    user_code: String,
    crypto_type: String,
    #[serde(with = "crate::util::bytes_hex")]
    address: Vec<u8>,
}

impl Account {
    pub async fn new(user_code: String, crypto_type: String) -> Result<Self> {
        let address = get_user_address(&user_code, &crypto_type).await?;
        Ok(Self {
            user_code,
            crypto_type,
            address,
        })
    }
}

impl Kms for Account {
    fn hash(&self, msg: &[u8]) -> Vec<u8> {
        match self.crypto_type.to_lowercase().as_str() {
            "sm2" => msg.crypt_hash(Encryption::Sm2).0.to_vec(),
            "scep256k1" => msg.crypt_hash(Encryption::Secp256k1).0.to_vec(),
            _ => unimplemented!(),
        }
    }

    fn address(&self) -> Vec<u8> {
        self.address.clone()
    }

    fn address_str(&self) -> String {
        add_0x(self.address.clone().encode_hex())
    }

    async fn sign(&self, msg: &str) -> Result<Vec<u8>> {
        sign_message(&self.user_code, &self.crypto_type, msg)
    }
}

impl Key for Account {
    fn sign(&self, _message: &[u8], _chain_id: Option<u64>) -> Result<Signature, SigningError> {
        unreachable!("only support EIP1559TX")
    }

    fn sign_message(&self, msg: &[u8]) -> Result<Signature, SigningError> {
        let sig_vec = sign_message(&self.user_code, &self.crypto_type, &hex::encode(msg))
            .map_err(|_| SigningError::InvalidMessage)?;

        if sig_vec.len() != 65 {
            return Err(SigningError::InvalidMessage);
        }

        let r = H256::from_slice(&sig_vec[0..32]);
        let s = H256::from_slice(&sig_vec[32..64]);
        let v = sig_vec[64];

        Ok(Signature { r, s, v: v.into() })
    }

    fn address(&self) -> Address {
        Address::from_slice(&self.address)
    }
}
