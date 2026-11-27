// web_flow.py 提供临时数据库、模型与 MCP 服务；浏览器使用实际网页和 HTTP 路由。
const assert = require("node:assert/strict");
const { chromium } = require("playwright");

const [base, otherId] = process.argv.slice(2);
const card = (page, name) => page.locator('.server-card[data-name="' + name + '"]');
const button = (panel, name) => panel.getByRole("button", { name, exact: true });
const isOpen = details => details.evaluate(node => node.open);

async function openEditor(page) {
  await page.locator("#edit-agent").click();
  await page.locator("#save-agent:enabled").waitFor();
}

async function saveEditor(page) {
  await page.locator("#save-agent").click();
  await page.locator("#agent-dialog").waitFor({ state: "hidden" });
}

async function checkDelayedEditorLoad(page, mirror) {
  let read, release;
  const captured = new Promise(resolve => { read = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  await page.route("**/api/agents/1", async route => {
    const old = await route.fetch();
    read();
    await gate;
    await route.fulfill({ response: old });
  }, { times: 1 });
  await page.locator("#edit-agent").click();
  await captured;
  try {
    await openEditor(mirror);
    await mirror.locator("#agent-name").fill("载入期间已更新");
    await saveEditor(mirror);
    await page.waitForFunction(() => document.querySelector("#agent-select").selectedOptions[0]?.text === "载入期间已更新");
  } finally {
    release();
  }
  await page.locator("#agent-error").waitFor();
  assert(await page.locator("#save-agent").isDisabled());
  await page.locator("#close-agent").click();
}

async function checkTools(page, mirror) {
  const remote = card(page, "remote");
  await remote.locator("summary").waitFor();
  assert.equal(await isOpen(remote.locator("details")), false);
  await remote.locator("summary").click();
  assert(await button(remote, "全选").isDisabled());
  assert(await button(remote, "全不选").isEnabled());
  await button(remote, "全不选").click();
  await remote.getByText("此服务尚未公布工具").waitFor();

  await page.locator("#toggle-form").click();
  await page.locator("#server-name").fill("demo");
  await page.locator("#server-command").fill("python3");
  await page.locator("#server-args").fill("examples/mcp_demo.py");
  await page.locator("#connect-server").click();
  const demo = card(page, "demo");
  for (const tab of [page, mirror]) {
    await card(tab, "demo").getByRole("checkbox", { name: "demo_code" }).waitFor();
    assert(await isOpen(card(tab, "demo").locator("details")));
  }

  await demo.locator("summary").click();
  await page.locator("#agent-select").selectOption(otherId);
  await page.getByText("此助手还没有绑定 MCP 服务").waitFor();
  await page.locator("#agent-select").selectOption("1");
  await demo.locator("summary").waitFor();
  assert.equal(await isOpen(demo.locator("details")), false);
  await page.reload();
  await demo.locator("summary").waitFor();
  assert.equal(await isOpen(demo.locator("details")), false);

  await button(remote, "重连").click();
  await remote.getByRole("checkbox", { name: "new_tool" }).waitFor();
  assert.equal(await isOpen(demo.locator("details")), false);
}

async function checkEditors(page, mirror) {
  await openEditor(page);
  await page.locator("#agent-memory").fill("自己的未保存草稿");
  await mirror.locator("#agent-select").selectOption(otherId);
  await openEditor(mirror);
  await mirror.locator("#agent-name").fill("其他助手已更新");
  await saveEditor(mirror);
  await page.waitForFunction(() => [...document.querySelector("#agent-select").options]
    .some(option => option.text === "其他助手已更新"));
  assert(await page.locator("#save-agent").isEnabled());

  await mirror.locator("#agent-select").selectOption("1");
  await openEditor(mirror);
  await mirror.locator("#agent-instructions").fill("另一个标签页的指令");
  await saveEditor(mirror);
  await page.locator("#agent-error").waitFor();
  assert(await page.locator("#save-agent").isDisabled());
  assert.equal(await page.locator("#agent-memory").inputValue(), "自己的未保存草稿");
  await page.locator("#close-agent").click();

  await checkDelayedEditorLoad(page, mirror);
  await page.bringToFront();
  await openEditor(page);
  assert.equal(await page.locator("#agent-instructions").inputValue(), "另一个标签页的指令");
  await page.locator("#agent-memory").fill("记忆工具运行期间的草稿");
  const api = page.context().request;
  const conversation = await api.post(base + "/api/conversations", { data: { agent_id: 1 } });
  assert(conversation.ok());
  const chat = await conversation.json();
  const turn = await api.post(base + "/api/conversations/" + chat.id + "/messages", { data: { message: "更新记忆" } });
  assert(turn.ok());
  await page.locator("#agent-error").waitFor({ timeout: 12000 });
  assert(await page.locator("#save-agent").isDisabled());
  assert.equal(await page.locator("#agent-memory").inputValue(), "记忆工具运行期间的草稿");
}

async function run() {
  const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE });
  try {
    const context = await browser.newContext();
    const errors = [];
    context.on("page", page => page.on("pageerror", error => errors.push(error.message)));
    const page = await context.newPage();
    const mirror = await context.newPage();
    for (const tab of [page, mirror]) {
      await tab.goto(base + "/?agent=1&conversation=new");
      await card(tab, "remote").locator("summary").waitFor();
    }
    await checkTools(page, mirror);
    await checkEditors(page, mirror);
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
}

run().catch(error => { console.error(error); process.exitCode = 1; });
