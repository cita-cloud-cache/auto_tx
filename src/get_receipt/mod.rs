use crate::{send_tx::AutoTx, AutoTxGlobalState};

use color_eyre::eyre::Result;
use common_rs::{
    error::CALError,
    restful::{
        axum::{
            extract::{Path, State},
            response::IntoResponse,
        },
        ok, RESTfulError,
    },
};
use std::sync::Arc;

pub async fn get_receipt(
    State(state): State<Arc<AutoTxGlobalState>>,
    Path(chain_name): Path<String>,
    Path(hash): Path<String>,
) -> Result<impl IntoResponse, RESTfulError> {
    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;
    let result = chain.chain_client.get_receipt(&hash).await.map_err(|e| {
        warn!("get_receipt failed: {e:?}");
        CALError::NotFound
    })?;
    ok(result.to_json())
}
