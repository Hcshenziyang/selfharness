use super::{ApiError, ApiResult, AppState, agents, api_error, database_error};
use crate::tools::{McpClient, McpError, ServerConfig, ToolPolicy};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tools", get(policy).put(update_policy))
        .route("/mcp/servers", get(servers).post(connect))
        .route("/mcp/servers/{binding}", put(edit).delete(remove))
        .route("/mcp/servers/{binding}/reconnect", post(reconnect))
        .route("/mcp/servers/{binding}/disconnect", post(disconnect))
        .route("/mcp/servers/{binding}/tools", patch(grants))
}

fn mcp_error(error: McpError) -> ApiError {
    match error.downcast::<rusqlite::Error>() {
        Ok(e) if matches!(*e, rusqlite::Error::QueryReturnedNoRows) => {
            api_error(StatusCode::NOT_FOUND, "服务绑定不存在")
        }
        Ok(e) => database_error(*e),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}

async fn policy(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    let p = state
        .with_chats(|c| c.store.tool_policy(id))
        .map_err(agents::store_error)?;
    Ok(Json(json!(p)))
}

async fn update_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(mut policy): Json<ToolPolicy>,
) -> ApiResult {
    let previous = state
        .with_chats(|chats| chats.store.tool_policy(id))
        .map_err(agents::store_error)?;
    let conflict = || {
        api_error(
            StatusCode::CONFLICT,
            "工具设置已更新，请重新打开查看最新内容",
        )
    };
    if previous.revision != policy.revision {
        return Err(conflict());
    }
    tokio::task::block_in_place(|| policy.validate(previous.workspace_root.as_deref()))
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    if !state
        .with_chats(|chats| chats.store.save_tool_policy(id, &policy))
        .map_err(database_error)?
    {
        return Err(conflict());
    }
    policy.revision += 1;
    Ok(Json(json!(policy)))
}

impl AppState {
    fn mcp(&self, id: i64) -> Result<Arc<McpClient>, ApiError> {
        self.tools.agent(id).map_err(agents::store_error)
    }
}

async fn list(client: &McpClient) -> ApiResult {
    Ok(Json(json!({ "servers": client.servers().await })))
}

async fn servers(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    list(state.mcp(id)?.as_ref()).await
}

async fn connect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(mut input): Json<Value>,
) -> ApiResult {
    let client = state.mcp(id)?;
    let name = input
        .as_object_mut()
        .and_then(|o| o.remove("name"))
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "请填写服务名称"))?;
    let config = serde_json::from_value(input)
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, format!("配置格式错误：{e}")))?;
    client.add_server(name, config).await.map_err(mcp_error)?;
    list(&client).await
}

async fn edit(
    State(state): State<Arc<AppState>>,
    Path((id, binding)): Path<(i64, i64)>,
    Json(config): Json<ServerConfig>,
) -> ApiResult {
    let client = state.mcp(id)?;
    client
        .replace_server(binding, Some(config))
        .await
        .map_err(mcp_error)?;
    list(&client).await
}

async fn reconnect(
    State(state): State<Arc<AppState>>,
    Path((id, binding)): Path<(i64, i64)>,
) -> ApiResult {
    let client = state.mcp(id)?;
    client
        .replace_server(binding, None)
        .await
        .map_err(mcp_error)?;
    list(&client).await
}

async fn disconnect(
    State(state): State<Arc<AppState>>,
    Path((id, binding)): Path<(i64, i64)>,
) -> ApiResult {
    let client = state.mcp(id)?;
    client.disconnect(binding).await.map_err(mcp_error)?;
    list(&client).await
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path((id, binding)): Path<(i64, i64)>,
) -> ApiResult {
    let client = state.mcp(id)?;
    client.remove_server(binding).await.map_err(mcp_error)?;
    list(&client).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantChanges {
    revision: i64,
    tools: BTreeMap<String, bool>,
}

async fn grants(
    State(state): State<Arc<AppState>>,
    Path((id, binding)): Path<(i64, i64)>,
    Json(edit): Json<GrantChanges>,
) -> ApiResult {
    let client = state.mcp(id)?;
    if !client
        .set_grants(binding, edit.revision, edit.tools)
        .await
        .map_err(mcp_error)?
    {
        return Err(api_error(
            StatusCode::CONFLICT,
            "服务的工具清单或授权已更新，请查看最新勾选状态",
        ));
    }
    list(&client).await
}
