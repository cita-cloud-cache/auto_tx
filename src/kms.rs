use anyhow::Result;
use cita_tool::{Encryption, Hashable};
use ethabi::ethereum_types::H256;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
// use tiny_keccak::{Hasher, Keccak};
use web3::{
    signing::{Key, Signature, SigningError},
    types::Address,
};

use crate::util::parse_data;

static KMS: OnceCell<String> = OnceCell::new();

pub fn set_kms(s: String) {
    KMS.get_or_init(|| s);
}

#[derive(Deserialize, Debug)]
struct AddrResponse {
    code: i32,
    data: AddrResponseData,
    message: String,
}

#[derive(Deserialize, Debug)]
struct AddrResponseData {
    address: String,
}

async fn get_user_address(user_code: &str, crypto_type: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let kms_url = KMS.get().unwrap().clone() + "/api/keys";

    let data = serde_json::json!({
        "user_code": user_code,
        "crypto_type": crypto_type
    });

    let resp = client
        .post(kms_url)
        .json(&data)
        .send()
        .await?
        .json::<AddrResponse>()
        .await?;

    if resp.code != 200 {
        return Err(anyhow::anyhow!(resp.message));
    }

    parse_data(&resp.data.address)
}

#[derive(Deserialize, Debug)]
struct SignResponse {
    code: i32,
    data: SignResponseData,
    message: String,
}

#[derive(Deserialize, Debug)]
struct SignResponseData {
    signature: String,
}

async fn sign_message(user_code: &str, crypto_type: &str, message: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let kms_url = KMS.get().unwrap().clone() + "/api/keys/sign";

    let data = serde_json::json!({
        "user_code": user_code,
        "crypto_type": crypto_type,
        "message": message
    });

    let resp = client
        .post(kms_url)
        .json(&data)
        .send()
        .await
        .unwrap()
        .json::<SignResponse>()
        .await
        .unwrap();
    let sig = resp.data.signature;
    warn!("sig: {}", sig);

    if resp.code != 200 {
        return Err(anyhow::anyhow!(resp.message));
    }

    parse_data(&sig)
}

// fn keccak_hash(input: &[u8]) -> Vec<u8> {
//     let mut result = [0u8; 32];
//     let mut keccak = Keccak::v256();
//     keccak.update(input);
//     keccak.finalize(&mut result);
//     result.to_vec()
// }

// fn sm3_hash(input: &[u8]) -> Vec<u8> {
//     let mut result = [0u8; 32];
//     result.copy_from_slice(libsm::sm3::hash::Sm3Hash::new(input).get_hash().as_ref());
//     result.to_vec()
// }

#[axum::async_trait]
pub trait Kms {
    fn hash(&self, msg: &[u8]) -> Vec<u8>;
    fn address(&self) -> Vec<u8>;
    async fn sign(&self, msg: &str) -> Result<Vec<u8>>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    user_code: String,
    crypto_type: String,
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

#[axum::async_trait]
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

    async fn sign(&self, msg: &str) -> Result<Vec<u8>> {
        sign_message(&self.user_code, &self.crypto_type, msg).await
    }
}

#[axum::async_trait]
impl Key for Account {
    fn sign(
        &self,
        _message: &[u8],
        _chain_id: Option<u64>,
    ) -> std::result::Result<Signature, SigningError> {
        unreachable!()
    }

    async fn sign_message(&self, message: &[u8]) -> std::result::Result<Signature, SigningError> {
        let sig_vec = sign_message(&self.user_code, &self.crypto_type, &hex::encode(message))
            .await
            .map_err(|_| SigningError::InvalidMessage)?;

        if sig_vec.len() != 65 {
            return Err(SigningError::InvalidMessage);
        }

        let r = H256::from_slice(&sig_vec[0..32]);
        let s = H256::from_slice(&sig_vec[32..64]);
        let v = sig_vec[64] - 27;

        Ok(Signature { r, s, v: v.into() })
    }

    fn address(&self) -> Address {
        Address::from_slice(&self.address)
    }
}
