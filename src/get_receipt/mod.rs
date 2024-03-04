use crate::{send_tx::AutoTx, AutoTxGlobalState};

use color_eyre::eyre::{eyre, OptionExt, Result};
use common_rs::{
    error::CALError,
    restful::{ok, RESTfulError},
};
use salvo::prelude::*;
use std::sync::Arc;

#[handler]
pub async fn get_receipt(depot: &Depot, req: &Request) -> Result<impl Writer, RESTfulError> {
    let hash = req.param::<String>("hash").ok_or_eyre("hash is missing")?;
    let chain_name = req
        .param::<String>("chain_name")
        .ok_or_eyre("chain_name is missing")?;

    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    // get Chain
    let mut chain = state.chains.get_chain(&chain_name).await?;
    let result = chain.chain_client.get_receipt(&hash).await.map_err(|e| {
        warn!("get_receipt failed: {e:?}");
        CALError::NotFound
    })?;
    ok(result.to_json())
}
