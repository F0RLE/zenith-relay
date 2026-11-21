use super::{CHANGED, expected, local};
use crate::local_pool::{DesktopState, hosted_copy};
use serde::Serialize;
use tauri::{State, WebviewWindow};
use uuid::Uuid;
use zenith_relay_hosted_device::{CopyKey, CopyPlan, CopyState};

pub(super) static COPYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PENDING: &str = "copy_review_pending";

#[derive(Serialize)]
pub(crate) struct Review {
    plan: CopyPlan,
    code: String,
}

fn dto(key: CopyKey) -> Review {
    Review {
        plan: key.review().plan.clone(),
        code: key.review().code(),
    }
}

fn pending(
    state: &DesktopState,
    invitation: &str,
    operation: Uuid,
) -> Result<CopyKey, &'static str> {
    let device = expected(invitation)?;
    hosted_copy::load(state)?
        .filter(|k| k.belongs_to(device.target()) && k.review().plan.operation_id == operation)
        .ok_or(CHANGED)
}

#[tauri::command]
pub(crate) async fn list_hosted_copy_sources(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
) -> Result<Vec<hosted_copy::SourceChoice>, &'static str> {
    let state = state.inner().clone();
    local(&window, move || {
        hosted_copy::choices(&state, expected(&invitation)?.target())
    })
    .await
}

#[tauri::command]
pub(crate) async fn get_hosted_copy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
) -> Result<Option<Review>, &'static str> {
    let state = state.inner().clone();
    local(&window, move || {
        let target = expected(&invitation)?;
        let key = hosted_copy::load(&state)?;
        if key.as_ref().is_some_and(|k| !k.belongs_to(target.target())) {
            return Err(CHANGED);
        }
        Ok(key.map(dto))
    })
    .await
}

#[tauri::command]
pub(crate) async fn prepare_hosted_copy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
    source_id: Uuid,
) -> Result<Review, &'static str> {
    let _copy = COPYING.lock().await;
    let state = state.inner().clone();
    let _setup = state.setup_guard().await;
    let work = state.clone();
    local(&window, move || {
        let device = expected(&invitation)?;
        if hosted_copy::load(&work)?.is_some() {
            return Err(PENDING);
        }
        let snap = hosted_copy::snapshot(&work, device.target(), Uuid::new_v4(), source_id)?;
        let key = CopyKey::create(
            device.target(),
            snap.plan,
            &snap.provider,
            snap.proxy.as_ref().map(|v| v.as_str()),
        )?;
        hosted_copy::save(&key)?;
        Ok(dto(key))
    })
    .await
}

#[tauri::command]
pub(crate) async fn hosted_copy_status(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
    operation_id: Uuid,
) -> Result<CopyState, &'static str> {
    let _copy = COPYING.lock().await;
    let state = state.inner().clone();
    let key = local(&window, move || pending(&state, &invitation, operation_id)).await?;
    key.status().await
}

#[tauri::command]
pub(crate) async fn confirm_hosted_copy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
    operation_id: Uuid,
) -> Result<CopyState, &'static str> {
    let _copy = COPYING.lock().await;
    let state = state.inner().clone();
    let expected_invitation = invitation.clone();
    let work = state.clone();
    let key = local(&window, move || {
        pending(&work, &expected_invitation, operation_id)
    })
    .await?;
    let source_id = key.review().plan.source_id;
    key.commit(async || {
        let _setup = state.setup_guard().await;
        let work = state.clone();
        local(&window, move || {
            let device = expected(&invitation)?;
            hosted_copy::snapshot(&work, device.target(), operation_id, source_id)
        })
        .await
    })
    .await
}

#[tauri::command]
pub(crate) async fn clear_hosted_copy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invitation: String,
    operation_id: Uuid,
) -> Result<(), &'static str> {
    let _copy = COPYING.lock().await;
    let state = state.inner().clone();
    local(&window, move || {
        pending(&state, &invitation, operation_id)?;
        hosted_copy::clear()
    })
    .await
}
