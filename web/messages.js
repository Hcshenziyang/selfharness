import "./markdown-it.min.js";
import { byId, element } from "./ui.js";

// ******************************** Markdown 与复制 ********************************

// 禁用原始 HTML 和外部图片，模型文本只通过 Markdown 规则生成标记。
const markdown = window.markdownit({ html: false, breaks: true }).disable("image");
markdown.validateLink = url => /^(https?:\/\/|mailto:)/i.test(url);

function copyButton(text, label) {
  const button = element("button", "btn-bubble-action", "复制");
  button.type = "button";
  button.setAttribute("aria-label", label);
  let timer;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "已复制";
    } catch {
      button.textContent = "复制失败";
    }
    clearTimeout(timer);
    timer = setTimeout(() => { button.textContent = "复制"; }, 1800);
  });
  return button;
}

/** 使用实际代码节点的文本复制，保留缩进和换行。 */
function renderMarkdown(bubble, content) {
  bubble.innerHTML = markdown.render(content);
  for (const link of bubble.querySelectorAll("a[href]")) {
    link.target = "_blank";
    link.rel = "noopener noreferrer";
  }
  for (const code of bubble.querySelectorAll("pre > code")) {
    const pre = code.parentElement;
    const block = element("div", "code-block");
    const bar = element("div", "code-heading");
    bar.append(element("span", "", code.className.replace(/^language-/, "")), copyButton(code.textContent, "复制代码"));
    pre.replaceWith(block);
    block.append(bar, pre);
  }
}

// ******************************** 气泡与阅读位置 ********************************

const messages = byId("messages");
const scroll = byId("chat-scroll");
const bottom = byId("scroll-bottom");
let previous = [];

function nearBottom() {
  return scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight <= 80;
}

function updateBottomButton() {
  bottom.hidden = nearBottom();
}

bottom.addEventListener("click", () => scroll.scrollTo({ top: scroll.scrollHeight, behavior: "smooth" }));
scroll.addEventListener("scroll", updateBottomButton, { passive: true });
new ResizeObserver(updateBottomButton).observe(scroll);

function messageRow(item, stopTurn) {
  const row = element("div", "message " + item.role + (item.status ? " pending" : ""));
  const avatar = element("div", "avatar");
  avatar.setAttribute("aria-hidden", "true");
  if (item.role === "user") avatar.textContent = "我";
  else avatar.append(document.querySelector(".brand-logo svg").cloneNode(true));

  const wrap = element("div", "bubble-wrap");
  const bubble = element("div", "bubble");
  const actions = element("div", "message-actions");
  if (item.status) {
    const running = item.status === "running";
    bubble.textContent = running ? "回复中…" : item.status === "queued" ? "排队中" : "正在停止…";
    if (item.status !== "stopping") {
      const stop = element("button", "btn-bubble-action", running ? "停止" : "取消排队");
      stop.type = "button";
      if (running) stop.title = "停止本轮，已发出的工具操作可能仍会执行";
      stop.addEventListener("click", async () => {
        stop.disabled = true;
        try { await stopTurn(item.id); } finally { stop.disabled = false; }
      });
      bubble.append(stop);
    }
  } else {
    if (item.role === "assistant") renderMarkdown(bubble, item.content);
    else bubble.textContent = item.content;
    actions.append(copyButton(item.content, item.role === "assistant" ? "复制回复" : "复制消息"));
  }
  wrap.append(bubble);
  if (actions.childElementCount) wrap.append(actions);
  row.append(avatar, wrap);
  return row;
}

/** 打开会话时定位到末尾；轮询时保留相同气泡节点和用户的阅读位置。 */
export function renderMessages(history, turns, stopTurn, open = false) {
  const follow = open || nearBottom();
  const rows = [...history];
  for (const turn of turns) {
    rows.push({ role: "user", content: turn.message }, { role: "assistant", status: turn.status, id: turn.id });
  }
  if (open) {
    messages.replaceChildren();
    previous = [];
  }
  rows.forEach((item, index) => {
    const old = previous[index];
    if (old && old.role === item.role && old.content === item.content && old.status === item.status && old.id === item.id) return;
    const row = messageRow(item, stopTurn);
    if (messages.children[index]) messages.children[index].replaceWith(row);
    else messages.append(row);
  });
  while (messages.childElementCount > rows.length) messages.lastElementChild.remove();
  previous = rows;
  byId("empty-chat").hidden = rows.length > 0;
  if (follow) scroll.scrollTop = scroll.scrollHeight;
  updateBottomButton();
}
