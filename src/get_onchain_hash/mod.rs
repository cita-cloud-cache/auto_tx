use crate::AutoTxGlobalState;

use color_eyre::eyre::{eyre, Result};
use common_rs::restful::{ok, RESTfulError};
use salvo::prelude::*;
use std::sync::Arc;

#[handler]
pub async fn get_onchain_hash(depot: &Depot, req: &Request) -> Result<impl Writer, RESTfulError> {
    let headers = req.headers();
    // get request_key
    let request_key = headers
        .get("request_key")
        .ok_or(eyre!("no request_key in header"))?
        .to_str()?;
    let user_code = headers
        .get("user_code")
        .ok_or(eyre!("user_code missing"))?
        .to_str()?;

    let request_key = user_code.to_string() + "-" + request_key;

    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    let result = state
        .storage
        .load_auto_tx_result(&request_key)
        .await
        .map_err(|_| eyre!("load_auto_tx_result failed: not found"))?;

    ok(result.to_json())
}
