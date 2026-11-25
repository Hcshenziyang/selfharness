// ******************************** 通用页面操作 ********************************

export const byId = id => document.getElementById(id);

export function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** 请求本地 API。入队确认和停止回合的响应只有状态码。 */
export async function api(path, { method = "GET", body } = {}) {
  const options = { method };
  if (body !== undefined) {
    options.headers = { "Content-Type": "application/json" };
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  if (!response.ok) {
    const data = await response.json().catch(() => null);
    const error = new Error(data?.error || "请求失败（" + response.status + "）");
    error.status = response.status;
    throw error;
  }
  if (response.status === 202 || response.status === 204) return;
  return response.json();
}

// ******************************** 移动端侧栏 ********************************

const sidebar = byId("sidebar");
const toggle = byId("sidebar-toggle");
const mobile = matchMedia("(max-width: 768px)");

export function setSidebar(open) {
  const wasOpen = sidebar.classList.contains("open");
  open = open && mobile.matches;
  sidebar.classList.toggle("open", open);
  sidebar.inert = mobile.matches && !open;
  document.querySelector("main").inert = open;
  byId("sidebar-overlay").hidden = !open;
  toggle.setAttribute("aria-expanded", String(open));
  if (open) {
    sidebar.setAttribute("role", "dialog");
    sidebar.setAttribute("aria-modal", "true");
    byId("close-sidebar").focus();
  } else {
    sidebar.removeAttribute("role");
    sidebar.removeAttribute("aria-modal");
    if (wasOpen) (mobile.matches ? toggle : byId("brand-home")).focus();
  }
}

toggle.addEventListener("click", () => setSidebar(true));
byId("close-sidebar").addEventListener("click", () => setSidebar(false));
byId("sidebar-overlay").addEventListener("click", () => setSidebar(false));
mobile.addEventListener("change", () => setSidebar(false));
setSidebar(false);

sidebar.addEventListener("keydown", event => {
  if (!sidebar.classList.contains("open")) return;
  if (event.key === "Escape") {
    event.preventDefault();
    setSidebar(false);
  } else if (event.key === "Tab") {
    const buttons = [...sidebar.querySelectorAll("button:enabled")].filter(btn => btn.getClientRects().length);
    const first = buttons[0];
    const last = buttons.at(-1);
    if (event.shiftKey && document.activeElement === first || !event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  }
});
