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
const pendingTurns = new Map();
const failedDrafts = new Map();

// ─── Utilities ───
function escapeHtml(str) {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Lightweight formatting for assistant responses (code blocks, inline code, paragraphs)
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

function formatInline(text) {
  const escaped = escapeHtml(text);
  // Inline code: `code`
  const withCode = escaped.replace(/`([^`]+)`/g, "<code>$1</code>");
  // Paragraphs
  const paragraphs = withCode.split(/\n\n+/).filter(Boolean);
  if (paragraphs.length <= 1) {
    return `<p>${withCode.replace(/\n/g, "<br>")}</p>`;
  }
  return paragraphs.map(p => `<p>${p.replace(/\n/g, "<br>")}</p>`).join("");
}

// ─── Auto-resize & Input Validation ───
function updateInputState() {
  const hasText = Boolean(messageInput.value.trim());
  sendMessageBtn.disabled = !hasText || busy || pendingTurns.has(activeConversationId);
}

function autoResize() {
  messageInput.style.height = "auto";
  const newHeight = Math.min(messageInput.scrollHeight, 180);
  messageInput.style.height = `${newHeight}px`;
  updateInputState();
}

messageInput.addEventListener("input", autoResize);

// ─── Scroll Handling & FAB ───
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

// ─── Mobile Sidebar Toggle ───
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

// ─── Error Notification ───
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

// ─── API Helper ───
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

// ─── Copy to Clipboard ───
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
    // Fallback if clipboard API fails
  }
}

// ─── Chat Messages ───
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

  if (pending) {
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

  // Add copy action button on hover for assistant messages
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

function rememberConversation(info) {
  conversations = [info, ...conversations.filter(item => item.id !== info.id)];
}

function showConversationList() {
  byId("conversation-history").hidden = conversations.length === 0;
  conversationList.replaceChildren();
  for (const item of conversations) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "sidebar-action conversation-link";
    button.textContent = pendingTurns.has(item.id) ? `${item.title} · 回复中` : item.title;
    button.title = item.title;
    button.dataset.id = item.id;
    button.disabled = busy;
    if (item.id === activeConversationId) button.setAttribute("aria-current", "true");
    conversationList.append(button);
  }
}

function showConversation(info, historyMessages) {
  if (activeConversationId !== info.id) messageInput.value = "";
  activeConversationId = info.id;
  activeHistory = historyMessages;
  const url = new URL(location.href);
  url.searchParams.set("conversation", info.id);
  window.history.replaceState(null, "", url);
  byId("conversation-title").textContent = info.title;
  clearMessages();
  historyMessages.forEach(message => addMessage(message.role, message.content));
  const pending = pendingTurns.get(info.id);
  if (pending) {
    addMessage("user", pending.text);
    addMessage("assistant", "", true);
  }
  const failed = failedDrafts.get(info.id);
  showChatError(failed?.error);
  if (failed) {
    messageInput.value = failed.text;
    failedDrafts.delete(info.id);
  }
  autoResize();
  scrollToBottom();
  showConversationList();
}

async function openConversation(info) {
  const pending = pendingTurns.get(info.id);
  if (pending) {
    showConversation(info, pending.history);
    return;
  }
  const data = await api(`/api/conversations/${info.id}/messages`);
  showConversation(info, data.messages);
}

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
    await openConversation(info);
    sidebar.classList.remove("open");
    sidebarOverlay.hidden = true;
  } catch (error) {
    showChatError(error.message);
  } finally {
    setBusy(false);
  }
});

// ─── Servers List ───
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

    // Collapsible Tools
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

// ─── Form Logic ───
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

// ─── Event Listeners ───
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

// Chat Submit
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
  const history = activeHistory;
  pendingTurns.set(id, { text, history });
  messageInput.value = "";
  showChatError("");
  addMessage("user", text);
  addMessage("assistant", "", true);
  autoResize();
  showConversationList();
  try {
    const result = await api(`/api/conversations/${id}/messages`, {
      method: "POST", body: { message: text },
    });
    pendingTurns.delete(id);
    rememberConversation(result.conversation);
    if (activeConversationId === id) showConversation(result.conversation, result.messages);
  } catch (error) {
    pendingTurns.delete(id);
    failedDrafts.set(id, { text, error: error.message });
    if (activeConversationId === id) {
      showConversation(conversations.find(item => item.id === id), history);
    }
  } finally {
    showConversationList();
    if (activeConversationId === id) messageInput.focus();
  }
});

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

// ─── Initialize ───
setBusy(true);

Promise.allSettled([
  api("/api/conversations")
    .then(async (data) => {
      conversations = data.conversations;
      showConversationList();
      const id = new URLSearchParams(location.search).get("conversation");
      const info = conversations.find(item => String(item.id) === id) || conversations[0];
      if (info) await openConversation(info);
    })
    .catch((error) => showChatError(error.message)),
  api("/api/mcp/servers")
    .then((data) => showServers(data.servers))
    .catch((error) => {
      toggleForm(true);
      showServerError(error.message);
    }),
]).finally(() => setBusy(false));
