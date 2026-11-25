import { api, byId, element, setSidebar } from "./ui.js";
import { renderMessages } from "./messages.js";
import "./mcp.js";

// ******************************** 会话导航与草稿 ********************************

const input = byId("message-input");
const form = byId("chat-form");
const list = byId("conversation-list");
const menu = byId("conversation-menu");
const sessions = new Map();
const changes = new BroadcastChannel("dogo-conversations");
let conversations = [];
let view;
let listKey;
let listRequest = 0;

function session(id) {
  if (!sessions.has(id)) sessions.set(id, { draft: "", notice: "", admitting: false, seenFailure: null });
  return sessions.get(id);
}

function remember(info, first = false) {
  const at = conversations.findIndex(item => item.id === info.id);
  if (first || at < 0) conversations = [info, ...conversations.filter(item => item.id !== info.id)];
  else conversations[at] = info;
}

function showList() {
  const running = Boolean(view?.progress?.turns.length);
  const key = JSON.stringify([conversations, view?.id, running]);
  if (key === listKey) return;
  listKey = key;
  const items = conversations.map(info => {
    const selected = view?.id === info.id;
    const button = element("button", "sidebar-action conversation-link", info.title + (selected && running ? " · 回复中" : ""));
    button.type = "button";
    button.title = info.title;
    button.dataset.id = info.id;
    if (selected) button.setAttribute("aria-current", "true");
    return button;
  });
  list.replaceChildren(...items);
  byId("conversation-history").hidden = items.length === 0;
}

function updateHeader(current) {
  byId("conversation-title").textContent = current.info?.title ?? "新对话";
  menu.hidden = !current.id;
  const url = new URL(location.href);
  url.searchParams.set("conversation", current.id ?? "new");
  history.replaceState(null, "", url);
}

function showComposer() {
  const state = view.state;
  if (input.value !== state.draft) input.value = state.draft;
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 180) + "px";
  byId("send-message").disabled = !view.ready || state.admitting || !input.value.trim();
  const errorText = byId("chat-error-text");
  if (errorText.textContent !== state.notice) errorText.textContent = state.notice;
  byId("chat-error").hidden = !state.notice;
}

input.addEventListener("input", () => {
  view.state.draft = input.value;
  showComposer();
});
byId("dismiss-error").addEventListener("click", () => {
  view.state.notice = "";
  if (view.state.noticeFailure != null) {
    try { sessionStorage.setItem("dogo.dismissed." + view.id, String(view.state.noticeFailure)); }
    catch { /* 无持久存储时，seenFailure 仍能记住本页已看过的失败。 */ }
  }
  view.state.noticeFailure = null;
  showComposer();
});

function dismissed(id, failureId) {
  try { return sessionStorage.getItem("dogo.dismissed." + id) === String(failureId); }
  catch { return false; }
}

/** 切换时丢弃旧页面的轮询，保留该会话的草稿与错误。空白页直到首次发送才创建数据库记录。 */
function openView(info, writeUrl = true) {
  clearTimeout(view?.timer);
  const id = info?.id ?? null;
  const current = { id, info, state: session(id), ready: !id, request: 0, progress: null };
  view = current;
  menu.open = false;
  if (writeUrl) updateHeader(current);
  renderMessages([], [], stopTurn, true);
  showList();
  showComposer();
  if (id) refresh(current);
  return current;
}

function newChat() {
  if (view.id || !view.explicit) openView(null);
  view.explicit = true;
  updateHeader(view);
  setSidebar(false);
  input.focus();
}

byId("brand-home").addEventListener("click", newChat);
byId("new-chat").addEventListener("click", newChat);
list.addEventListener("click", event => {
  const id = Number(event.target.closest("button[data-id]")?.dataset.id);
  const info = conversations.find(item => item.id === id);
  if (!info) return;
  if (id !== view.id) openView(info);
  setSidebar(false);
});

function notice(state, message, failureId = null) {
  state.notice = message;
  state.noticeFailure = failureId;
  if (state === view.state) showComposer();
}

/** 新草稿优先。只有输入框为空时，失败原文才自动填回。 */
function recover(state, text, error, failureId = null) {
  const otherDraft = state.draft && state.draft !== text;
  if (!state.draft) state.draft = text;
  notice(state, otherDraft ? error + "（原消息：" + text + "）" : error, failureId);
}

// ******************************** 快照、轮询与停止 ********************************

function chatPath(id) {
  return "/api/conversations/" + id;
}

function schedule(current, full = false) {
  clearTimeout(current.timer);
  if (view === current && (full || current.progress?.turns.length)) {
    current.timer = setTimeout(() => refresh(current, !full), 1000);
  }
}

/** 只有当前视图最后一次请求可以更新 DOM。平时只查进度，有变化才读取完整历史。 */
async function refresh(current, progressOnly = false) {
  if (current !== view || !current.id) return;
  clearTimeout(current.timer);
  const request = ++current.request;
  const latest = () => view === current && request === current.request;
  try {
    if (progressOnly) {
      const progress = await api(chatPath(current.id) + "/progress");
      if (!latest()) return;
      if (JSON.stringify(progress) === JSON.stringify(current.progress)) {
        schedule(current);
        return;
      }
    }
    const data = await api(chatPath(current.id) + "/messages");
    if (!latest()) return;
    const opening = !current.ready;
    current.ready = true;
    current.info = data.conversation;
    current.progress = data.progress;
    remember(data.conversation);
    updateHeader(current);
    renderMessages(data.messages, data.progress.turns, stopTurn, opening);
    const state = current.state;
    if (state.notice === current.readError) notice(state, "");
    current.readError = null;
    const failure = data.progress.last_failure;
    if (failure && failure.id !== state.seenFailure) {
      state.seenFailure = failure.id;
      if (!dismissed(current.id, failure.id)) {
        recover(state, failure.message, failure.error, failure.id);
      }
    }
    showList();
    showComposer();
    schedule(current);
  } catch (error) {
    if (!latest()) return;
    if (error.status === 404) {
      conversations = conversations.filter(item => item.id !== current.id);
      const draft = current.state.draft;
      sessions.delete(current.id);
      const fresh = openView(null);
      if (!fresh.state.draft) fresh.state.draft = draft;
      notice(fresh.state, "该会话已删除");
      return;
    }
    current.readError = "读取会话失败：" + error.message;
    if (!current.state.notice || current.state.notice === current.readError) notice(current.state, current.readError);
    schedule(current, true);
  }
}

async function stopTurn(turnId) {
  const current = view;
  try {
    await api(chatPath(current.id) + "/turns/" + turnId, { method: "DELETE" });
    changes.postMessage(current.id);
  } catch (error) {
    if (error.status !== 404) notice(current.state, error.message);
  }
  await refresh(current);
}

// ******************************** 提交、命名与删除 ********************************

form.addEventListener("submit", async event => {
  event.preventDefault();
  const current = view;
  const state = current.state;
  const text = state.draft.trim();
  if (!text || !current.ready || state.admitting) return;
  state.admitting = true;
  state.draft = "";
  notice(state, "");
  try {
    if (!current.id) {
      // 即使切到别的会话，首条消息仍属于在此创建的会话。
      sessions.delete(null);
      const info = await api("/api/conversations", { method: "POST" });
      current.id = info.id;
      current.info = info;
      sessions.set(info.id, state);
      remember(info, true);
      if (current === view) updateHeader(current);
      showList();
    }
    await api(chatPath(current.id) + "/turns", { method: "POST", body: { message: text } });
    changes.postMessage(current.id);
  } catch (error) {
    if (!current.id && current === view) sessions.set(null, state);
    recover(state, text, error.message);
  } finally {
    state.admitting = false;
    if (view.state === state) showComposer();
    if (current.id === view.id && current.id) refresh(view);
  }
});

input.addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229) {
    event.preventDefault();
    form.requestSubmit();
  }
});

byId("rename-chat").addEventListener("click", async () => {
  menu.open = false;
  const current = view;
  const title = prompt("会话名称", current.info.title)?.trim();
  if (!title || title === current.info.title) return;
  try {
    const info = await api(chatPath(current.id), { method: "PATCH", body: { title } });
    remember(info);
    if (view.id === info.id) {
      view.info = info;
      updateHeader(view);
      refresh(view);
    }
    showList();
    changes.postMessage(info.id);
  } catch (error) { notice(current.state, error.message); }
});

byId("delete-chat").addEventListener("click", async () => {
  menu.open = false;
  const current = view;
  if (!confirm("永久删除「" + current.info.title + "」及其全部消息？")) return;
  try {
    await api(chatPath(current.id), { method: "DELETE" });
    conversations = conversations.filter(item => item.id !== current.id);
    sessions.delete(current.id);
    if (view.id === current.id) openView(null);
    showList();
    changes.postMessage(current.id);
  } catch (error) { notice(current.state, error.message); }
});

document.addEventListener("click", event => {
  if (menu.open && !menu.contains(event.target)) menu.open = false;
});
menu.addEventListener("keydown", event => {
  if (event.key === "Escape") {
    menu.open = false;
    menu.querySelector("summary").focus();
  }
});

// ******************************** 页面加载与跨标签同步 ********************************

async function loadList() {
  const request = ++listRequest;
  const data = await api("/api/conversations");
  if (request !== listRequest) return;
  conversations = data.conversations;
  showList();
}

changes.addEventListener("message", event => {
  loadList().catch(error => notice(view.state, error.message));
  if (event.data === view.id) refresh(view);
});
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && view.id) refresh(view);
});

const initial = openView(null, false);
const requested = new URLSearchParams(location.search).get("conversation");
loadList().then(() => {
  if (view !== initial || initial.id || initial.state.admitting || input.value || initial.explicit) return;
  const info = requested ? conversations.find(item => String(item.id) === requested) : conversations[0];
  if (info) openView(info);
  else if (requested && requested !== "new") {
    updateHeader(initial);
    notice(initial.state, "该会话不存在");
  }
}).catch(error => notice(initial.state, error.message));
