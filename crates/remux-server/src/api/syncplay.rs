use axum::{
    Json,
    extract::{Path, State},
};
use axum_anyhow::ApiResult as Result;
use http::StatusCode;
use remux_macros::{get, post};
use serde::Deserialize;
use uuid::Uuid;
use serde_json::json;

use crate::{ AppState, OptionExt, db::auth, syncplay::{GroupInfo, ItemChange}, ws::WsEvent,};

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

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReadyRequest {
    playlist_item_id: String,
    #[serde(default)]
    _when: Option<String>,
    #[serde(default)]
    _position_ticks: Option<i64>,
    #[serde(default)]
    _is_playing: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeekRequest {
    position_ticks: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistItemRequest {
    playlist_item_id: String,
}

fn broadcast_playback_command(
    state: &AppState,
    group_id: Uuid,
    playlist_item_id: Uuid,
    position_ticks: i64,
    members: Vec<String>,
    group_state: &str,
    command_name: &str,
    when: chrono::DateTime<chrono::Utc>,
) {
    let emitted_at = chrono::Utc::now();
    let state_update = json!({
        "GroupId": group_id.to_string(),
        "Type": "StateUpdate",
        "Data": { "State": group_state },
    });
    let command = json!({
        "GroupId": group_id.to_string(),
        "Command": command_name,
        "PositionTicks": position_ticks,
        "When": when.to_rfc3339(),
        "EmittedAt": emitted_at.to_rfc3339(),
        "PlaylistItemId": playlist_item_id.to_string(),
    });

    for device_id in members {
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
            device_id: device_id.clone(),
            data: state_update.clone(),
        });
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayCommand {
            device_id,
            data: command.clone(),
        });
    }
}

fn change_playlist_item(
    state: &AppState,
    device_id: &str,
    requested_id: &str,
    change: ItemChange,
) -> StatusCode {
    let Ok((group_id, queue, members)) =
        state.ctx.syncplay.change_item(device_id, requested_id, change)
    else {
        return StatusCode::CONFLICT;
    };

    let playlist: Vec<_> = queue
        .item_ids
        .iter()
        .zip(&queue.playlist_item_ids)
        .map(|(item_id, playlist_item_id)| {
            json!({
                "ItemId": item_id.to_string(),
                "PlaylistItemId": playlist_item_id.to_string(),
            })
        })
        .collect();

    let state_update = json!({
        "GroupId": group_id.to_string(),
        "Type": "StateUpdate",
        "Data": {
            "State": "Waiting",
            "Reason": change.reason(),
        },
    });

    let queue_update = json!({
        "GroupId": group_id.to_string(),
        "Type": "PlayQueue",
        "Data": {
            "Reason": change.reason(),
            "LastUpdate": chrono::Utc::now().to_rfc3339(),
            "Playlist": playlist,
            "PlayingItemIndex": queue.playing_index,
            "StartPositionTicks": 0,
            "IsPlaying": true,
            "ShuffleMode": "Sorted",
            "RepeatMode": "RepeatNone",
        },
    });

    for device_id in members {
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
            device_id: device_id.clone(),
            data: state_update.clone(),
        });
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
            device_id,
            data: queue_update.clone(),
        });
    }

    StatusCode::NO_CONTENT
}

#[post("/syncplay/nextitem")]
pub async fn next_item(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<PlaylistItemRequest>,
) -> Result<StatusCode> {
    Ok(change_playlist_item(
        &state,
        &session.device.id,
        &body.playlist_item_id,
        ItemChange::Next,
    ))
}

#[post("/syncplay/previousitem")]
pub async fn previous_item(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<PlaylistItemRequest>,
) -> Result<StatusCode> {
    Ok(change_playlist_item(
        &state,
        &session.device.id,
        &body.playlist_item_id,
        ItemChange::Previous,
    ))
}

#[post("/syncplay/setplaylistitem")]
pub async fn set_playlist_item(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<PlaylistItemRequest>,
) -> Result<StatusCode> {
    Ok(change_playlist_item(
        &state,
        &session.device.id,
        &body.playlist_item_id,
        ItemChange::Select,
    ))
}

#[post("/syncplay/pause")]
pub async fn pause(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<StatusCode> {
    let now = chrono::Utc::now();
    let Ok((group_id, item_id, position, members)) =
        state.ctx.syncplay.pause(&session.device.id, now)
    else {
        return Ok(StatusCode::CONFLICT);
    };

    broadcast_playback_command(
        &state, group_id, item_id, position, members, "Paused", "Pause", now,
    );
    Ok(StatusCode::NO_CONTENT)
}

#[post("/syncplay/unpause")]
pub async fn unpause(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<StatusCode> {
    let when = chrono::Utc::now() + chrono::Duration::milliseconds(750);
    let Ok((group_id, item_id, position, members)) =
        state.ctx.syncplay.unpause(&session.device.id, when)
    else {
        return Ok(StatusCode::CONFLICT);
    };

    broadcast_playback_command(
        &state, group_id, item_id, position, members, "Playing", "Unpause", when,
    );
    Ok(StatusCode::NO_CONTENT)
}

#[post("/syncplay/ready")]
pub async fn ready(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<ReadyRequest>,
) -> Result<StatusCode> {
    let result = state
        .ctx
        .syncplay
        .mark_ready(&session.device.id, &body.playlist_item_id);

    let Some((group_id, playlist_item_id, position_ticks, members)) = (match result {
        Ok(value) => value,
        Err(reason) => {
            tracing::warn!(
                device_id = %session.device.id,
                %reason,
                "SyncPlay Ready rejected"
            );
            return Ok(StatusCode::BAD_REQUEST);
        }
    }) else {
        return Ok(StatusCode::NO_CONTENT);
    };

    let emitted_at = chrono::Utc::now();
    let when = emitted_at + chrono::Duration::milliseconds(750);
	
	if let Err(reason) = state
		.ctx
		.syncplay
		.record_play_start(group_id, playlist_item_id, when)
	{
		tracing::warn!(%group_id, %reason, "SyncPlay play start not recorded");
		return Ok(StatusCode::CONFLICT);
	}

    let state_update = serde_json::json!({
        "GroupId": group_id.to_string(),
        "Type": "StateUpdate",
        "Data": {
            "State": "Playing",
            "Reason": "AllReady",
        },
    });

    let command = serde_json::json!({
        "GroupId": group_id.to_string(),
        "Command": "Unpause",
        "PositionTicks": position_ticks,
        "When": when.to_rfc3339(),
        "EmittedAt": emitted_at.to_rfc3339(),
        "PlaylistItemId": playlist_item_id.to_string(),
    });

    for device_id in members {
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
            device_id: device_id.clone(),
            data: state_update.clone(),
        });
        let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayCommand {
            device_id,
            data: command.clone(),
        });
    }

    tracing::info!(%group_id, "SyncPlay group ready; Unpause sent");
    Ok(StatusCode::NO_CONTENT)
}

#[post("/syncplay/seek")]
pub async fn seek(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<SeekRequest>,
) -> Result<StatusCode> {
    let Ok((group_id, item_id, position, members, was_playing)) =
        state.ctx.syncplay.seek(&session.device.id, body.position_ticks)
    else {
        return Ok(StatusCode::CONFLICT);
    };

    let now = chrono::Utc::now();

    if was_playing {
        broadcast_playback_command(
            &state,
            group_id,
            item_id,
            position,
            members,
            "Waiting",
            "Seek",
            now,
        );
    } else {
        broadcast_playback_command(
            &state,
            group_id,
            item_id,
            position,
            members,
            "Paused",
            "Pause",
            now,
        );
    }

    Ok(StatusCode::NO_CONTENT)
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
			let playlist: Vec<_> = queue
				.item_ids
				.iter()
				.zip(&queue.playlist_item_ids)
				.map(|(item_id, playlist_item_id)| {
					json!({
						"ItemId": item_id.to_string(),
						"PlaylistItemId": playlist_item_id.to_string(),
					})
				})
				.collect();

			let state_update = json!({
				"GroupId": group_id.to_string(),
				"Type": "StateUpdate",
				"Data": {
					"State": "Waiting",
					"Reason": "NewPlaylist",
				},
			});

			let queue_update = json!({
				"GroupId": group_id.to_string(),
				"Type": "PlayQueue",
				"Data": {
					"Reason": "NewPlaylist",
					"LastUpdate": "",
					"Playlist": playlist,
					"PlayingItemIndex": queue.playing_index,
					"StartPositionTicks": queue.position_ticks,
					"IsPlaying": true,
					"ShuffleMode": "Sorted",
					"RepeatMode": "RepeatNone",
				},
			});

			for device_id in state.ctx.syncplay.members_for_group(group_id) {
				let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
					device_id: device_id.clone(),
					data: state_update.clone(),
				});
				let _ = state.ctx.ws_tx.send(WsEvent::SyncPlayGroupUpdate {
					device_id,
					data: queue_update.clone(),
				});
			}
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