use super::start_catalog_job;
use crate::state::AppState;
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};

pub(super) fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    start_catalog_job(state, shutdown, |state| state.model_metadata_loader())
}
