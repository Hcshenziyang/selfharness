use super::{ApiError, ApiResult, api_error, database_error};
use crate::conversation::{Conversation, ConversationStore};
use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

// ******************************** 内存中的回合状态 ********************************

/// 一条已受理的用户消息，通过 reply 把结果交给提交它的 HTTP 请求。
struct PendingTurn {
    id: u64,
    message: String,
    reply: oneshot::Sender<ApiResult>,
}

/// 留给刷新页面读取的失败记录，包含所属会话、失败时间、用户原文及错误。
struct FailedTurn {
    conversation_id: i64,
    at: Instant,
    detail: Value,
}

/// 按会话维护后台队列，在回合成功结束时把新增历史写入 SQLite。
pub(super) struct TurnQueues {
    pub(super) store: ConversationStore,
    /// 键是会话 ID。队首归当前后台任务，其余消息按受理顺序等待。
    active: HashMap<i64, VecDeque<PendingTurn>>,
    failures: VecDeque<FailedTurn>,
    next_id: u64,
    limit: usize,
}

// ******************************** 入队、查询进度与完成回合 ********************************

impl TurnQueues {
    /// 初始化所有会话共享的队列，并设置可同时受理的未完成回合总数。
    pub(super) fn new(store: ConversationStore, limit: usize) -> Self {
        Self {
            store,
            active: HashMap::new(),
            failures: VecDeque::new(),
            next_id: 1,
            limit,
        }
    }

    /// 检查会话和全局上限后将消息加到队尾。仅当原队列为空时返回首条消息，让调用方启动任务。
    pub(super) fn enqueue(
        &mut self,
        id: i64,
        message: String,
        reply: oneshot::Sender<ApiResult>,
    ) -> Result<Option<String>, ApiError> {
        self.store.info(id).map_err(database_error)?;
        if self.active.values().map(VecDeque::len).sum::<usize>() >= self.limit {
            return Err(api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "服务繁忙，消息尚未入队，请稍后重试",
            ));
        }
        self.failures.retain(|f| f.conversation_id != id);
        let queue = self.active.entry(id).or_default();
        let first = queue.is_empty().then(|| message.clone());
        queue.push_back(PendingTurn {
            id: self.next_id,
            message,
            reply,
        });
        self.next_id += 1;
        Ok(first)
    }

    /// 返回正在执行的队首、排队消息以及该会话最近五分钟内保留的失败记录，供多个页面轮询。
    pub(super) fn progress(&mut self, id: i64) -> Value {
        self.failures
            .retain(|f| f.at.elapsed() < Duration::from_secs(300));
        let turns: Vec<_> = self
            .active
            .get(&id)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, turn)| {
                json!({
                    "id": turn.id, "message": turn.message,
                    "status": if index == 0 { "running" } else { "queued" }
                })
            })
            .collect();
        let failure = self.failures.iter().find(|f| f.conversation_id == id);
        json!({ "turns": turns, "last_failure": failure.map(|f| &f.detail) })
    }

    /// 结束队首回合，成功时提交历史，失败时保留原文和错误，再通知等待方。
    /// 整个操作在同一把锁内完成。返回下一条待处理消息，队列为空时移除该会话。
    pub(super) fn finish(
        &mut self,
        id: i64,
        result: Result<Conversation, ApiError>,
    ) -> Option<String> {
        let queue = self.active.get_mut(&id)?;
        let turn = queue.pop_front()?;
        let next = queue.front().map(|turn| turn.message.clone());
        if next.is_none() {
            self.active.remove(&id);
        }
        let reply = result.and_then(|completed| {
            let info = self.store.save_turn(&completed).map_err(database_error)?;
            Ok(Json(
                json!({ "conversation": info, "messages": completed.visible_messages(),
                "progress": self.progress(id) }),
            ))
        });
        if let Err((_, body)) = &reply {
            self.failures.retain(|f| f.conversation_id != id);
            self.failures.push_front(FailedTurn {
                conversation_id: id,
                at: Instant::now(),
                detail: json!({ "id": turn.id, "message": turn.message, "error": body.0["error"] }),
            });
            // 全局只保留最近 64 条，每个会话至多一条。多个页面都可以读取同一条失败记录。
            self.failures.truncate(64);
        }
        let _ = turn.reply.send(reply);
        next
    }
}
