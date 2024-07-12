use crate::{task::Status, AutoTxGlobalState};

use color_eyre::eyre::Result;
use common_rs::{
    error::CALError,
    restful::{
        axum::{extract::State, http::HeaderMap, response::IntoResponse},
        err, ok, RESTfulError,
    },
};
use std::sync::Arc;

pub async fn get_task(
    State(state): State<Arc<AutoTxGlobalState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RESTfulError> {
    let init_hash = if let Some(init_hash) = headers.get("hash") {
        init_hash.to_str()?.to_string()
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

    match task.status {
        Status::Uncheck | Status::Unsend => {
            state
                .storage
                .send_processing_task(&task.init_hash, &task.status)
                .await
                .ok();
        }
        _ => {}
    }

    ok(serde_json::to_value(task)?)
}
