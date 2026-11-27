import { api, byId, changes, element, setSidebar } from "./ui.js";
import { setupAgentEditor, showAgents } from "./agents.js";
import { renderMessages } from "./messages.js";
import { setToolAgent } from "./mcp.js";
import { setupToolSettings } from "./tool-settings.js";

// ******************************** 会话导航与草稿 ********************************

const input = byId("message-input");
const form = byId("chat-form");
const list = byId("conversation-list");
const menu = byId("conversation-menu");
const sessions = new Map();

let agents = [];
let activeAgentId = null;
let agentRequest = 0;
let conversations = [];
let view;
let listKey;
let listRequest = 0;

function session(id) {
  if (!sessions.has(id)) sessions.set(id, { draft: "", notice: "", admitting: false, seenFailure: null });
  return sessions.get(id);
}

function remember(info, first = false) {
  if (info.agent_id !== activeAgentId) return;
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
  if (current.agentId) url.searchParams.set("agent", current.agentId);
  history.replaceState(null, "", url);
}

function showComposer() {
  const state = view.state;
  input.disabled = !view.agentId;
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
  const agentId = info?.agent_id ?? activeAgentId;
  const current = { id, info, agentId, state: session(id ?? "new:" + agentId),
    ready: !id && Boolean(agentId), request: 0, progress: null };
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
  if (!activeAgentId) return;
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
    current.agentId = data.conversation.agent_id;
    if (current.agentId !== activeAgentId) {
      selectAgent(current.agentId);
      reloadList();
    }
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
      if (!activeAgentId) {
        selectAgent(current.fallbackAgentId);
        reloadList();
      }
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
      sessions.delete("new:" + current.agentId);
      const info = await api("/api/conversations", { method: "POST", body: { agent_id: current.agentId } });
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
    const draftKey = "new:" + current.agentId;
    if (!current.id && !sessions.has(draftKey)) sessions.set(draftKey, state);
    recover(current.id ? state : sessions.get(draftKey), text, error.message);
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
  if (!activeAgentId) return;
  const request = ++listRequest;
  const data = await api("/api/conversations?agent_id=" + activeAgentId);
  if (request !== listRequest) return;
  conversations = data.conversations;
  showList();
}

function reloadList() {
  const id = activeAgentId;
  loadList().catch(error => {
    if (id === activeAgentId) notice(view.state, error.message);
  });
}

function selectAgent(id) {
  if (id !== activeAgentId) {
    ++listRequest;
    conversations = [];
    activeAgentId = id;
    setToolAgent(id);
  }
  showAgents(agents, id);
}

function switchAgent(id) {
  selectAgent(id);
  openView(null);
  reloadList();
}

async function loadAgents() {
  const request = ++agentRequest;
  const data = await api("/api/agents");
  if (request === agentRequest) {
    agents = data.agents;
    showAgents(agents, activeAgentId);
  }
  return data.default_agent_id;
}

setupToolSettings(() => activeAgentId);
byId("agent-select").addEventListener("change", event => switchAgent(Number(event.target.value)));
setupAgentEditor((agent, created) => {
  ++agentRequest;
  const summary = { id: agent.id, name: agent.name, model: agent.model };
  const at = agents.findIndex(item => item.id === agent.id);
  if (at < 0) agents.push(summary);
  else agents[at] = summary;
  if (created) switchAgent(agent.id);
  else showAgents(agents, activeAgentId);
  changes.postMessage({ agentsChanged: true, agent_id: agent.id, revision: agent.revision });
});

changes.addEventListener("message", event => {
  if (event.data?.mcpChanged || event.data?.toolPolicyChanged) return;
  if (event.data?.agentsChanged) {
    loadAgents().catch(error => notice(view.state, error.message));
    return;
  }
  reloadList();
  if (event.data === view.id) refresh(view);
});
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && view.id) refresh(view);
});

async function initialize() {
  const params = new URLSearchParams(location.search);
  const defaultId = await loadAgents();
  const requestedAgent = Number(params.get("agent"));
  const fallbackId = agents.some(agent => agent.id === requestedAgent) ? requestedAgent : defaultId;
  const requested = params.get("conversation");
  if (requested && requested !== "new" && /^[1-9]\d*$/.test(requested) && Number.isSafeInteger(Number(requested))) {
    // 直链只需会话 ID，历史接口返回的归属优先于 URL 中的助手。
    const current = openView({ id: Number(requested) }, false);
    current.fallbackAgentId = fallbackId;
    return;
  }
  selectAgent(fallbackId);
  const initial = openView(null);
  await loadList();
  if (view !== initial || initial.id || initial.state.admitting || input.value || initial.explicit) return;
  if (!requested && conversations.length) openView(conversations[0]);
  else if (requested && requested !== "new") notice(initial.state, "该会话不存在");
}

openView(null, false);
initialize().catch(error => notice(view.state, error.message));
