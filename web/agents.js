import { api, byId, changes } from "./ui.js";

const picker = byId("agent-select");
const dialog = byId("agent-dialog");
const form = byId("agent-form");
const errorText = byId("agent-error");
let editing;

export function showAgents(agents, selectedId) {
  picker.replaceChildren(...agents.map(agent => {
    const option = new Option(agent.name, String(agent.id));
    option.dataset.model = agent.model;
    return option;
  }));
  picker.value = String(selectedId);
  picker.disabled = !picker.value;
  byId("new-agent").disabled = byId("edit-agent").disabled = !picker.value;
}

function showError(message = "") {
  errorText.textContent = message;
  errorText.hidden = !message;
}

export function setupAgentEditor(onSaved) {
  const save = byId("save-agent");
  let revisionTimer;

  function observeRevision(edit, revision) {
    if (editing !== edit || !dialog.open) return;
    edit.latestRevision = Math.max(edit.latestRevision, revision);
    if (edit.latestRevision > edit.revision) {
      showError("助手设置或私有记忆已更新。当前草稿仍在，重新打开可查看最新内容。");
      save.disabled = true;
    }
  }

  async function checkRevision() {
    const edit = editing;
    if (!dialog.open || document.hidden || !edit?.revision || edit.checking || edit.latestRevision > edit.revision) return;
    edit.checking = true;
    try {
      const agent = await api("/api/agents/" + edit.id);
      observeRevision(edit, agent.revision);
    } catch {
      // 后台核对暂时失败时保留草稿，保存时服务端仍会检查版本。
    } finally {
      edit.checking = false;
    }
  }

  changes.addEventListener("message", event => {
    const { agentsChanged, agent_id, revision } = event.data ?? {};
    if (agentsChanged && editing?.id === agent_id && Number.isSafeInteger(revision)) {
      observeRevision(editing, revision);
    }
  });
  window.addEventListener("focus", checkRevision);
  document.addEventListener("visibilitychange", checkRevision);

  async function open(create) {
    const option = picker.selectedOptions[0];
    if (!option) return;
    const edit = { id: create ? null : Number(option.value), latestRevision: 0 };
    clearInterval(revisionTimer);
    editing = edit;
    form.reset();
    showError();
    byId("agent-form-title").textContent = create ? "新建助手" : "编辑助手";
    byId("agent-model").value = option.dataset.model;
    const fields = [...form.querySelectorAll("input, textarea")];
    for (const field of fields) field.readOnly = !create;
    save.disabled = !create;
    dialog.showModal();
    if (create) return;
    try {
      const agent = await api("/api/agents/" + edit.id);
      if (editing !== edit || !dialog.open) return;
      edit.revision = agent.revision;
      for (const name of ["name", "model", "instructions", "memory"]) {
        form.elements.namedItem(name).value = agent[name];
      }
      for (const field of fields) field.readOnly = false;
      save.disabled = false;
      observeRevision(edit, agent.revision);
      // 模型从后端写入记忆。只在已有助手的编辑窗口打开时追踪版本。
      revisionTimer = setInterval(checkRevision, 5000);
    } catch (error) {
      if (editing === edit) showError(error.message);
    }
  }

  byId("new-agent").addEventListener("click", () => open(true));
  byId("edit-agent").addEventListener("click", () => open(false));
  byId("close-agent").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    if (!dialog.open) {
      clearInterval(revisionTimer);
      editing = null;
    }
  });

  form.addEventListener("submit", async event => {
    event.preventDefault();
    const edit = editing;
    if (!edit || save.disabled) return;
    const body = Object.fromEntries(new FormData(form));
    if (edit.id) body.revision = edit.revision;
    save.disabled = true;
    showError();
    try {
      const agent = await api(edit.id ? "/api/agents/" + edit.id : "/api/agents", {
        method: edit.id ? "PUT" : "POST", body,
      });
      const active = dialog.open && editing === edit;
      if (active) dialog.close();
      onSaved(agent, active && !edit.id);
    } catch (error) {
      if (editing === edit && !(edit.latestRevision > edit.revision)) {
        showError(error.message);
        save.disabled = error.status === 409;
      }
    }
  });
}
