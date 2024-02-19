use crate::AutoTxGlobalState;

use color_eyre::eyre::{eyre, Result};
use common_rs::{
    error::CALError,
    restful::{err, ok, RESTfulError},
};
use salvo::prelude::*;
use std::sync::Arc;

#[handler]
pub async fn get_onchain_hash(depot: &Depot, req: &Request) -> Result<impl Writer, RESTfulError> {
    let headers = req.headers();
    // get request_key
    let request_key = if let Some(request_key) = headers.get("request_key") {
        request_key.to_str()?
    } else {
        return err(CALError::BadRequest, "request_key missing");
    };
    let user_code = if let Some(user_code) = headers.get("user_code") {
        user_code.to_str()?
    } else {
        return err(CALError::BadRequest, "user_code missing");
    };

    let request_key = user_code.to_string() + "-" + request_key;

    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    let result = state
        .storage
        .load_auto_tx_result(&request_key)
        .await
        .map_err(|e| {
            warn!("load_auto_tx_result failed: {e}");
            CALError::NotFound
        })?;

    ok(result.to_json())
}
