use crate::{storage::AutoTxStorage, AutoTxGlobalState, RequestParams};
use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use common_rs::restful::{ok, RESTfulError};
use std::sync::Arc;

pub async fn get_onchain_hash(
    headers: HeaderMap,
    State(state): State<Arc<AutoTxGlobalState>>,
    Json(params): Json<RequestParams>,
) -> std::result::Result<impl IntoResponse, RESTfulError> {
    debug!("params: {:?}", params);

    // get request_key
    let request_key = headers
        .get("request_key")
        .ok_or(anyhow::anyhow!("no request_key in header"))?
        .to_str()?;
    let user_code = headers
        .get("user_code")
        .ok_or(anyhow::anyhow!("user_code missing"))?
        .to_str()?;

    let request_key = user_code.to_string() + "-" + request_key;

    let result = state.storage.get_done(&request_key).await?;

    ok(result.to_json())
}
