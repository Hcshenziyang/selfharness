use super::{ApiError, ApiResult, AppState, api_error, database_error};
use crate::agent::{AgentSettings, DEFAULT_AGENT_ID};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub(super) fn store_error(error: rusqlite::Error) -> ApiError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => api_error(StatusCode::NOT_FOUND, "助手不存在"),
        other => database_error(other),
    }
}

pub(super) async fn list(State(state): State<Arc<AppState>>) -> ApiResult {
    let agents = state
        .with_chats(|chats| chats.store.agents())
        .map_err(store_error)?;
    Ok(Json(
        json!({ "default_agent_id": DEFAULT_AGENT_ID, "agents": agents }),
    ))
}

pub(super) async fn get(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    let agent = state
        .with_chats(|chats| chats.store.agent(id))
        .map_err(store_error)?;
    Ok(Json(json!(agent)))
}

pub(super) async fn create(
    State(state): State<Arc<AppState>>,
    Json(settings): Json<AgentSettings>,
) -> ApiResult {
    let settings = settings
        .validate()
        .map_err(|reason| api_error(StatusCode::BAD_REQUEST, reason))?;
    let agent = state
        .with_chats(|chats| chats.store.create_agent(&settings))
        .map_err(store_error)?;
    Ok(Json(json!(agent)))
}

#[derive(Deserialize)]
pub(super) struct AgentUpdate {
    revision: i64,
    #[serde(flatten)]
    settings: AgentSettings,
}

pub(super) async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(edit): Json<AgentUpdate>,
) -> ApiResult {
    let settings = edit
        .settings
        .validate()
        .map_err(|reason| api_error(StatusCode::BAD_REQUEST, reason))?;
    let agent = state
        .with_chats(|chats| chats.store.update_agent(id, edit.revision, &settings))
        .map_err(store_error)?
        .ok_or_else(|| {
            api_error(
                StatusCode::CONFLICT,
                "助手设置已更新，请重新打开编辑窗口查看最新内容",
            )
        })?;
    Ok(Json(json!(agent)))
}
