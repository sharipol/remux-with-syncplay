use axum::{
    Json,
    extract::{Path, State},
};
use axum_anyhow::ApiResult as Result;
use http::StatusCode;
use remux_macros::{get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, ResultExt, db::auth, syncplay::GroupInfo};

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