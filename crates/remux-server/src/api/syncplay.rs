use axum::{
    Json,
    extract::{Path, State},
};
use axum_anyhow::ApiResult as Result;
use http::StatusCode;
use remux_macros::{get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, OptionExt, db::auth, syncplay::GroupInfo};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NewGroupRequest {
    group_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct JoinGroupRequest {
    group_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SetNewQueueRequest {
    playing_queue: Vec<Uuid>,
    playing_item_position: usize,
    start_position_ticks: i64,
}

#[post("/syncplay/setnewqueue")]
pub async fn set_new_queue(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<SetNewQueueRequest>,
) -> Result<StatusCode> {
    match state.ctx.syncplay.set_new_queue(
        &session.device.id,
        body.playing_queue,
        body.playing_item_position,
        body.start_position_ticks,
    ) {
        Ok((group_id, queue)) => {
            tracing::info!(
                %group_id,
                device_id = %session.device.id,
                item_id = %queue.item_ids[queue.playing_index],
                queue_length = queue.item_ids.len(),
                position_ticks = queue.position_ticks,
                "SyncPlay group queue updated"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(reason) => {
            tracing::warn!(
                device_id = %session.device.id,
                %reason,
                "SyncPlay SetNewQueue rejected"
            );
            Ok(StatusCode::BAD_REQUEST)
        }
    }
}

#[post("/syncplay/new")]
pub async fn create_group(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<NewGroupRequest>,
) -> Result<StatusCode> {
    let name = body.group_name.trim();
    if name.is_empty() {
        return Ok(StatusCode::BAD_REQUEST);
    }

    state
        .ctx
        .syncplay
        .create(&session.device.id, name.to_owned());

    Ok(StatusCode::NO_CONTENT)
}

#[get("/syncplay/list")]
pub async fn list_groups(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<Json<Vec<GroupInfo>>> {
    Ok(Json(state.ctx.syncplay.list()))
}

#[get("/syncplay/{group_id}")]
pub async fn get_group(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(group_id): Path<Uuid>,
) -> Result<Json<GroupInfo>> {
    let group = state
        .ctx
        .syncplay
        .get(group_id)
        .context_not_found("SyncPlay group not found")?;
    Ok(Json(group))
}

#[post("/syncplay/join")]
pub async fn join_group(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<JoinGroupRequest>,
) -> Result<StatusCode> {
    state
        .ctx
        .syncplay
        .join(&session.device.id, body.group_id)
        .context_not_found("SyncPlay group not found")?;
    Ok(StatusCode::NO_CONTENT)
}

#[post("/syncplay/leave")]
pub async fn leave_group(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<StatusCode> {
    state.ctx.syncplay.leave(&session.device.id);
    Ok(StatusCode::NO_CONTENT)
}