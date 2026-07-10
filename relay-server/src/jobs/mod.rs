mod health_probe;
mod quota_refresh;
mod wake_automation;

use crate::state::AppState;
use std::sync::Arc;

pub fn start(state: Arc<AppState>) {
    health_probe::start(state.clone());
    quota_refresh::start(state.clone());
    wake_automation::start(state);
}
