use crate::{agent_loop, config::Config, conversation::ConversationStore, tools};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    error::Error,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::task::TaskTracker;

type ApiError = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, ApiError>;

struct AppState {
    config: Config,
    client: Client,
    store: Mutex<ConversationStore>,
    conversation_locks: AsyncMutex<HashMap<i64, Arc<AsyncMutex<()>>>>,
    turns: TaskTracker,
    mcp: tools::McpClient,
    port: u16,
}

impl AppState {
    // 只在读写 SQLite 时占用连接。在阻塞区执行，让 Tokio 继续调度其他请求。
    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut ConversationStore) -> rusqlite::Result<T>,
    ) -> Result<T, ApiError> {
        tokio::task::block_in_place(|| {
            let mut store = self
                .store
                .lock()
                .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "数据库连接不可用"))?;
            operation(&mut store).map_err(database_error)
        })
    }

    async fn lock_conversation(&self, id: i64) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.conversation_locks.lock().await;
            // 保留正在运行或排队的会话。空闲会话下一次发消息时可以新建锁。
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(locks.entry(id).or_default())
        };
        lock.lock_owned().await
    }
}

#[derive(Deserialize)]
struct ChatInput {
    message: String,
}

pub(crate) async fn serve(
    config: Config,
    client: Client,
    store: ConversationStore,
) -> Result<(), Box<dyn Error>> {
    let port: u16 = env::var("DOGO_WEB_PORT")
        .unwrap_or_else(|_| "8787".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(AppState {
        config,
        client,
        port,
        store: Mutex::new(store),
        conversation_locks: AsyncMutex::default(),
        turns: TaskTracker::new(),
        mcp: tools::McpClient::default(),
    });
    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/style.css", get(style))
        .route(
            "/api/conversations",
            get(conversations).post(create_conversation),
        )
        .route("/api/conversations/{id}/messages", get(history).post(chat))
        .route("/api/mcp/servers", get(servers).post(connect))
        .route("/api/mcp/servers/{name}", axum::routing::delete(disconnect))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            check_origin,
        ))
        .with_state(Arc::clone(&state));
    println!("DogOHarness 网页  http://127.0.0.1:{port}");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    state.turns.close();
    state.turns.wait().await;
    state.mcp.shutdown().await;
    result?;
    Ok(())
}

fn database_error(error: rusqlite::Error) -> ApiError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => api_error(StatusCode::NOT_FOUND, "会话不存在"),
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            api_error(StatusCode::CONFLICT, "会话已更新，请重新打开会话")
        }
        _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// 仅允许本机 Host 和本网页来源，避免其他网站通过浏览器请求启动本机命令。
async fn check_origin(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let local_host = format!("127.0.0.1:{}", state.port);
    let localhost = format!("localhost:{}", state.port);
    if host != Some(local_host.as_str()) && host != Some(localhost.as_str()) {
        return (StatusCode::FORBIDDEN, "仅接受本机请求").into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let local_origin = format!("http://{local_host}");
        let localhost_origin = format!("http://{localhost}");
        if origin.as_bytes() != local_origin.as_bytes()
            && origin.as_bytes() != localhost_origin.as_bytes()
        {
            return (StatusCode::FORBIDDEN, "不允许的页面来源").into_response();
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; base-uri 'none'; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    response
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../web/index.html"),
    )
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/app.js"),
    )
}

async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../web/style.css"),
    )
}

fn api_error(status: StatusCode, error: impl ToString) -> ApiError {
    (status, Json(json!({ "error": error.to_string() })))
}

async fn conversations(State(state): State<Arc<AppState>>) -> ApiResult {
    let list = state.with_store(|store| store.list())?;
    Ok(Json(json!({ "conversations": list })))
}

async fn create_conversation(State(state): State<Arc<AppState>>) -> ApiResult {
    let info = state.with_store(|store| store.create())?;
    Ok(Json(json!(info)))
}

async fn history(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    let conversation = state.with_store(|store| store.load(id))?;
    Ok(Json(json!({ "messages": conversation.visible_messages() })))
}

async fn chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<ChatInput>,
) -> ApiResult {
    let message = input.message.trim().to_string();
    if message.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "请输入消息"));
    }
    let turn = state.lock_conversation(id).await;
    let turn_state = Arc::clone(&state);
    // 已开始的回合在浏览器断开后仍然执行并保存。
    state
        .turns
        .spawn(async move {
            let _turn = turn;
            let conversation = turn_state.with_store(|store| store.load(id))?;
            let tools = turn_state.mcp.tools().await;
            let completed = agent_loop::run_turn(
                &turn_state.client,
                &turn_state.config,
                conversation,
                &message,
                &tools,
            )
            .await
            .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error))?;
            let info = turn_state.with_store(|store| store.save_turn(&completed))?;
            Ok(Json(
                json!({ "conversation": info, "messages": completed.visible_messages() }),
            ))
        })
        .await
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, error))?
}

async fn servers(State(state): State<Arc<AppState>>) -> ApiResult {
    Ok(Json(json!({ "servers": state.mcp.servers().await })))
}

async fn connect(State(state): State<Arc<AppState>>, Json(mut input): Json<Value>) -> ApiResult {
    let object = input
        .as_object_mut()
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "配置必须是 JSON 对象"))?;
    let name = object
        .remove("name")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "请填写服务名称"))?;
    let config: tools::ServerConfig = serde_json::from_value(input)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, format!("配置格式错误：{error}")))?;
    state
        .mcp
        .add_server(name, config)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    servers(State(state)).await
}

async fn disconnect(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult {
    state
        .mcp
        .remove_server(&name)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    servers(State(state)).await
}
