import { api, byId, changes } from "./ui.js";

export function setupToolSettings(currentAgent) {
  const dialog = byId("tool-dialog");
  const form = byId("tool-form");
  const save = byId("save-tools");
  const error = byId("tool-error");
  const switches = ["echo", "memory_update", "read_file", "write_file"];
  let edit;

  function feedback(message = "") {
    error.textContent = message;
    error.hidden = !message;
  }

  byId("edit-tools").addEventListener("click", async () => {
    const session = { agentId: currentAgent() };
    if (!session.agentId) return;
    edit = session;
    form.reset();
    feedback();
    for (const input of form.querySelectorAll("input")) input.disabled = true;
    save.disabled = true;
    dialog.showModal();
    try {
      const policy = await api("/api/agents/" + session.agentId + "/tools");
      if (edit !== session || !dialog.open) return;
      session.revision = policy.revision;
      for (const name of switches) form.elements[name].checked = policy[name];
      form.elements.workspace_root.value = policy.workspace_root ?? "";
      for (const input of form.querySelectorAll("input")) input.disabled = false;
      save.disabled = false;
    } catch (err) {
      if (edit === session) feedback(err.message);
    }
  });
  changes.addEventListener("message", event => {
    if (dialog.open && event.data?.toolPolicyChanged === edit?.agentId) {
      feedback("另一个标签页已保存工具设置，请重新打开查看最新内容。");
      save.disabled = true;
    }
  });
  byId("close-tools").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => { if (!dialog.open) edit = null; });
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (!edit || save.disabled) return;
    const session = edit;
    const body = { revision: session.revision, workspace_root: form.elements.workspace_root.value.trim() || null };
    for (const name of switches) body[name] = form.elements[name].checked;
    feedback();
    save.disabled = true;
    try {
      await api("/api/agents/" + session.agentId + "/tools", { method: "PUT", body });
      changes.postMessage({ toolPolicyChanged: session.agentId });
      if (edit === session && dialog.open) dialog.close();
    } catch (err) {
      if (edit === session) {
        feedback(err.message);
        save.disabled = err.status === 409;
      }
    }
  });
}
