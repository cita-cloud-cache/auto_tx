use crate::{AutoTxGlobalState, RequestParams};
use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use common_rs::restful::{ok, RESTfulError};
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

    // check params
    if params.user_code.is_empty() {
        return Err(anyhow::anyhow!("user_code missing").into());
    }

    let req_key = params.user_code.clone() + "-" + req_key;

    let result = state.storage.load_auto_tx_result(&req_key).await?;

    ok(result.to_json())
}
