use crate::{storage::AutoTxStorage, util::add_0x, AutoTxGlobalState, RequestParams};
use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use common_rs::restful::{ok, RESTfulError};
use serde_json::json;
use std::sync::Arc;

pub async fn get_onchain_hash(
    headers: HeaderMap,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    debug!("params: {:?}", params);

    // get req_key
    let req_key = headers
        .get("key")
        .ok_or(anyhow::anyhow!("no key in header"))?
        .to_str()?;

    let mut result = state.storage.get_done(req_key).await?;
    let is_success = if result.len() == 64 {
        result = add_0x(result);
        true
    } else {
        false
    };

    ok(json!({
        "is_success": is_success,
        "result": result
    }))
}
