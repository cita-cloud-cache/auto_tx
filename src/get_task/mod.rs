use crate::AutoTxGlobalState;

use color_eyre::eyre::{eyre, OptionExt, Result};
use common_rs::{
    error::CALError,
    restful::{ok, RESTfulError},
};
use salvo::prelude::*;
use std::sync::Arc;

#[handler]
pub async fn get_task(depot: &Depot, req: &Request) -> Result<impl Writer, RESTfulError> {
    let hash = req.param::<String>("hash").ok_or_eyre("hash is missing")?;

    let state = depot
        .obtain::<Arc<AutoTxGlobalState>>()
        .map_err(|e| eyre!("get app_state failed: {e:?}"))?;

    let task = state.storage.load_task(&hash).await.map_err(|e| {
        warn!("load_task failed: {e}");
        CALError::NotFound
    })?;

    ok(serde_json::to_value(task)?)
}
