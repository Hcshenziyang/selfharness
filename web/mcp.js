import { byId, changes, element, api } from "./ui.js";

const list = byId("servers");
const dialog = byId("server-dialog");
const form = byId("server-form");
const submit = byId("connect-server");
const add = byId("toggle-form");
const retry = byId("retry-servers");
let active = null;
let editing = null;

function feedback(id, message = "") {
  byId(id).textContent = message;
  byId(id).hidden = !message;
}
function listError(message = "") {
  feedback("server-list-error", message);
  retry.hidden = !message;
}
function controls() {
  const disabled = !active || active.busy;
  submit.disabled = add.disabled = retry.disabled = disabled;
  list.querySelectorAll("input, button").forEach(field => {
    field.disabled = disabled || field.hasAttribute("data-no-changes");
  });
}

export function setToolAgent(id) {
  if (active?.id === id) return;
  if (dialog.open) dialog.close();
  if (byId("tool-dialog").open) byId("tool-dialog").close();
  active = id ? { id, base: "/api/agents/" + id + "/mcp/servers", version: 0, busy: false,
    servers: null, entered: new Map() } : null;
  byId("edit-tools").disabled = !id;
  list.replaceChildren();
  listError();
  controls();
  if (active) loadServers();
}

function action(label, handler) {
  const button = element("button", "server-action", label);
  button.type = "button";
  button.addEventListener("click", handler);
  return button;
}

function toolChoices(server, open) {
  const detail = element("details", "server-tools");
  detail.dataset.id = server.id;
  detail.open = open;
  const enabled = new Set(server.enabled_tools);
  const names = [...server.tools, ...server.unavailable_tools];
  detail.append(element("summary", "server-tools-toggle", "工具授权 " + enabled.size + " / " + server.tools.length));
  if (!enabled.size && server.connected && server.tools.length) detail.append(element("p", "tool-hint", "勾选后，助手即可调用工具。"));
  const bulk = element("div", "server-actions");
  const toEnable = server.tools.filter(name => !enabled.has(name));
  const toDisable = [...server.enabled_tools, ...server.unavailable_tools];
  for (const [label, targets, value] of [["全选", toEnable, true], ["全不选", toDisable, false]]) {
    const button = action(label, () => setGrants(server, Object.fromEntries(targets.map(name => [name, value]))));
    button.toggleAttribute("data-no-changes", targets.length === 0);
    bulk.append(button);
  }
  detail.append(bulk);
  for (const name of names) {
    const label = element("label", "tool-choice");
    const input = element("input");
    input.type = "checkbox";
    const missing = server.unavailable_tools.includes(name);
    input.checked = missing || enabled.has(name);
    input.addEventListener("change", () => setGrants(server, { [name]: input.checked }));
    label.title = name;
    const shortName = name.slice(("mcp__" + server.name + "__").length);
    label.append(input, document.createTextNode(shortName + (missing ? "（暂未提供）" : "")));
    detail.append(label);
  }
  return detail;
}

function showServers(servers, expandName) {
  const expanded = new Set([...list.querySelectorAll("details[open]")].map(d => Number(d.dataset.id)));
  const oldServers = active.servers;
  active.servers = servers;
  list.replaceChildren();
  if (!servers.length) list.append(element("div", "server-empty", "此助手还没有绑定 MCP 服务"));
  for (const server of servers) {
    const card = element("div", "server-card");
    card.dataset.name = server.name;
    const meta = element("div", "server-meta-line");
    const status = server.connected ? "已连接" : "待连接";
    const dot = element("span", "server-status-dot " + (server.connected ? "connected" : "disconnected"));
    dot.title = status;
    meta.append(dot, element("span", "server-badge", status), element("span", "server-badge", server.type));
    card.append(element("div", "server-title", server.name), meta);
    if (server.tools.length || server.unavailable_tools.length) {
      const old = oldServers?.find(s => s.id === server.id);
      const newTools = oldServers !== null && (!old || server.tools.some(name => !old.tools.includes(name)));
      card.append(toolChoices(server, expanded.has(server.id) || newTools || server.name === expandName));
    } else card.append(element("p", "tool-hint", "此服务尚未公布工具"));
    const actions = element("div", "server-actions");
    const edit = action(server.connected ? "编辑" : "配置并连接", event => openForm(server, event.currentTarget));
    edit.setAttribute("aria-haspopup", "dialog");
    edit.setAttribute("aria-controls", "server-dialog");
    actions.append(edit);
    if (server.can_reconnect) actions.append(action("重连", () => mutate(server.id + "/reconnect", "POST")));
    if (server.connected) actions.append(action("断开", () => mutate(server.id + "/disconnect", "POST")));
    actions.append(action("移除", () => {
      if (confirm("移除「" + server.name + "」及其工具授权？")) mutate(String(server.id), "DELETE");
    }));
    card.append(actions);
    list.append(card);
    if (!server.can_reconnect) active.entered.delete(server.id);
  }
  const ids = new Set(servers.map(s => s.id));
  for (const id of active.entered.keys()) if (!ids.has(id)) active.entered.delete(id);
  controls();
}

async function loadServers(clearError = true) {
  const owner = active;
  if (!owner || owner.busy) return;
  const version = ++owner.version;
  try {
    const data = await api(owner.base);
    if (active !== owner || owner.version !== version) return;
    if (JSON.stringify(data.servers) !== JSON.stringify(owner.servers)) showServers(data.servers);
    if (clearError) listError();
  } catch (error) {
    if (owner === active && owner.version === version) listError(error.message);
  }
}

/** 每次变更绑定到发起操作时的助手。跨标签同时修改时，重新读取最终状态。 */
async function mutate(suffix, method, body, onSaved, session) {
  const owner = active;
  if (!owner || owner.busy) return;
  ++owner.version;
  owner.busy = true;
  controls();
  listError();
  let failed = false;
  try {
    const data = await api(owner.base + (suffix ? "/" + suffix : ""), { method, body });
    const resetConfig = method === "PUT" || method === "DELETE" || String(suffix).endsWith("/disconnect");
    changes.postMessage({ mcpChanged: owner.id, resetBinding: resetConfig ? Number(String(suffix).split("/")[0]) : null });
    if (owner !== active) return;
    showServers(data.servers, session?.server ? undefined : session?.name);
    onSaved?.(data.servers);
  } catch (error) {
    failed = true;
    if (owner !== active) return;
    if (owner.servers) showServers(owner.servers);
    if (session && editing === session && dialog.open) feedback("server-error", error.message);
    else listError(error.message);
  } finally {
    owner.busy = false;
    if (owner === active) {
      controls();
      await loadServers(!failed);
    } else if (active?.id === owner.id) await loadServers();
  }
}

function setGrants(server, tools) {
  return mutate(server.id + "/tools", "PATCH", { revision: server.revision, tools });
}

retry.addEventListener("click", () => loadServers());
changes.addEventListener("message", event => {
  if (active && event.data?.mcpChanged === active.id) {
    if (event.data.resetBinding) active.entered.delete(event.data.resetBinding);
    loadServers();
  }
});
document.addEventListener("visibilitychange", () => { if (!document.hidden) loadServers(); });
window.addEventListener("focus", () => loadServers());

const field = name => byId("server-" + name);
const fields = ["name", "type", "command", "args", "cwd", "env", "url", "token", "headers"];

function updateFields() {
  const http = field("type").value === "http";
  for (const [id, active] of [["stdio-fields", !http], ["http-fields", http]]) {
    const section = byId(id);
    section.hidden = !active;
    section.querySelectorAll("input, textarea").forEach(input => { input.disabled = !active; });
  }
  field("command").required = !http;
  field("url").required = http;
}


function openForm(server, button) {
  editing = { server, opener: button };
  form.reset();
  const saved = server?.can_reconnect && active.entered.get(server.id);
  if (saved) {
    for (const name of fields) field(name).value = saved[name];
    field("parallel").checked = saved.parallel;
  } else if (server) {
    field("name").value = server.name;
    field("type").value = server.type;
  }
  field("name").readOnly = Boolean(server);
  byId("edit-hint").hidden = !server || Boolean(saved);
  byId("server-form-title").textContent = server?.connected ? "编辑 MCP 服务" : "连接 MCP 服务";
  submit.textContent = server?.connected ? "保存并重连" : "连接";
  feedback("server-error");
  updateFields();
  add.setAttribute("aria-expanded", String(!server));
  dialog.showModal();
  (server ? field("type") : field("name")).focus();
}

add.addEventListener("click", () => openForm(null, add));
byId("cancel-form").addEventListener("click", () => dialog.close());
dialog.addEventListener("close", () => {
  if (dialog.open) return;
  const old = editing;
  editing = null;
  form.reset();
  add.setAttribute("aria-expanded", "false");
  const card = [...list.children].find(node => node.dataset.name === old?.server?.name);
  const trigger = old?.opener?.isConnected ? old.opener : card?.querySelector("[aria-haspopup=dialog]") || add;
  if (byId("sidebar").inert) byId("sidebar-toggle").focus();
  else if (!trigger.disabled) trigger.focus();
});
field("type").addEventListener("change", updateFields);

/** 环境变量原样保留等号后的值；HTTP 头忽略键名大小写和冒号后的空白。 */
function parsePairs(value, headers = false) {
  const result = Object.create(null);
  const seen = new Set();
  const description = headers ? "请求头" : "环境变量";
  for (const line of value.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const index = line.indexOf(headers ? ":" : "=");
    const key = line.slice(0, index).trim();
    if (index < 1 || !key) throw new Error(description + "格式不正确");
    const normalized = headers ? key.toLowerCase() : key;
    if (seen.has(normalized)) throw new Error(description + "名称重复：" + key);
    seen.add(normalized);
    const raw = line.slice(index + 1);
    result[key] = headers ? raw.trim() : raw;
  }
  return result;
}

function readConfig(values) {
  const body = { type: values.type, supports_parallel_tool_calls: values.parallel };
  if (values.type === "stdio") {
    body.command = values.command.trim();
    body.args = values.args.split(/\r?\n/).filter(line => line.length);
    body.env = parsePairs(values.env);
    if (values.cwd.trim()) body.cwd = values.cwd.trim();
  } else {
    body.url = values.url.trim();
    body.headers = parsePairs(values.headers, true);
    const token = values.token.trim();
    if (token && Object.keys(body.headers).some(key => key.toLowerCase() === "authorization")) {
      throw new Error("请只在一处填写 Authorization");
    }
    if (token) body.headers.Authorization = "Bearer " + token;
  }
  return body;
}


form.addEventListener("submit", async event => {
  event.preventDefault();
  const session = editing;
  if (!session || !active || active.busy) return;
  const values = Object.fromEntries(fields.map(name => [name, field(name).value]));
  values.parallel = field("parallel").checked;
  session.name = values.name.trim();
  feedback("server-error");
  try {
    const body = readConfig(values);
    if (!session.server) body.name = session.name;
    submit.textContent = "正在连接…";
    await mutate(session.server?.id, session.server ? "PUT" : "POST", body, servers => {
      const connected = servers.find(server => server.name === session.name);
      if (connected) active.entered.set(connected.id, values);
      if (dialog.open && editing === session) dialog.close();
    }, session);
  } catch (error) {
    if (editing === session) feedback("server-error", error.message);
  } finally {
    if (editing === session) submit.textContent = session.server?.connected ? "保存并重连" : "连接";
  }
});
