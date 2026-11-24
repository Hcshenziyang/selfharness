use crate::{agent_loop, config::Config, conversation::ConversationStore, tools};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    net::Ipv4Addr,
    sync::mpsc::{self, SyncSender},
};
use tokio::sync::oneshot;

type ApiError = (StatusCode, Json<Value>);
type Reply = oneshot::Sender<Result<Value, ApiError>>;
type ApiResult = Result<Json<Value>, ApiError>;

enum Action {
    ListConversations(Reply),
    CreateConversation(Reply),
    History(i64, Reply),
    Chat(i64, String, Reply),
    Servers(Reply),
    Connect(String, tools::ServerConfig, Reply),
    Disconnect(String, Reply),
    Shutdown,
}

#[derive(Clone)]
struct AppState {
    actions: SyncSender<Action>,
    port: u16,
}

#[derive(Deserialize)]
struct ChatInput {
    message: String,
}

pub(crate) fn serve(
    config: Config,
    client: Client,
    mut store: ConversationStore,
) -> Result<(), Box<dyn Error>> {
    let port: u16 = env::var("DOGO_WEB_PORT")
        .unwrap_or_else(|_| "8787".into())
        .parse()?;
    let (actions, receiver) = mpsc::sync_channel(16);
    let worker = std::thread::spawn(move || {
        // MCP 的异步运行时属于这个线程，模型请求和工具调用也在此串行执行。
        let mut mcp = match tools::McpClient::empty() {
            Ok(mcp) => mcp,
            Err(error) => {
                eprintln!("无法初始化 MCP：{error}");
                return;
            }
        };
        while let Ok(action) = receiver.recv() {
            match action {
                Action::ListConversations(reply) => {
                    let result = store.list().map(|list| json!({ "conversations": list }));
                    let _ = reply.send(result.map_err(database_error));
                }
                Action::CreateConversation(reply) => {
                    let result = store.create().map(|info| json!(info));
                    let _ = reply.send(result.map_err(database_error));
                }
                Action::History(id, reply) => {
                    let result = store
                        .load(id)
                        .map(|conversation| json!({ "messages": conversation.visible_messages() }));
                    let _ = reply.send(result.map_err(database_error));
                }
                Action::Chat(id, input, reply) => {
                    let _ = reply.send(chat_turn(&mut store, &client, &config, &mcp, id, &input));
                }
                Action::Servers(reply) => {
                    let _ = reply.send(Ok(json!({ "servers": mcp.servers() })));
                }
                Action::Connect(name, server, reply) => {
                    let result = mcp
                        .add_server(name, server)
                        .map(|()| json!({ "servers": mcp.servers() }))
                        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error));
                    let _ = reply.send(result);
                }
                Action::Disconnect(name, reply) => {
                    let result = mcp
                        .remove_server(&name)
                        .map(|()| json!({ "servers": mcp.servers() }))
                        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error));
                    let _ = reply.send(result);
                }
                Action::Shutdown => break,
            }
        }
        // 网页服务退出时，先关闭连接，让 stdio 子进程正常结束。
        for server in mcp.servers() {
            if let Some(name) = server["name"].as_str() {
                let _ = mcp.remove_server(name);
            }
        }
    });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let port = listener.local_addr()?.port();
        let state = AppState {
            actions: actions.clone(),
            port,
        };
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
            .layer(middleware::from_fn_with_state(state.clone(), check_origin))
            .with_state(state);
        println!("DogOHarness 网页  http://127.0.0.1:{port}");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    });
    let _ = actions.send(Action::Shutdown);
    let _ = worker.join();
    result?;
    Ok(())
}

fn chat_turn(
    store: &mut ConversationStore,
    client: &Client,
    config: &Config,
    mcp: &tools::McpClient,
    id: i64,
    input: &str,
) -> Result<Value, ApiError> {
    let conversation = store.load(id).map_err(database_error)?;
    let completed = agent_loop::run_turn(client, config, conversation, input, mcp)
        .map_err(|error| api_error(StatusCode::BAD_GATEWAY, error))?;
    let info = store.save_turn(&completed).map_err(database_error)?;
    Ok(json!({ "conversation": info, "messages": completed.visible_messages() }))
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
async fn check_origin(State(state): State<AppState>, request: Request, next: Next) -> Response {
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

async fn request_action(state: &AppState, action: impl FnOnce(Reply) -> Action) -> ApiResult {
    let (reply, result) = oneshot::channel();
    state
        .actions
        .try_send(action(reply))
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "服务忙，请稍后再试"))?;
    result
        .await
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "服务已停止"))?
        .map(Json)
}

fn api_error(status: StatusCode, error: impl ToString) -> ApiError {
    (status, Json(json!({ "error": error.to_string() })))
}

async fn conversations(State(state): State<AppState>) -> ApiResult {
    request_action(&state, Action::ListConversations).await
}

async fn create_conversation(State(state): State<AppState>) -> ApiResult {
    request_action(&state, Action::CreateConversation).await
}

async fn history(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult {
    request_action(&state, |reply| Action::History(id, reply)).await
}

async fn chat(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatInput>,
) -> ApiResult {
    let message = input.message.trim().to_string();
    if message.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "请输入消息"));
    }
    request_action(&state, |reply| Action::Chat(id, message, reply)).await
}

async fn servers(State(state): State<AppState>) -> ApiResult {
    request_action(&state, Action::Servers).await
}

async fn connect(State(state): State<AppState>, Json(mut input): Json<Value>) -> ApiResult {
    let object = input
        .as_object_mut()
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "配置必须是 JSON 对象"))?;
    let name = object
        .remove("name")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "请填写服务名称"))?;
    let config: tools::ServerConfig = serde_json::from_value(input)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, format!("配置格式错误：{error}")))?;
    request_action(&state, |reply| Action::Connect(name, config, reply)).await
}

async fn disconnect(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult {
    request_action(&state, |reply| Action::Disconnect(name, reply)).await
}
