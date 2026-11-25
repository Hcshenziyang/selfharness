// ******************************** 页面元素与会话状态 ********************************

const byId = (id) => document.getElementById(id);

const messages = byId("messages");
const emptyChat = byId("empty-chat");
const chatScroll = byId("chat-scroll");
const chatError = byId("chat-error");
const chatErrorText = byId("chat-error-text");
const dismissErrorBtn = byId("dismiss-error");
const serverError = byId("server-error");
const serverForm = byId("server-form");
const serverFormReveal = byId("server-form-reveal");
const serverFormInner = serverFormReveal ? serverFormReveal.querySelector(".form-reveal-inner") : null;
const toggleFormBtn = byId("toggle-form");
const cancelFormBtn = byId("cancel-form");
const sidebar = document.querySelector(".sidebar");
const sidebarOverlay = byId("sidebar-overlay");
const messageInput = byId("message-input");
const sendMessageBtn = byId("send-message");
const scrollBottomBtn = byId("scroll-bottom");
const conversationList = byId("conversation-list");

let busy = false;
let conversations = [];
let activeConversationId = null;
let activeHistory = [];
const emptyProgress = { turns: [], last_failure: null };
let activeProgress = emptyProgress;
// 每次重绘或切换会话会更新版本号，旧轮询借此识别过期结果。
let viewVersion = 0;
let pollTimer;
let displayedFailure;
// 本标签页仍在等发送结果的会话 ID。activeProgress 来自后端，也能看到其他标签页提交的回合。
const pendingTurns = new Set();
// 按会话保留本页发送失败的原文，包括后端尚未受理的消息。
const failedDrafts = new Map();

// ******************************** 回复文本排版 ********************************
/** 在插入 HTML 前转义模型正文、代码片段和工具名中的特殊字符。 */
function escapeHtml(str) {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/** 单独渲染围栏代码块，其余文字交给 formatInline 处理。 */
function renderFormattedContent(text) {
  const parts = [];
  const codeBlockRegex = /```(\w*)\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match;

  while ((match = codeBlockRegex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(formatInline(text.slice(lastIndex, match.index)));
    }
    const lang = match[1] ? ` data-lang="${escapeHtml(match[1])}"` : "";
    const code = escapeHtml(match[2].replace(/\n$/, ""));
    parts.push(`<pre${lang}><code>${code}</code></pre>`);
    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    parts.push(formatInline(text.slice(lastIndex)));
  }

  return parts.join("");
}

/** 先转义普通文本，再处理行内代码、段落和换行。 */
function formatInline(text) {
  const escaped = escapeHtml(text);
  const withCode = escaped.replace(/`([^`]+)`/g, "<code>$1</code>");
  const paragraphs = withCode.split(/\n\n+/).filter(Boolean);
  if (paragraphs.length <= 1) {
    return `<p>${withCode.replace(/\n/g, "<br>")}</p>`;
  }
  return paragraphs.map(p => `<p>${p.replace(/\n/g, "<br>")}</p>`).join("");
}

// ******************************** 输入框与页面交互 ********************************
/** 有内容、界面可操作且本页没有在等当前会话的发送结果时，启用发送按钮。 */
function updateInputState() {
  const hasText = Boolean(messageInput.value.trim());
  sendMessageBtn.disabled = !hasText || busy || pendingTurns.has(activeConversationId);
}

/** 随草稿调整输入框高度，同时重新判断能否发送。 */
function autoResize() {
  messageInput.style.height = "auto";
  const newHeight = Math.min(messageInput.scrollHeight, 180);
  messageInput.style.height = `${newHeight}px`;
  updateInputState();
}

messageInput.addEventListener("input", autoResize);

/** 判断用户是否靠近底部，供追加消息和显示「回到底部」按钮时共用。 */
function isNearBottom(threshold = 80) {
  return chatScroll.scrollHeight - chatScroll.scrollTop - chatScroll.clientHeight <= threshold;
}

function scrollToBottom(smooth = true) {
  chatScroll.scrollTo({
    top: chatScroll.scrollHeight,
    behavior: smooth ? "smooth" : "auto",
  });
}

chatScroll.addEventListener("scroll", () => {
  if (scrollBottomBtn) {
    const shouldShow = !isNearBottom(120);
    scrollBottomBtn.classList.toggle("visible", shouldShow);
  }
});

if (scrollBottomBtn) {
  scrollBottomBtn.addEventListener("click", () => scrollToBottom(true));
}

// 移动端通过遮罩收起侧栏。
const sidebarToggleBtn = byId("sidebar-toggle");
if (sidebarToggleBtn) {
  sidebarToggleBtn.addEventListener("click", () => {
    sidebar.classList.add("open");
    sidebarOverlay.hidden = false;
  });
}

sidebarOverlay.addEventListener("click", () => {
  sidebar.classList.remove("open");
  sidebarOverlay.hidden = true;
});

// 聊天错误展示在输入框旁，连接错误展示在 MCP 表单里。
function showChatError(message = "") {
  if (!message) {
    chatError.hidden = true;
    chatErrorText.textContent = "";
  } else {
    chatErrorText.textContent = message;
    chatError.hidden = false;
  }
}

if (dismissErrorBtn) {
  dismissErrorBtn.addEventListener("click", () => showChatError(""));
}

function showServerError(message = "") {
  serverError.textContent = message;
  serverError.hidden = !message;
}

// ******************************** 后端请求与控件状态 ********************************
/** 向本地后端收发 JSON，把 HTTP 错误转成页面可展示的错误消息。 */
async function api(path, { method = "GET", body } = {}) {
  const options = { method };
  if (body !== undefined) {
    options.headers = { "Content-Type": "application/json" };
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  const data = await response.json().catch(() => ({ error: "服务响应格式错误" }));
  if (!response.ok) throw new Error(data.error || `请求失败（${response.status}）`);
  return data;
}

/** 新建、切换会话或管理 MCP 服务时，临时锁住相应控件。 */
function setBusy(value) {
  busy = value;
  for (const id of ["new-chat", "toggle-form", "connect-server", "cancel-form"]) {
    const el = byId(id);
    if (el) el.disabled = value;
  }
  updateInputState();
  document.querySelectorAll(".server-remove, .conversation-link").forEach((button) => {
    button.disabled = value;
  });
}

// ******************************** 聊天气泡与复制 ********************************
/** 复制回复原文，成功后短暂显示「已复制」。 */
async function copyToClipboard(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    const originalHtml = button.innerHTML;
    button.innerHTML = `
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
        <path d="M3 8.5l3.5 3.5 6.5-7" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <span>已复制</span>
    `;
    button.style.color = "var(--success)";
    setTimeout(() => {
      button.innerHTML = originalHtml;
      button.style.color = "";
    }, 1800);
  } catch {
    // 浏览器拒绝写入剪贴板时，保留原按钮状态。
  }
}

/** 添加消息气泡。排队中的回合显示文字，执行中的回合显示等待动画。 */
function addMessage(role, content, pending = false) {
  const wasBottom = isNearBottom(100);
  const row = document.createElement("div");
  row.className = `message ${role}${pending ? " pending" : ""}`;

  const avatar = document.createElement("div");
  avatar.className = "avatar";
  if (role === "user") {
    avatar.textContent = "我";
  } else {
    avatar.innerHTML = `
      <svg width="16" height="16" viewBox="0 0 28 28" fill="none">
        <path d="M8 10a4 4 0 014-4h4a4 4 0 014 4v4a6 6 0 01-6 6h0a6 6 0 01-6-6v-4z" fill="currentColor"/>
        <circle cx="12" cy="12" r="1.5" fill="#342c24"/>
        <circle cx="16" cy="12" r="1.5" fill="#342c24"/>
      </svg>
    `;
  }

  const bubbleWrap = document.createElement("div");
  bubbleWrap.className = "bubble-wrap";

  const bubble = document.createElement("div");
  bubble.className = "bubble";

  if (pending === "queued") {
    bubble.textContent = "排队中";
  } else if (pending) {
    bubble.innerHTML = `
      <div class="typing-dots" aria-label="正在思考">
        <span class="typing-dot"></span>
        <span class="typing-dot"></span>
        <span class="typing-dot"></span>
      </div>
    `;
  } else if (role === "assistant") {
    bubble.innerHTML = renderFormattedContent(content);
  } else {
    bubble.textContent = content;
  }

  bubbleWrap.append(bubble);

  // 只为已经有正文的助手回复提供复制按钮。
  if (!pending && role === "assistant") {
    const actions = document.createElement("div");
    actions.className = "message-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "btn-bubble-action";
    copyBtn.innerHTML = `
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
        <rect x="5" y="5" width="8" height="8" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
        <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.3"/>
      </svg>
      <span>复制</span>
    `;
    copyBtn.addEventListener("click", () => copyToClipboard(content, copyBtn));
    actions.append(copyBtn);
    bubbleWrap.append(actions);
  }

  row.append(avatar, bubbleWrap);
  messages.append(row);
  emptyChat.hidden = true;

  if (wasBottom || role === "user") {
    scrollToBottom(true);
  }

  return row;
}

function clearMessages() {
  messages.replaceChildren();
  emptyChat.hidden = false;
  if (scrollBottomBtn) scrollBottomBtn.classList.remove("visible");
}

// ******************************** 会话列表与切换 ********************************

/** 将新建或刚完成回合的会话放到侧栏最前面。 */
function rememberConversation(info) {
  conversations = [info, ...conversations.filter(item => item.id !== info.id)];
}

/** 重画侧栏，用本页的待处理请求或当前会话的后端进度标出「回复中」。 */
function showConversationList() {
  byId("conversation-history").hidden = conversations.length === 0;
  conversationList.replaceChildren();
  for (const item of conversations) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "sidebar-action conversation-link";
    const running = pendingTurns.has(item.id) || (item.id === activeConversationId && activeProgress.turns.length);
    button.textContent = running ? `${item.title} · 回复中` : item.title;
    button.title = item.title;
    button.dataset.id = item.id;
    button.disabled = busy;
    if (item.id === activeConversationId) button.setAttribute("aria-current", "true");
    conversationList.append(button);
  }
}

// ******************************** 进度轮询与历史恢复 ********************************

/** 停止旧轮询，返回新版本号供异步回调核对。 */
function invalidateView() {
  clearTimeout(pollTimer);
  return ++viewVersion;
}

/** 仍有待处理回合时每秒查一次进度，进度变化时才重新加载完整历史。 */
function pollProgress(id, version) {
  if (version !== viewVersion || (!activeProgress.turns.length && !pendingTurns.has(id))) return;
  pollTimer = setTimeout(async () => {
    try {
      const progress = await api(`/api/conversations/${id}/progress`);
      if (version !== viewVersion) return;
      if (JSON.stringify(progress) !== JSON.stringify(activeProgress)) {
        await loadConversation(id, version);
        return;
      }
    } catch (error) {
      if (version === viewVersion) showChatError(error.message);
    }
    pollProgress(id, version);
  }, 1000);
}

/** 先画已保存的对话，再补上执行和排队中的回合，以及最近失败的原文。 */
function showConversation(info, historyMessages, progress = emptyProgress) {
  const version = invalidateView();
  if (activeConversationId !== info.id) {
    messageInput.value = "";
    displayedFailure = undefined;
  }
  activeConversationId = info.id;
  activeHistory = historyMessages;
  activeProgress = progress;
  const listed = conversations.find(item => item.id === info.id);
  if (listed) listed.title = info.title;
  const url = new URL(location.href);
  url.searchParams.set("conversation", info.id);
  window.history.replaceState(null, "", url);
  byId("conversation-title").textContent = info.title;
  clearMessages();
  historyMessages.forEach(message => addMessage(message.role, message.content));
  for (const turn of progress.turns) {
    addMessage("user", turn.message);
    addMessage("assistant", "", turn.status);
  }
  const recent = progress.last_failure;
  const local = failedDrafts.get(info.id);
  if (local && recent?.message === local.message && recent.error === local.error) {
    failedDrafts.delete(info.id);
  }
  // 本页已有新草稿时，在错误提示中展示失败原文，避免刷新覆盖正在编辑的文字。
  const failed = failedDrafts.get(info.id) || recent;
  const failureKey = failed?.id ?? failed;
  if (failureKey !== displayedFailure) {
    displayedFailure = failureKey;
    if (failed && messageInput.value && messageInput.value !== failed.message) {
      showChatError(`${failed.error}（原消息：${failed.message}）`);
    } else {
      showChatError(failed?.error);
      if (failed) messageInput.value = failed.message;
    }
  } else if (!failed) {
    showChatError("");
  }
  autoResize();
  scrollToBottom();
  showConversationList();
  pollProgress(info.id, version);
}

/** 一次读取历史和进度。轮询传入版本号，避免旧请求覆盖现在的会话。 */
async function loadConversation(id, version) {
  const data = await api(`/api/conversations/${id}/messages`);
  if (version !== undefined && version !== viewVersion) return;
  showConversation(data.conversation, data.messages, data.progress);
}

/** 创建并打开空会话，拿到的 ID 用于后续发送消息。 */
async function newConversation() {
  const info = await api("/api/conversations", { method: "POST" });
  rememberConversation(info);
  showConversation(info, []);
}

conversationList.addEventListener("click", async (event) => {
  const button = event.target.closest(".conversation-link");
  if (!button || busy) return;
  const info = conversations.find(item => String(item.id) === button.dataset.id);
  if (!info || info.id === activeConversationId) return;
  setBusy(true);
  showChatError("");
  try {
    await loadConversation(info.id);
    sidebar.classList.remove("open");
    sidebarOverlay.hidden = true;
  } catch (error) {
    showChatError(error.message);
  } finally {
    setBusy(false);
  }
});

// ******************************** MCP 服务列表 ********************************
/** 根据后端返回的连接状态、工具清单重画服务卡片，并绑定断开操作。 */
function showServers(servers) {
  const list = byId("servers");
  list.replaceChildren();

  if (!servers.length) {
    const empty = document.createElement("div");
    empty.className = "server-empty";
    empty.textContent = "还没有连接 MCP 服务";
    list.append(empty);
    return;
  }

  for (const server of servers) {
    const card = document.createElement("div");
    card.className = "server-card";

    const row = document.createElement("div");
    row.className = "server-row";

    const details = document.createElement("div");
    const title = document.createElement("div");
    title.className = "server-title";
    title.textContent = server.name;

    const metaLine = document.createElement("div");
    metaLine.className = "server-meta-line";

    const dot = document.createElement("span");
    dot.className = `server-status-dot ${server.connected ? "connected" : "disconnected"}`;
    dot.title = server.connected ? "已连接" : "未连接";

    const typeBadge = document.createElement("span");
    typeBadge.className = "server-badge";
    typeBadge.textContent = server.type === "stdio" ? "stdio" : "HTTP";

    const toolCountBadge = document.createElement("span");
    toolCountBadge.className = "server-badge";
    toolCountBadge.textContent = `${server.tools.length} 个工具`;

    metaLine.append(dot, typeBadge, toolCountBadge);
    details.append(title, metaLine);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "server-remove";
    remove.textContent = "断开";
    remove.disabled = busy;
    remove.addEventListener("click", async () => {
      if (busy) return;
      showServerError("");
      setBusy(true);
      try {
        const result = await api(`/api/mcp/servers/${encodeURIComponent(server.name)}`, { method: "DELETE" });
        showServers(result.servers);
      } catch (error) {
        showServerError(error.message);
        toggleForm(true);
      } finally {
        setBusy(false);
      }
    });

    row.append(details, remove);
    card.append(row);

    // 按需展开服务公布的工具名。
    if (server.tools.length > 0) {
      const toggleToolsBtn = document.createElement("button");
      toggleToolsBtn.type = "button";
      toggleToolsBtn.className = "server-tools-toggle";
      toggleToolsBtn.innerHTML = `
        <svg width="10" height="10" viewBox="0 0 16 16" fill="none">
          <path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        <span>查看工具列表</span>
      `;

      const toolsList = document.createElement("ul");
      toolsList.className = "server-tools-list";

      for (const name of server.tools) {
        const item = document.createElement("li");
        item.innerHTML = `
          <svg width="10" height="10" viewBox="0 0 16 16" fill="none">
            <circle cx="8" cy="8" r="3" fill="currentColor"/>
          </svg>
          <span>${escapeHtml(name)}</span>
        `;
        toolsList.append(item);
      }

      toggleToolsBtn.addEventListener("click", () => {
        const isExpanded = toolsList.classList.toggle("expanded");
        toggleToolsBtn.classList.toggle("expanded", isExpanded);
        toggleToolsBtn.querySelector("span").textContent = isExpanded ? "收起工具列表" : "查看工具列表";
      });

      card.append(toggleToolsBtn, toolsList);
    }

    list.append(card);
  }
}

// ******************************** MCP 连接表单 ********************************
/** 按传输方式启用对应字段，并切换必填项。 */
function updateFields() {
  const http = byId("server-type").value === "http";
  for (const [id, active] of [["stdio-fields", !http], ["http-fields", http]]) {
    const section = byId(id);
    section.hidden = !active;
    section.querySelectorAll("input, textarea").forEach((field) => {
      field.disabled = !active;
    });
  }
  byId("server-command").required = !http;
  byId("server-url").required = http;
}

/** 展开时聚焦服务名称，收起时重置字段并同步键盘可访问状态。 */
function toggleForm(show) {
  if (serverFormReveal) {
    serverFormReveal.classList.toggle("open", show);
    if (serverFormInner) {
      if (show) {
        serverFormInner.removeAttribute("inert");
      } else {
        serverFormInner.setAttribute("inert", "");
      }
    }
  } else {
    serverForm.hidden = !show;
  }

  toggleFormBtn.setAttribute("aria-expanded", String(show));
  showServerError("");

  if (show) {
    byId("server-name").focus();
  } else {
    serverForm.reset();
    updateFields();
  }
}

/** 逐行解析环境变量或请求头，只在第一个分隔符处分割，并检查重复键。 */
function parsePairs(value, separator, description) {
  const result = Object.create(null);
  for (const line of value.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const index = line.indexOf(separator);
    const key = line.slice(0, index).trim();
    if (index < 1 || !key) throw new Error(`${description}格式不正确：${line}`);
    if (Object.hasOwn(result, key)) throw new Error(`${description}名称重复：${key}`);
    result[key] = line.slice(index + 1).trim();
  }
  return result;
}

// 表单打开、取消和切换传输方式的入口。
toggleFormBtn.addEventListener("click", () => {
  const isOpen = serverFormReveal ? serverFormReveal.classList.contains("open") : !serverForm.hidden;
  toggleForm(!isOpen);
});

cancelFormBtn.addEventListener("click", () => toggleForm(false));
byId("server-type").addEventListener("change", updateFields);

serverForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (busy) return;
  showServerError("");
  try {
    const name = byId("server-name").value.trim();
    const type = byId("server-type").value;
    let body;
    if (type === "stdio") {
      body = {
        name,
        type,
        command: byId("server-command").value.trim(),
        args: byId("server-args").value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
        env: parsePairs(byId("server-env").value, "=", "环境变量"),
      };
      const cwd = byId("server-cwd").value.trim();
      if (cwd) body.cwd = cwd;
    } else {
      const headers = parsePairs(byId("server-headers").value, ":", "请求头");
      const token = byId("server-token").value.trim();
      if (token && Object.keys(headers).some((key) => key.toLowerCase() === "authorization")) {
        throw new Error("请只在一处填写 Authorization");
      }
      if (token) headers.Authorization = `Bearer ${token}`;
      body = { name, type, url: byId("server-url").value.trim(), headers };
    }
    body.supports_parallel_tool_calls = byId("server-parallel").checked;

    setBusy(true);
    const submitBtn = byId("connect-server");
    submitBtn.textContent = "正在连接…";
    const result = await api("/api/mcp/servers", { method: "POST", body });
    showServers(result.servers);
    toggleForm(false);
  } catch (error) {
    showServerError(error.message);
  } finally {
    byId("connect-server").textContent = "连接";
    setBusy(false);
  }
});

// ******************************** 发送消息与新建对话 ********************************

// 立即显示本条输入和等待状态，后端返回后用会话历史和进度更新页面。
byId("chat-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = messageInput.value.trim();
  if (!text || busy || pendingTurns.has(activeConversationId)) return;

  if (activeConversationId === null) {
    setBusy(true);
    try {
      await newConversation();
    } catch (error) {
      showChatError(error.message);
      return;
    } finally {
      setBusy(false);
    }
  }
  const id = activeConversationId;
  pendingTurns.add(id);
  failedDrafts.delete(id);
  messageInput.value = "";
  showChatError("");
  addMessage("user", text);
  addMessage("assistant", "", activeProgress.turns.length ? "queued" : "running");
  autoResize();
  showConversationList();
  pollProgress(id, invalidateView());
  let completed;
  try {
    completed = await api(`/api/conversations/${id}/messages`, {
      method: "POST", body: { message: text },
    });
    rememberConversation(completed.conversation);
  } catch (error) {
    failedDrafts.set(id, { message: text, error: error.message });
  } finally {
    pendingTurns.delete(id);
    showConversationList();
    if (activeConversationId === id) {
      const version = invalidateView();
      try {
        // 其他页面可能已提交下一轮，使用服务器最新的历史与队列。
        await loadConversation(id, version);
      } catch (error) {
        if (version === viewVersion) {
          if (completed) {
            showConversation(completed.conversation, completed.messages, completed.progress);
          } else {
            showConversation(conversations.find(item => item.id === id), activeHistory, activeProgress);
          }
        }
      }
      if (activeConversationId === id) messageInput.focus();
    }
  }
});

// Enter 发送，Shift+Enter 换行，输入法选词期间的 Enter 交给输入法。
messageInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229) {
    event.preventDefault();
    if (messageInput.value.trim()) {
      byId("chat-form").requestSubmit();
    }
  }
});

byId("new-chat").addEventListener("click", async () => {
  if (busy) return;
  setBusy(true);
  showChatError("");
  try {
    await newConversation();
  } catch (error) {
    showChatError(error.message);
  } finally {
    setBusy(false);
  }
});

// ******************************** 首次加载与刷新 ********************************

// 会话列表和 MCP 服务列表可以分别加载，优先恢复 URL 指定的会话。
setBusy(true);

Promise.allSettled([
  api("/api/conversations")
    .then(async (data) => {
      conversations = data.conversations;
      showConversationList();
      const id = new URLSearchParams(location.search).get("conversation");
      const info = conversations.find(item => String(item.id) === id) || conversations[0];
      if (info) await loadConversation(info.id);
    })
    .catch((error) => showChatError(error.message)),
  api("/api/mcp/servers")
    .then((data) => showServers(data.servers))
    .catch((error) => {
      toggleForm(true);
      showServerError(error.message);
    }),
]).finally(() => setBusy(false));
