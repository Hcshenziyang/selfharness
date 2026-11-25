//! 网页通过 [`enqueue`] 提交消息，由 [`AppState::run_queue`] 调用 [`agent_loop::run_turn`]。
//! 回合结束后，[`TurnQueues::finish`] 处理结果并取下一条消息。

mod turns;

use crate::{agent_loop, conversation::ConversationStore, llm::ModelClient, tools};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, error::Error, net::Ipv4Addr, sync::Arc};
use tokio::sync::{Mutex, oneshot};
use tokio_util::task::TaskTracker;
use turns::TurnQueues;

// ******************************** 服务状态与后台任务 ********************************

/// 返回给网页的 HTTP 状态码和 JSON 错误正文。
type ApiError = (StatusCode, Json<Value>);
/// 网页 API 处理器的统一返回类型。
type ApiResult = Result<Json<Value>, ApiError>;

/// 网页请求和后台会话任务共享的模型客户端、队列、任务跟踪器及 MCP 连接。
struct AppState {
    model: ModelClient,
    chats: Mutex<TurnQueues>,
    workers: TaskTracker,
    mcp: tools::McpClient,
    port: u16,
}

impl AppState {
    /// 在同步闭包内持锁访问队列或 SQLite。模型请求和工具调用在锁外等待。
    fn with_chats<T>(&self, operation: impl FnOnce(&mut TurnQueues) -> T) -> T {
        tokio::task::block_in_place(|| operation(&mut self.chats.blocking_lock()))
    }

    /// 单个会话的后台任务。每次取队首都重新读取已提交的历史，然后执行一个用户回合。
    async fn run_queue(self: Arc<Self>, id: i64, mut work: turns::TurnWork) {
        loop {
            let result = match self.with_chats(|chats| chats.store.load(id)) {
                Ok(conversation) => tokio::select! {
                    biased;
                    _ = work.cancel.cancelled() => Err(turns::cancelled()),
                    result = agent_loop::run_turn(&self.model, conversation, &work.message, &self.mcp) =>
                        result.map_err(turn_error),
                },
                Err(error) => Err(database_error(error)),
            };
            match self.with_chats(|chats| chats.finish(id, result)) {
                Some(next) => work = next,
                None => return,
            }
        }
    }

    /// 两种提交入口共用同一条 FIFO。只有原本为空的队列需要启动后台任务。
    fn submit(
        self: &Arc<Self>,
        id: i64,
        input: ChatInput,
        reply: Option<oneshot::Sender<ApiResult>>,
    ) -> Result<(), ApiError> {
        let message = input.message.trim();
        if message.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "请输入消息"));
        }
        if let Some(first) = self.with_chats(|chats| chats.enqueue(id, message.into(), reply))? {
            self.workers.spawn(Arc::clone(self).run_queue(id, first));
        }
        Ok(())
    }
}

// ******************************** 本地服务与路由 ********************************

/// 监听本机端口，提供聊天页面与 API。收到退出信号后等已受理的回合结束，再关闭 MCP。
pub(crate) async fn serve(
    model: ModelClient,
    store: ConversationStore,
    max_turns: usize,
) -> Result<(), Box<dyn Error>> {
    let port: u16 = env::var("DOGO_WEB_PORT")
        .unwrap_or_else(|_| "8787".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(AppState {
        model,
        port,
        chats: Mutex::new(TurnQueues::new(store, max_turns)),
        workers: TaskTracker::new(),
        mcp: tools::McpClient::default(),
    });
    let app = Router::new()
        .route("/", get(index))
        .route("/{file}", get(asset))
        .route(
            "/api/conversations",
            get(conversations).post(create_conversation),
        )
        .route("/api/conversations/{id}/messages", get(history).post(chat))
        .route(
            "/api/conversations/{id}",
            axum::routing::patch(rename_conversation).delete(delete_conversation),
        )
        .route(
            "/api/conversations/{id}/turns",
            axum::routing::post(enqueue),
        )
        .route(
            "/api/conversations/{id}/turns/{turn}",
            axum::routing::delete(stop_turn),
        )
        .route("/api/conversations/{id}/progress", get(progress))
        .route("/api/mcp/servers", get(servers).post(connect))
        .route(
            "/api/mcp/servers/{name}",
            axum::routing::put(edit_server).delete(disconnect),
        )
        .route(
            "/api/mcp/servers/{name}/reconnect",
            axum::routing::post(reconnect),
        )
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
    state.workers.close();
    state.workers.wait().await;
    state.mcp.shutdown().await;
    result?;
    Ok(())
}

// ******************************** 请求校验与错误响应 ********************************

/// 将错误说明包装成前端统一读取的 `error` 字段。
fn api_error(status: StatusCode, error: impl ToString) -> ApiError {
    (status, Json(json!({ "error": error.to_string() })))
}

/// 把会话不存在、消息位置冲突等数据库错误映射到相应的 HTTP 状态码。
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

/// 保留上游模型的 429 限流状态，其余回合执行错误返回 502。
fn turn_error(error: Box<dyn Error>) -> ApiError {
    if error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        == Some(StatusCode::TOO_MANY_REQUESTS)
    {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "模型服务限流，请稍后重试");
    }
    api_error(StatusCode::BAD_GATEWAY, error)
}

/// 检查 Host 及请求携带的 Origin，避免其他网站通过浏览器启动本机命令，并为响应设置安全头。
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

// ******************************** 内嵌网页资源 ********************************

/// 返回聊天页面的 HTML。
async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../web/index.html"),
    )
}

/// 只提供编译时内嵌的前端文件，Markdown 解析也可以完全在本地运行。
async fn asset(Path(file): Path<String>) -> Response {
    let js = "text/javascript; charset=utf-8";
    let (mime, content) = match file.as_str() {
        "app.js" => (js, include_str!("../web/app.js")),
        "ui.js" => (js, include_str!("../web/ui.js")),
        "messages.js" => (js, include_str!("../web/messages.js")),
        "mcp.js" => (js, include_str!("../web/mcp.js")),
        "markdown-it.min.js" => (js, include_str!("../web/vendor/markdown-it.min.js")),
        "style.css" => ("text/css; charset=utf-8", include_str!("../web/style.css")),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, mime)], content).into_response()
}

// ******************************** 会话与聊天接口 ********************************

/// 返回侧栏需要的会话列表。
async fn conversations(State(state): State<Arc<AppState>>) -> ApiResult {
    let list = state
        .with_chats(|chats| chats.store.list())
        .map_err(database_error)?;
    Ok(Json(json!({ "conversations": list })))
}

/// 创建空会话并返回其 ID，供网页随后发送首条消息。
async fn create_conversation(State(state): State<Arc<AppState>>) -> ApiResult {
    let info = state
        .with_chats(|chats| chats.store.create())
        .map_err(database_error)?;
    Ok(Json(json!(info)))
}

#[derive(Deserialize)]
struct RenameInput {
    title: String,
}

/// 修改侧栏标题，保留原有消息。
async fn rename_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<RenameInput>,
) -> ApiResult {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "会话标题不能为空"));
    }
    let info = state
        .with_chats(|chats| chats.store.rename(id, title))
        .map_err(database_error)?;
    Ok(Json(json!(info)))
}

/// 删除历史后，旧会话地址失效。
async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.with_chats(|chats| chats.delete_conversation(id))?;
    Ok(StatusCode::NO_CONTENT)
}

/// 在同一次加锁期间读取已保存的消息和内存中的进度，供打开或刷新会话时恢复页面。
async fn history(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    state.with_chats(|chats| {
        let conversation = chats.store.load(id).map_err(database_error)?;
        Ok(Json(
            json!({ "conversation": conversation.info, "messages": conversation.visible_messages(),
            "progress": chats.progress(id) }),
        ))
    })
}

/// 供页面轮询指定会话的队列和最近失败记录，省去重复读取聊天历史。
async fn progress(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult {
    state.with_chats(|chats| {
        chats.store.info(id).map_err(database_error)?;
        Ok(Json(chats.progress(id)))
    })
}

/// 网页发送一条用户消息时的 JSON 请求体，会话 ID 来自路由路径。
#[derive(Deserialize)]
struct ChatInput {
    message: String,
}

/// 通过 [`TurnQueues::enqueue`] 受理消息。队列从空变为非空时启动后台任务，
/// 当前 HTTP 请求通过 oneshot 等待本条消息的结果。
async fn chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<ChatInput>,
) -> ApiResult {
    let (reply, result) = oneshot::channel();
    state.submit(id, input, Some(reply))?;
    // HTTP 断开只会丢弃接收端。后台任务负责排完这个会话里已受理的消息。
    result
        .await
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "会话任务中断"))?
}

/// 网页只等待入队确认，随后通过历史和进度接口跟踪回合。
async fn enqueue(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<ChatInput>,
) -> Result<StatusCode, ApiError> {
    state.submit(id, input, None)?;
    Ok(StatusCode::ACCEPTED)
}

/// 停止指定回合，同会话排在后面的消息继续执行。
async fn stop_turn(
    State(state): State<Arc<AppState>>,
    Path((id, turn)): Path<(i64, u64)>,
) -> Result<StatusCode, ApiError> {
    state.with_chats(|chats| chats.cancel(id, turn))?;
    Ok(StatusCode::NO_CONTENT)
}

// ******************************** MCP 服务管理接口 ********************************

/// 返回已注册的 MCP 服务及各自的工具。
async fn servers(State(state): State<Arc<AppState>>) -> ApiResult {
    Ok(Json(json!({ "servers": state.mcp.servers().await })))
}

/// 分别读取服务名称和连接配置，注册成功后返回最新服务列表。
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

/// 按服务名断开连接，并把更新后的列表返回网页。
async fn disconnect(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult {
    state
        .mcp
        .remove_server(&name)
        .await
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    servers(State(state)).await
}

/// 验证新配置并连接成功后才替换同名服务。
async fn edit_server(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(config): Json<tools::ServerConfig>,
) -> ApiResult {
    state
        .mcp
        .replace_server(name, Some(config))
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    servers(State(state)).await
}

/// 用服务端保存的配置重连，凭据始终留在注册表内。
async fn reconnect(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult {
    state
        .mcp
        .replace_server(name, None)
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    servers(State(state)).await
}
