import { byId, element, api } from "./ui.js";

// ******************************** 注册服务列表 ********************************

const base = "/api/mcp/servers";
const list = byId("servers");
const dialog = byId("server-dialog");
const form = byId("server-form");
const submit = byId("connect-server");
const add = byId("toggle-form");
const retry = byId("retry-servers");
// 仅在本标签页内保留用户刚填过的配置，方便继续编辑。列表接口不返回凭据。
const entered = new Map();
let editing = null;
let changing = false;
let listVersion = 0;

function feedback(id, message = "") {
  byId(id).textContent = message;
  byId(id).hidden = !message;
}

function listError(message = "") {
  feedback("server-list-error", message);
  retry.hidden = !message;
}

function setChanging(value) {
  changing = value;
  submit.disabled = value;
  add.disabled = value;
  retry.disabled = value;
  list.querySelectorAll(".server-action").forEach(button => { button.disabled = value; });
}

function serverUrl(name) {
  return base + "/" + encodeURIComponent(name);
}

function action(label, handler) {
  const button = element("button", "server-action", label);
  button.type = "button";
  button.disabled = changing;
  button.addEventListener("click", handler);
  return button;
}

function showServers(servers) {
  const expanded = new Set([...list.querySelectorAll("details[open]")].map(detail => detail.dataset.name));
  list.replaceChildren();
  if (!servers.length) list.append(element("div", "server-empty", "还没有连接 MCP 服务"));
  for (const server of servers) {
    const card = element("div", "server-card");
    card.dataset.name = server.name;
    card.append(element("div", "server-title", server.name));
    const meta = element("div", "server-meta-line");
    const status = server.connected ? "已连接" : "已断开";
    const dot = element("span", "server-status-dot " + (server.connected ? "connected" : "disconnected"));
    dot.title = status;
    meta.append(dot, element("span", "server-badge", status), element("span", "server-badge", server.type),
      element("span", "server-badge", server.tools.length + " 个工具"));
    card.append(meta);

    if (server.tools.length) {
      const detail = element("details", "server-tools");
      detail.dataset.name = server.name;
      detail.open = expanded.has(server.name);
      detail.append(element("summary", "server-tools-toggle", "工具列表"));
      const items = element("ul", "server-tools-list");
      for (const name of server.tools) items.append(element("li", "", name));
      detail.append(items);
      card.append(detail);
    }
    const actions = element("div", "server-actions");
    const edit = action("编辑", event => openForm(server, event.currentTarget));
    edit.setAttribute("aria-haspopup", "dialog");
    edit.setAttribute("aria-controls", "server-dialog");
    actions.append(edit,
      action("重连", () => changeConnection(server, true)),
      action("断开", () => changeConnection(server, false)));
    card.append(actions);
    list.append(card);
  }
}

/** 列表加载与聊天分别请求。慢列表不能覆盖更近一次修改返回的新状态。 */
async function loadServers() {
  const version = ++listVersion;
  try {
    const data = await api(base);
    if (version !== listVersion) return;
    showServers(data.servers);
    listError();
  } catch (error) {
    if (version === listVersion) listError(error.message);
  }
}

async function changeConnection(server, reconnect) {
  if (changing || (!reconnect && !confirm("断开 MCP 服务「" + server.name + "」？"))) return;
  ++listVersion;
  setChanging(true);
  listError();
  try {
    const data = await api(serverUrl(server.name) + (reconnect ? "/reconnect" : ""), { method: reconnect ? "POST" : "DELETE" });
    if (!reconnect) entered.delete(server.name);
    showServers(data.servers);
  } catch (error) {
    listError(error.message);
  } finally {
    setChanging(false);
  }
}

retry.addEventListener("click", loadServers);

// ******************************** 连接与编辑弹窗 ********************************

const field = name => byId("server-" + name);
const fields = ["name", "type", "command", "args", "cwd", "env", "url", "token", "headers"];
let opener;

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
  editing = server?.name ?? null;
  opener = button;
  form.reset();
  const saved = entered.get(editing);
  if (saved) {
    for (const name of fields) field(name).value = saved[name];
    field("parallel").checked = saved.parallel;
  } else if (server) {
    field("name").value = server.name;
    field("type").value = server.type;
  }
  field("name").readOnly = Boolean(server);
  byId("edit-hint").hidden = !server || Boolean(saved);
  byId("server-form-title").textContent = server ? "编辑 MCP 服务" : "连接 MCP 服务";
  submit.textContent = server ? "保存并重连" : "连接";
  feedback("server-error");
  updateFields();
  add.setAttribute("aria-expanded", String(!server));
  dialog.showModal();
  (server ? field("type") : field("name")).focus();
}

add.addEventListener("click", () => openForm(null, add));
byId("cancel-form").addEventListener("click", () => dialog.close());
dialog.addEventListener("close", () => {
  add.setAttribute("aria-expanded", "false");
  const card = [...list.children].find(node => node.dataset.name === editing);
  const trigger = opener?.isConnected ? opener : card?.querySelector("[aria-haspopup=dialog]") || add;
  if (byId("sidebar").inert) byId("sidebar-toggle").focus();
  else if (trigger.disabled) byId("brand-home").focus();
  else trigger.focus();
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
  if (changing) return;
  const values = Object.fromEntries(fields.map(name => [name, field(name).value]));
  values.parallel = field("parallel").checked;
  const name = values.name.trim();
  feedback("server-error");
  listError();
  try {
    const body = readConfig(values);
    const path = editing ? serverUrl(editing) : base;
    if (!editing) body.name = name;
    ++listVersion;
    setChanging(true);
    submit.textContent = "正在连接…";
    const data = await api(path, { method: editing ? "PUT" : "POST", body });
    entered.set(name, values);
    showServers(data.servers);
    if (dialog.open) dialog.close();
  } catch (error) {
    if (dialog.open) feedback("server-error", error.message);
    else listError(error.message);
  } finally {
    setChanging(false);
    submit.textContent = editing ? "保存并重连" : "连接";
  }
});

loadServers();
