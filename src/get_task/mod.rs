use crate::AutoTxGlobalState;

use color_eyre::eyre::{eyre, Result};
use common_rs::{
    error::CALError,
    restful::{err, ok, RESTfulError},
};
use salvo::prelude::*;
use std::sync::Arc;

#[handler]
pub async fn get_task(depot: &Depot, req: &Request) -> Result<impl Writer, RESTfulError> {
    let headers = req.headers().clone();
    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    let init_hash = if let Some(user_code) = headers.get("hash") {
        user_code.to_str()?.to_string()
    } else {
        let request_key = if let Some(request_key) = headers.get("request_key") {
            request_key.to_str()?
        } else {
            return err(CALError::BadRequest, "hash or request_key missing");
        };
        let user_code = if let Some(user_code) = headers.get("user_code") {
            user_code.to_str()?
        } else {
            return err(CALError::BadRequest, "hash or user_code missing");
        };
        let request_key = format!("{user_code}-{request_key}");
        state
            .storage
            .load_init_hash_by_request_key(&request_key)
            .await
            .map_err(|_| CALError::NotFound)?
    };

    let task = state
        .storage
        .load_task(&init_hash)
        .await
        .map_err(|_| CALError::NotFound)?;

    ok(serde_json::to_value(task)?)
}
