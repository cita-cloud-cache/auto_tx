use crate::{send_tx::AutoTx, AutoTxGlobalState};

use color_eyre::eyre::{OptionExt, Result};
use common_rs::{
    error::CALError,
    restful::{
        axum::{
            extract::{Query, State},
            response::IntoResponse,
        },
        ok, RESTfulError,
    },
};
use serde_json::Value;
use std::sync::Arc;

pub async fn get_receipt(
    State(state): State<Arc<AutoTxGlobalState>>,
    Query(params): Query<Value>,
) -> Result<impl IntoResponse, RESTfulError> {
    let hash = params
        .get("hash")
        .ok_or_eyre("hash is missing")?
        .to_string();
    let chain_name = params
        .get("chain_name")
        .ok_or_eyre("chain_name is missing")?
        .to_string();
    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;
    let result = chain.chain_client.get_receipt(&hash).await.map_err(|e| {
        warn!("get_receipt failed: {e:?}");
        CALError::NotFound
    })?;
    ok(result.to_json())
}
