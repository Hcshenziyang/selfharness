"""模拟模型和 HTTP MCP，测试网页接口实际连接和工具调用。"""

from concurrent.futures import ThreadPoolExecutor
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from urllib.parse import urlsplit
import json
import os
import select
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest


BINARY = sys.argv.pop(1)
ROOT = Path(__file__).resolve().parent.parent


# ******************************** 模拟模型与 HTTP MCP 服务 ********************************

def model_text(text="完成"):
    """构造模型完成回复时返回的 Responses 文本输出项。"""
    return [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}]


def model_call(name, call_id, arguments="{}"):
    """构造一条带调用 ID 的工具请求，让被测 Agent Loop 进入工具执行分支。"""
    return [{"type": "function_call", "name": name, "call_id": call_id, "arguments": arguments}]


SERVERS = "/api/agents/1/mcp/servers"


class Handler(BaseHTTPRequestHandler):
    """用同一个本地 HTTP 服务模拟模型和 MCP，具体回复由各测试设置的状态决定。"""

    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def reply(self, status, payload=None, headers=None):
        data = b"" if payload is None else payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        self.send_response(status)
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.reply(405)

    def do_DELETE(self):
        on_delete = self.server.state.get("on_mcp_delete")
        if on_delete:
            on_delete()
        self.reply(204)

    def do_POST(self):
        """按路径把请求分派给模型接口或 MCP 接口。"""
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/v1/responses":
            self.model(request)
        elif self.path == "/mcp":
            self.mcp(request)
        else:
            self.reply(404)

    def model(self, request):
        """记录并校验 Responses 请求，按用例返回工具调用、最终回复或上游错误。"""
        state = self.server.state
        state["requests"].append(request)
        history = request.get("input")
        tools = request.get("tools", [])
        if (self.headers.get("Authorization") != "Bearer model-secret"
                or request.get("model") not in state.get("models", {"model"})
                or request.get("store") is not False or request.get("stream") is not False
                or request.get("parallel_tool_calls") is not True
                or not isinstance(history, list) or not history or "messages" in request or "thinking" in request
                or any(tool.get("type") != "function" or "function" in tool or "parameters" not in tool
                       for tool in tools)):
            state["errors"].append("Responses 请求格式或认证错误")
            self.reply(400)
            return
        if state["fail_model"] or (state.get("fail_after_tool") and history[-1].get("type") == "function_call_output"):
            self.reply(state.get("model_error_status", 500))
            return
        if "model_output" in state:
            output = state["model_output"](request)
            self.reply(200, {"status": "completed", "output": output}, {"Content-Type": "application/json"})
            return
        names = {tool["name"] for tool in tools}
        expected = {"echo", *state["tools"]}
        if names != expected:
            state["errors"].append(f"工具清单错误: {names} != {expected}")
            self.reply(500)
            return
        if "model_response" in state:
            self.reply(200, state["model_response"], {"Content-Type": "application/json"})
            return
        last_user = max(i for i, item in enumerate(history) if item.get("role") == "user")
        if history[-1].get("role") == "user" and state["tools"]:
            output = [{"type": "reasoning", "id": f"rs_{last_user}", "summary": [],
                       "encrypted_content": "opaque-test-value"}]
            for index, name in enumerate(state["tools"]):
                output.append({"type": "function_call", "id": f"fc_{last_user}_{index}",
                               "call_id": f"call_{last_user}_{index}", "name": name,
                               "arguments": json.dumps({"text": "回声验证"}) if name == "echo" else "{}",
                               "status": "completed"})
        else:
            results = [(item["call_id"], item["output"]) for item in history[last_user:]
                       if item.get("type") == "function_call_output"]
            expected_results = [(f"call_{last_user}_{index}", state["results"][name])
                                for index, name in enumerate(state["tools"])]
            if results != expected_results:
                state["errors"].append(f"工具结果错误: {results} != {expected_results}")
                self.reply(500)
                return
            output = [{"type": "message", "id": f"msg_{last_user}", "role": "assistant", "status": "completed",
                       "content": [{"type": "output_text", "text": text, "annotations": []}
                                   for text in ("工具执行", "完成")]}]
        self.reply(200, {"object": "response", "status": "completed", "output": output},
                   {"Content-Type": "application/json"})

    def mcp(self, request):
        """校验认证及 MCP 会话 ID，模拟握手、工具发现和调用后的 JSON 或 SSE 回包。"""
        if (self.headers.get("Authorization") != "Bearer " + self.server.state.get("mcp_token", "test-token")
                or self.headers.get("X-Test") != "test-header"):
            self.server.state["errors"].append("HTTP MCP 请求头错误")
            self.reply(401)
            return
        method = request["method"]
        if method != "initialize" and self.headers.get("Mcp-Session-Id") != "web-test":
            self.server.state["errors"].append("HTTP MCP 会话 ID 未回传")
            self.reply(400)
            return
        if "id" not in request:
            self.reply(202)
            return
        if method == "initialize":
            result = {"protocolVersion": request["params"]["protocolVersion"],
                      "capabilities": {"tools": {}}, "serverInfo": {"name": "test", "version": "1"}}
        elif method == "tools/list":
            if self.server.state.get("fail_discovery"):
                self.reply(200, {"jsonrpc": "2.0", "id": request["id"],
                                 "error": {"code": -32603, "message": "discovery failed"}},
                           {"Content-Type": "application/json", "Mcp-Session-Id": "web-test"})
                return
            names = ["a.b", "a b"] if self.server.state.get("duplicate_tools") else ["demo_code"]
            result = {"tools": [{"name": name, "description": "HTTP test",
                                 "inputSchema": {"type": "object", "properties": {}}} for name in names]}
            if "mcp_tools" in self.server.state:
                result = {"tools": self.server.state["mcp_tools"]}
        elif method == "tools/call":
            callback = self.server.state.get("call_mcp")
            result = callback(request["params"]["name"]) if callback else {
                "content": [{"type": "text", "text": "HTTP-TEST-RESULT"}]}
        else:
            self.reply(404)
            return
        response = {"jsonrpc": "2.0", "id": request["id"], "result": result}
        if method == "tools/call" and not self.server.state.get("json_tool_results"):
            # Streamable HTTP 的调用响应可以使用 SSE。
            self.reply(200, f"event: message\ndata: {json.dumps(response)}\n\n".encode(),
                       {"Content-Type": "text/event-stream", "Mcp-Session-Id": "web-test"})
        else:
            self.reply(200, response, {"Content-Type": "application/json", "Mcp-Session-Id": "web-test"})


# ******************************** 测试环境与请求辅助方法 ********************************

class WebIntegration(unittest.TestCase):
    """每个用例启动独立的 DogOHarness 子进程、模拟上游及临时 SQLite 数据库。"""

    def setUp(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.state = {"tools": [], "results": {}, "errors": [], "requests": [], "fail_model": False}
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.temp = tempfile.TemporaryDirectory(prefix="dogo-web-test-")
        self.db_path = Path(self.temp.name) / "history.sqlite3"
        self.env = dict(os.environ, LLM_API_KEY="model-secret", LLM_MODEL="model", DOGO_WEB_PORT="0",
                        DOGO_DB_PATH=str(self.db_path),
                        DOGO_MAX_MODEL_CONCURRENCY="2", DOGO_MAX_UNFINISHED_TURNS="16",
                        LLM_API_URL=f"http://127.0.0.1:{self.server.server_port}/v1/responses",
                        NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
        self.process = None
        self.addCleanup(self.cleanup)
        self.start_app()

    def start_app(self):
        """启动被测程序，从启动日志读取系统实际分配的本地端口。"""
        self.process = subprocess.Popen([BINARY], cwd=ROOT, env=self.env, stdin=subprocess.DEVNULL,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        ready, _, _ = select.select([self.process.stdout], [], [], 10)
        self.assertTrue(ready, "网页服务未启动")
        line = self.process.stdout.readline().strip()
        self.base = line.split()[-1]
        self.assertTrue(self.base.startswith("http://127.0.0.1:"), line)

    def stop_app(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=3)
        self.process.stdout.close()
        self.process.stderr.close()
        self.process = None

    def cleanup(self):
        self.stop_app()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        self.temp.cleanup()

    def request(self, path, method="GET", body=None, headers=None):
        """经由网页 HTTP 接口发送请求，保留状态码和响应正文供断言。"""
        headers = dict(headers or {})
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        request = Request(self.base + path, data=data, headers=headers, method=method)
        try:
            response = urlopen(request, timeout=20)
        except HTTPError as error:
            response = error
        with response:
            content = response.read()
            if "application/json" in response.headers.get("Content-Type", ""):
                content = json.loads(content)
            return response.status, content

    def new_conversation(self, agent_id=None):
        """创建空会话，返回可用于读取历史和提交消息的接口路径。"""
        body = {"agent_id": agent_id} if agent_id is not None else None
        status, info = self.request("/api/conversations", "POST", body)
        self.assertEqual(status, 200, info)
        self.assertEqual(info["title"], "新对话")
        return f"/api/conversations/{info['id']}/messages"

    def wait_progress(self, path, matches):
        """在超时前轮询进度，直到用例指定的队列状态出现。"""
        until = time.monotonic() + 5
        while time.monotonic() < until:
            status, data = self.request(path.removesuffix("messages") + "progress")
            self.assertEqual(status, 200, data)
            if matches(data):
                return data
            time.sleep(0.02)
        self.fail(f"会话状态未达到预期: {data}")

    def submit_connection(self, path, text):
        """发送消息后立刻交还 HTTP 连接，供用例模拟发送方关闭标签页。"""
        address = urlsplit(self.base)
        conn = HTTPConnection(address.hostname, address.port, timeout=10)
        conn.request("POST", path, json.dumps({"message": text}), {"Content-Type": "application/json"})
        self.addCleanup(conn.close)
        return conn

    def binding(self, name, base=SERVERS):
        status, data = self.request(base)
        self.assertEqual(status, 200, data)
        server = next(server for server in data["servers"] if server["name"] == name)
        return base + "/" + str(server["id"]), server

    def grant_all(self, name, base=SERVERS):
        path, server = self.binding(name, base)
        status, data = self.request(path + "/tools", "PATCH", {
            "revision": server["revision"], "tools": dict.fromkeys(server["tools"], True),
        })
        self.assertEqual(status, 200, data)

    def connect_test_tools(self, hints, callback, *, server_parallel=False, agent_id=1):
        """通过真实注册接口公布测试工具，并为远端调用指定回调和并发提示。"""
        state = self.server.state
        state["mcp_tools"] = [{"name": name, "inputSchema": {"type": "object"},
                               **({"annotations": {"readOnlyHint": hint}} if hint is not None else {})}
                              for name, hint in hints.items()]
        state["call_mcp"] = callback
        config = {"name": "remote", "type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                  "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        if server_parallel:
            config["supports_parallel_tool_calls"] = True
        base = f"/api/agents/{agent_id}/mcp/servers"
        status, response = self.request(base, "POST", config)
        self.assertEqual(status, 200, response)
        self.grant_all("remote", base)
        state["tools"] = [f"mcp__remote__{name}" for name in hints]
        state["results"] = {f"mcp__remote__{name}": name for name in hints}

    # ******************************** 助手归属、配置快照与迁移 ********************************

    def test_agent_turn_isolation_and_queued_edits(self):
        """两个助手同时请求网关。工具等待期间修改 A，只影响下一轮及新会话。"""
        def create(name):
            status, agent = self.request("/api/agents", "POST", {
                "name": name, "model": "model-" + name, "instructions": "规则-" + name, "memory": "笔记-" + name,
            })
            self.assertEqual(status, 200, agent)
            return agent

        a, b = create("A"), create("B")
        chat_a, chat_b = self.new_conversation(a["id"]), self.new_conversation(b["id"])
        a_started, b_started = threading.Event(), threading.Event()
        in_tool, release_tool = threading.Event(), threading.Event()

        def model(request):
            last = request["input"][-1]
            if last.get("content") == "A1":
                a_started.set()
                self.assertTrue(b_started.wait(6))
                return model_call("mcp__remote__slow", "hold-a")
            if last.get("content") == "B1":
                b_started.set()
            return model_text()

        def tool(name):
            in_tool.set()
            self.assertTrue(release_tool.wait(6))
            return {"content": [{"type": "text", "text": name}]}

        state = self.server.state
        state.update(model_output=model, models={"model-A", "model-B", "model-A2"})
        self.connect_test_tools({"slow": True}, tool, agent_id=a["id"])
        with ThreadPoolExecutor(max_workers=1) as pool:
            first = pool.submit(self.request, chat_a, "POST", {"message": "A1"})
            try:
                self.assertTrue(a_started.wait(5))
                self.assertEqual(self.request(chat_b, "POST", {"message": "B1"})[0], 200)
                self.assertTrue(in_tool.wait(5))
                self.assertEqual(self.request(chat_a.removesuffix("messages") + "turns", "POST", {"message": "A2"})[0], 202)
                status, updated = self.request(f"/api/agents/{a['id']}", "PUT", {
                    **a, "model": "model-A2", "instructions": "更新的规则", "memory": "更新的笔记",
                })
                self.assertEqual(status, 200, updated)
            finally:
                b_started.set()
                release_tool.set()
            self.assertEqual(first.result(timeout=5)[0], 200)
        progress = self.wait_progress(chat_a, lambda p: not p["turns"])
        self.assertIsNone(progress["last_failure"])
        second_chat = self.new_conversation(a["id"])
        self.assertEqual(self.request(second_chat, "POST", {"message": "A3"})[0], 200)

        expected = {"A1": a, "B1": b, "A2": updated, "A3": updated}
        seen = []
        for request in state["requests"]:
            users = [item["content"] for item in request["input"] if item.get("role") == "user"]
            question = users[-1]
            seen.append(question)
            self.assertEqual(users, ["A1", "A2"] if question == "A2" else [question])
            agent = expected[question]
            self.assertEqual(request["model"], agent["model"])
            self.assertEqual(request["instructions"], agent["instructions"] + "\n\n助手私有笔记（供参考）\n" + agent["memory"])
        self.assertCountEqual(seen, ["A1", "A1", "A2", "A3", "B1"])
        with sqlite3.connect(self.db_path) as db:
            rows = db.execute("SELECT payload FROM messages").fetchall()
            self.assertTrue(rows)
            for (payload,) in rows:
                for agent in (a, b, updated):
                    self.assertNotIn(agent["instructions"], payload)
                    self.assertNotIn(agent["memory"], payload)
        self.assertEqual(state["errors"], [])

    def test_agent_settings_and_conversation_lists(self):
        """助手配置持久化，列表按归属过滤，过期编辑和不存在的助手返回明确错误。"""
        default_chat = self.new_conversation()
        default_id = self.request("/api/agents")[1]["default_agent_id"]
        default_path = f"/api/agents/{default_id}"
        default = self.request(default_path)[1]
        self.assertEqual((default["name"], default["model"]), ("默认助手", "model"))
        edit = {**default, "model": "edited", "memory": "用户的笔记"}
        self.assertEqual(self.request(default_path, "PUT", edit)[0], 200)
        self.assertEqual(self.request(default_path, "PUT", default)[0], 409)
        self.assertEqual(self.request("/api/agents", "POST", {"name": " ", "model": "m"})[0], 400)
        self.assertEqual(self.request("/api/agents", "POST", {"name": "A", "model": " "})[0], 400)
        self.assertEqual(self.request("/api/agents", "POST", {"name": "A", "model": "m", "memory": "文"*4001})[0], 400)
        self.assertEqual(self.request("/api/conversations", "POST", {"agent_id": 999})[0], 404)
        self.assertEqual(self.request("/api/conversations?agent_id=999")[0], 404)
        status, agent = self.request("/api/agents", "POST", {"name": "私有助手", "model": "m"})
        self.assertEqual(status, 200)
        path = self.new_conversation(agent["id"])
        self.stop_app()
        self.env["LLM_MODEL"] = "different-seed"
        self.start_app()
        self.assertEqual(self.request(default_path)[1]["model"], "edited")
        self.assertEqual(self.request(default_path)[1]["memory"], "用户的笔记")
        agents = self.request("/api/agents")[1]["agents"]
        self.assertEqual(len(agents), 2)
        self.assertTrue(all("memory" not in item and "instructions" not in item for item in agents))
        for endpoint, chat, owner in (("/api/conversations", default_chat, default_id),
                                      (f"/api/conversations?agent_id={agent['id']}", path, agent["id"])):
            listed = self.request(endpoint)[1]["conversations"]
            self.assertEqual(listed, [self.request(chat)[1]["conversation"]])
            self.assertEqual(listed[0]["agent_id"], owner)

    def test_agent_migration_preserves_conversation_sequence(self):
        """上一版已删除的会话 ID 在增加归属后也不能复用。"""
        self.stop_app()
        with sqlite3.connect(self.db_path) as db:
            db.executescript("""
                DROP TABLE conversations;
                DROP TABLE agents;
                CREATE TABLE conversations (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
                INSERT INTO conversations (id, title) VALUES (7, '旧会话'), (60, '已删除');
                DELETE FROM conversations WHERE id=60;
                INSERT INTO messages (conversation_id, position, payload)
                VALUES (7, 0, '{"type":"message","role":"user","content":"旧消息"}');
            """)
        self.start_app()
        default_id = self.request("/api/agents")[1]["default_agent_id"]
        old = self.request("/api/conversations/7/messages")[1]
        self.assertEqual(old["conversation"]["agent_id"], default_id)
        self.assertEqual(old["messages"][0]["content"], "旧消息")
        self.assertGreater(self.request("/api/conversations", "POST")[1]["id"], 60)
        with sqlite3.connect(self.db_path) as db:
            self.assertFalse(db.execute("PRAGMA foreign_key_check").fetchall())

    def invoke_tools(self, agent_id, calls, chat=None):
        """让网关点名这一批工具，包括未公布的名称，检查实际发出的定义和逐条执行结果。"""
        state = self.server.state
        start = len(state["requests"])
        def model(req):
            if req["input"][-1].get("role") == "user":
                return [model_call(name, f"{start}-{n}", json.dumps(args))[0]
                        for n, (name, args) in enumerate(calls)]
            return model_text()
        state["model_output"] = model
        status, data = self.request(chat or self.new_conversation(agent_id), "POST", {"message": "执行本轮工具"})
        self.assertEqual(status, 200, data)
        first, last = state["requests"][start:]
        return {t["name"] for t in first["tools"]}, [item["output"] for item in last["input"][-len(calls):]]

    def test_mcp_bindings_and_grants_survive_without_credentials(self):
        base = SERVERS
        state = self.server.state
        config = {"type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                  "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        name = "mcp__db__query"
        def discover(*names):
            state["mcp_tools"] = [{"name": n, "inputSchema": {"type": "object"}} for n in names]
        discover("query")
        status, data = self.request(base, "POST", {**config, "name": "db"})
        self.assertEqual(status, 200, data)
        path, new = self.binding("db")
        self.assertEqual((new["tools"], new["enabled_tools"]), ([name], []))
        names, [result] = self.invoke_tools(1, [(name, {})])
        self.assertEqual(names, {"echo"})
        self.assertIn("error", json.loads(result))
        self.grant_all("db")
        stale = {"revision": new["revision"], "tools": {name: False}}
        self.assertEqual(self.request(path + "/tools", "PATCH", stale)[0], 409)
        self.assertEqual(self.request(path + "/disconnect", "POST")[0], 200)
        _, pending = self.binding("db")
        self.assertFalse(pending["connected"] or pending["can_reconnect"])
        self.assertEqual(pending["enabled_tools"], [name])
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 400)
        # 在活动连接上重启，再用此前的绑定 ID 重新配置。
        self.assertEqual(self.request(path, "PUT", config)[0], 200)
        self.stop_app()
        with sqlite3.connect(self.db_path) as db:
            stored = "\n".join(db.iterdump())
        for secret in (config["url"], "test-token", "test-header", "Authorization"):
            self.assertNotIn(secret, stored)
            self.assertNotIn(secret.encode(), self.db_path.read_bytes())
        self.start_app()
        same_path, pending = self.binding("db")
        self.assertEqual(same_path, path)
        self.assertEqual(pending["tools"], pending["enabled_tools"])
        self.assertFalse(pending["connected"] or pending["can_reconnect"])
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 400)
        discover("query", "export")
        self.assertEqual(self.request(path, "PUT", config)[0], 200)
        _, live = self.binding("db")
        self.assertEqual(live["enabled_tools"], [name])
        self.assertIn("mcp__db__export", live["tools"])
        invalid = {"revision": live["revision"], "tools": {name: False, "missing": True}}
        self.assertEqual(self.request(path + "/tools", "PATCH", invalid)[0], 400)
        self.assertEqual(self.binding("db")[1], live)
        names, [allowed, denied] = self.invoke_tools(1, [(name, {}), ("mcp__db__export", {})])
        self.assertEqual(names, {"echo", name})
        self.assertEqual(allowed, "HTTP-TEST-RESULT")
        self.assertIn("error", json.loads(denied))
        discover("export")
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 200)
        hidden = self.binding("db")[1]
        self.assertEqual(hidden["unavailable_tools"], [name])
        names, [denied] = self.invoke_tools(1, [(name, {})])
        self.assertNotIn(name, names)
        self.assertIn("error", json.loads(denied))
        discover("query", "export")
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 200)
        self.assertEqual(self.binding("db")[1]["enabled_tools"], [name])
        # 两个远端原名映射到同一个模型名称时，替换不能继承授权。
        discover("x.y")
        self.assertEqual(self.request(path, "PUT", config)[0], 200)
        self.grant_all("db")
        discover("x y")
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 200)
        self.assertFalse(self.binding("db")[1]["enabled_tools"])
        self.assertEqual(self.request(path, "DELETE"), (200, {"servers": []}))
        self.assertEqual(self.request(base, "POST", {**config, "name": "db"})[0], 200)
        replacement, server = self.binding("db")
        self.assertNotEqual(replacement, path)
        self.assertEqual(self.request(path, "DELETE")[0], 404)
        self.assertFalse(server["enabled_tools"])

    def test_unchanged_grants_preserve_revision(self):
        self.connect_test_tools({"query": True}, None)
        path, original = self.binding("remote")
        name = original["enabled_tools"][0]

        def patch(revision, changes):
            return self.request(path + "/tools", "PATCH", {"revision": revision, "tools": changes})

        for changes in ({}, {name: True}):
            self.assertEqual(patch(original["revision"], changes), (200, {"servers": [original]}))
        self.assertEqual(patch(original["revision"], {"missing": False})[0], 400)
        status, updated = patch(original["revision"], {name: False})
        self.assertEqual(status, 200)
        rev = updated["servers"][0]["revision"]
        self.assertEqual(rev, original["revision"] + 1)
        self.assertEqual(patch(original["revision"], {})[0], 409)
        self.assertEqual(patch(rev, {name: False})[1], updated)
        self.assertEqual(patch(rev, {name: True})[0], 200)

        self.server.state["mcp_tools"] = []
        status, data = self.request(path + "/reconnect", "POST")
        self.assertEqual(status, 200, data)
        hidden = data["servers"][0]
        self.assertEqual((hidden["tools"], hidden["unavailable_tools"]), ([], [name]))
        self.assertEqual(patch(hidden["revision"], {}), (200, data))
        status, revoked = patch(hidden["revision"], {name: False})
        self.assertEqual(status, 200)
        self.assertEqual(revoked["servers"][0]["unavailable_tools"], [])
        self.assertEqual(revoked["servers"][0]["revision"], hidden["revision"] + 1)

    @unittest.skipUnless(os.environ.get("DOGO_BROWSER_TESTS") == "1", "按需运行 Playwright 浏览器回归")
    def test_agent_panels_in_two_tabs(self):
        self.connect_test_tools({"query": True}, None)
        path, _ = self.binding("remote")
        self.server.state["mcp_tools"] = []
        self.assertEqual(self.request(path + "/reconnect", "POST")[0], 200)
        # 浏览器稍后点重连时，模拟服务会公布这项新工具。
        self.server.state["mcp_tools"] = [{"name": "new_tool", "inputSchema": {"type": "object"}}]
        self.server.state["model_output"] = lambda req: (
            model_call("memory_update", "save-note", '{"memory":"来自工具的笔记"}')
            if req["input"][-1].get("role") == "user" else model_text()
        )
        policy = self.request("/api/agents/1/tools")[1]
        self.assertEqual(self.request("/api/agents/1/tools", "PUT", {**policy, "memory_update": True})[0], 200)
        status, other = self.request("/api/agents", "POST", {"name": "第二位助手", "model": "model"})
        self.assertEqual(status, 200, other)
        subprocess.run(["node", str(ROOT / "tests/agent_panels.cjs"), self.base, str(other["id"])],
                       cwd=ROOT, check=True, timeout=90)
        self.assertEqual(self.request("/api/agents/1")[1]["memory"], "来自工具的笔记")

    def test_agents_can_bind_same_service_name_independently(self):
        b = self.request("/api/agents", "POST", {"name": "B", "model": "model"})[1]["id"]
        base_b = f"/api/agents/{b}/mcp/servers"
        other = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        other.state = {"errors": [], "mcp_token": "second-token"}
        threading.Thread(target=other.serve_forever, daemon=True).start()
        self.addCleanup(other.server_close)
        self.addCleanup(other.shutdown)
        for server, tag, base in [(self.server, "A", SERVERS), (other, "B", base_b)]:
            server.state["call_mcp"] = lambda _, value=tag: {"content": [{"type": "text", "text": value}]}
            cfg = {"name": "db", "type": "http", "url": f"http://127.0.0.1:{server.server_port}/mcp",
                   "headers": {"Authorization": "Bearer " + server.state.get("mcp_token", "test-token"),
                               "X-Test": "test-header"}}
            self.assertEqual(self.request(base, "POST", cfg)[0], 200)
        tool = "mcp__db__demo_code"
        self.grant_all("db")
        self.assertEqual(self.invoke_tools(1, [(tool, {})]), ({"echo", tool}, ["A"]))
        names, [denied] = self.invoke_tools(b, [(tool, {})])
        self.assertEqual(names, {"echo"})
        self.assertIn("error", json.loads(denied))
        self.grant_all("db", base_b)
        self.assertEqual(self.invoke_tools(b, [(tool, {})]), ({"echo", tool}, ["B"]))
        a, binding = self.binding("db")
        self.assertEqual(self.request(base_b + "/" + str(binding["id"]), "DELETE")[0], 404)
        self.assertEqual(self.request(a + "/disconnect", "POST")[0], 200)
        self.assertEqual(self.invoke_tools(b, [(tool, {})]), ({"echo", tool}, ["B"]))
        self.assertFalse(other.state["errors"] or self.server.state["errors"])
        self.assertEqual(self.request("/api/agents/99999/mcp/servers")[0], 404)

    def test_grant_change_between_model_requests(self):
        entered, release = threading.Event(), threading.Event()
        self.connect_test_tools({"run": False}, lambda name: {"content": [{"type": "text", "text": name}]})
        public = "mcp__remote__run"
        def model(req):
            last = req["input"][-1]
            if last.get("role") == "user":
                entered.set()
                release.wait(6)
                return model_call(public, "captured")
            if last.get("call_id") == "captured":
                return model_call(public, "revoked")
            return model_text()
        self.server.state["model_output"] = model
        with ThreadPoolExecutor(max_workers=1) as pool:
            run = pool.submit(self.request, self.new_conversation(), "POST", {"message": "使用快照"})
            try:
                self.assertTrue(entered.wait(5))
                path, server = self.binding("remote")
                self.assertEqual(self.request(path + "/tools", "PATCH", {
                    "revision": server["revision"], "tools": {public: False}})[0], 200)
            finally:
                release.set()
            self.assertEqual(run.result()[0], 200)
        first, second, third = self.server.state["requests"]
        self.assertEqual({t["name"] for t in first["tools"]}, {"echo", public})
        self.assertEqual([t["name"] for t in second["tools"]], ["echo"])
        self.assertEqual(second["input"][-1]["output"], "run")
        self.assertIn("error", json.loads(third["input"][-1]["output"]))

    def test_private_memory_default_permissions_and_concurrent_edit(self):
        b = self.request("/api/agents", "POST", {"name": "B", "model": "model", "memory": "起点"})[1]["id"]
        policy = f"/api/agents/{b}/tools"
        initial = self.request(policy)[1]
        calls = [("memory_update", {"memory": "笔记"})]
        names, [denied] = self.invoke_tools(b, calls)
        self.assertEqual(names, {"echo"})
        self.assertIn("error", json.loads(denied))
        self.assertEqual(self.request(policy, "PUT", {**initial, "echo": False, "memory_update": True})[0], 200)
        self.assertEqual(self.request(policy, "PUT", initial)[0], 409)
        names, results = self.invoke_tools(b, calls + [
            ("echo", {"text": "未授权"}), ("memory_update", {"agent_id": 1, "memory": "误写"}),
            ("memory_update", {"memory": "x" * 4001}), ("memory_update", {"memory": "完成"}),
        ])
        self.assertEqual(names, {"memory_update"})
        self.assertIn("已保存", results[0])
        for output in results[1:4]:
            self.assertIn("error", json.loads(output))
        self.assertIn("已保存", results[4])
        self.assertIn("起点", self.server.state["requests"][-1]["instructions"])
        agent_path = f"/api/agents/{b}"
        before = self.request(agent_path)[1]
        self.assertEqual(before["memory"], "完成")
        self.assertEqual(self.request("/api/agents/1")[1]["memory"], "")
        entered, release = threading.Event(), threading.Event()
        def model(req):
            if req["input"][-1].get("role") == "user":
                entered.set()
                release.wait(6)
                return model_call("memory_update", "compete", '{"memory":"迟到的旧笔记"}')
            return model_text()
        self.server.state["model_output"] = model
        with ThreadPoolExecutor(max_workers=1) as pool:
            run = pool.submit(self.request, self.new_conversation(b), "POST", {"message": "改笔记"})
            try:
                self.assertTrue(entered.wait(5))
                self.assertIn("完成", self.server.state["requests"][-1]["instructions"])
                self.assertEqual(self.request(agent_path, "PUT", {**before, "memory": "手动保存"})[0], 200)
            finally:
                release.set()
            self.assertEqual(run.result()[0], 200)
        self.assertIn("已被", json.loads(self.server.state["requests"][-1]["input"][-1]["output"])["error"])
        self.stop_app()
        self.start_app()
        self.assertEqual(self.request(agent_path)[1]["memory"], "手动保存")
        self.assertTrue(self.request(policy)[1]["memory_update"])

    def test_file_tools_enforce_agent_root_on_execution(self):
        base = Path(self.temp.name)
        a, b = base / "docs", base / "docs-other"
        a.mkdir()
        b.mkdir()
        (a / "sub").mkdir()
        (b / "private.txt").write_text("目录外")
        (a / "alias").symlink_to(b, target_is_directory=True)
        (a / "link").symlink_to(b / "private.txt")
        os.link(b / "private.txt", a / "hard")
        policy = "/api/agents/1/tools"
        initial = self.request(policy)[1]
        for root in (None, "relative", str(a / "missing"), str(b / "private.txt")):
            self.assertEqual(self.request(policy, "PUT", {**initial, "read_file": True, "workspace_root": root})[0], 400)
        settings = {**initial, "workspace_root": str(a), "read_file": True, "write_file": True}
        self.assertEqual(self.request(policy, "PUT", settings)[0], 200)
        calls = [("write_file", {"path": "sub/note", "content": "初稿"}), ("read_file", {"path": "sub/note"}),
                 ("write_file", {"path": "sub/note", "content": "定稿"}), ("read_file", {"path": "sub/note"})]
        bad = ["../docs-other/private.txt", str(b / "private.txt"), "link", "alias/private.txt", "hard",
               "sub/note/", "sub/note/.", "sub/new/", "sub/new/."]
        for path in bad:
            calls.extend([("read_file", {"path": path}), ("write_file", {"path": path, "content": "越界"})])
        names, results = self.invoke_tools(1, calls)
        self.assertEqual(names, {"echo", "read_file", "write_file"})
        self.assertEqual(results[1:4:2], ["初稿", "定稿"])
        for out in results[4:]:
            self.assertIn("error", json.loads(out))
        self.assertEqual((b / "private.txt").read_text(), "目录外")
        self.assertEqual((a / "sub/note").read_text(), "定稿")
        self.assertFalse((a / "sub/new").exists())
        other = self.request("/api/agents", "POST", {"name": "B", "model": "model"})[1]["id"]
        names, [result] = self.invoke_tools(other, [("read_file", {"path": "sub/note"})])
        self.assertEqual(names, {"echo"})
        self.assertIn("error", json.loads(result))
        # 工作目录本身在保存后被替换成链接，旧授权也不能跟随。
        a.rename(base / "moved")
        a.symlink_to(b, target_is_directory=True)
        self.stop_app()
        self.start_app()
        _, [blocked] = self.invoke_tools(1, [("write_file", {"path": "escaped", "content": "错误"})])
        self.assertIn("error", json.loads(blocked))
        self.assertFalse((b / "escaped").exists())
        saved = self.request(policy)[1]
        self.assertEqual(self.request(policy, "PUT", {**saved, "memory_update": True})[0], 400)
        status, disabled = self.request(policy, "PUT", {**saved, "read_file": False, "write_file": False})
        self.assertEqual(status, 200, disabled)
        self.assertEqual(disabled["workspace_root"], saved["workspace_root"])
        self.assertEqual(self.request(policy, "PUT", {**disabled, "write_file": True})[0], 400)

    def test_file_root_is_captured_for_each_model_request(self):
        roots = [Path(self.temp.name) / name for name in ("first", "second")]
        for root in roots:
            root.mkdir()
            (root / "note").write_text(root.name)
        policy = "/api/agents/1/tools"
        initial = self.request(policy)[1]
        status, saved = self.request(policy, "PUT", {**initial, "workspace_root": str(roots[0]), "read_file": True})
        self.assertEqual(status, 200, saved)
        entered, release = threading.Event(), threading.Event()
        def model(req):
            last = req["input"][-1]
            if last.get("role") == "user":
                entered.set()
                release.wait(6)
                return model_call("read_file", "old-root", '{"path":"note"}')
            if last.get("call_id") == "old-root":
                return model_call("read_file", "new-root", '{"path":"note"}')
            return model_text()
        self.server.state["model_output"] = model
        with ThreadPoolExecutor(max_workers=1) as pool:
            run = pool.submit(self.request, self.new_conversation(), "POST", {"message": "两次读取"})
            try:
                self.assertTrue(entered.wait(5))
                self.assertEqual(self.request(policy, "PUT", {**saved, "workspace_root": str(roots[1])})[0], 200)
            finally:
                release.set()
            self.assertEqual(run.result()[0], 200)
        _, second, third = self.server.state["requests"]
        self.assertEqual(second["input"][-1]["output"], "first")
        self.assertEqual(third["input"][-1]["output"], "second")

    # ******************************** 工具并发与执行顺序 ********************************

    def test_parallel_batch_and_exclusive_tool(self):
        """相邻只读工具可以重叠执行，独占工具要等前一组完成。"""
        self.check_parallel_batch(server_parallel=False)

    def test_server_parallel_override(self):
        """服务显式允许并发时，缺少只读提示的工具也可以同时执行。"""
        self.server.state["json_tool_results"] = True
        self.check_parallel_batch(server_parallel=True)

    def check_parallel_batch(self, server_parallel):
        """用事件控制两个工具的完成顺序，检查工具执行的先后和模型收到的结果顺序。"""
        slow_started, fast_finished = threading.Event(), threading.Event()
        events = []

        def call(name):
            events.append(("start", name))
            if name == "slow":
                slow_started.set()
                if not fast_finished.wait(5):
                    self.server.state["errors"].append("第二个工具没有并发执行")
            if name == "fast" and not slow_started.wait(5):
                self.server.state["errors"].append("第一个工具没有并发执行")
            events.append(("end", name))
            if name == "fast":
                fast_finished.set()
            return {"content": [{"type": "text", "text": name}], "isError": name == "fast"}

        hints = ({"slow": False, "fast": None} if server_parallel else
                 {"slow": True, "fast": True, "exclusive": None, "after": True})
        self.connect_test_tools(hints, call, server_parallel=server_parallel)
        # 快工具先以错误结束，模型仍应收到按原 call_id 顺序排列的全部结果。
        self.server.state["results"]["mcp__remote__fast"] = '{"error":"fast"}'
        status, result = self.request(self.new_conversation(), "POST", {"message": "并发调用"})
        self.assertEqual(status, 200, result)
        self.assertLess(events.index(("end", "fast")), events.index(("end", "slow")))
        if not server_parallel:
            self.assertLess(events.index(("end", "slow")), events.index(("start", "exclusive")))
            self.assertLess(events.index(("end", "exclusive")), events.index(("start", "after")))
        self.assertEqual(self.server.state["errors"], [])

    def test_invalid_arguments_keep_their_place_in_batch(self):
        """同批调用两次 echo，一条参数有误时，两条结果仍按各自的调用 ID 回传。"""
        def model(request):
            if request["input"][-1].get("role") == "user":
                return model_call("echo", "bad", "{") + model_call("echo", "good", '{"text":"第二条"}')
            return model_text()

        self.server.state["model_output"] = model
        status, response = self.request(self.new_conversation(), "POST", {"message": "调用两次 echo"})
        self.assertEqual(status, 200, response)
        history = self.server.state["requests"][-1]["input"]
        outputs = [item for item in history if item.get("type") == "function_call_output"]
        self.assertEqual([item["call_id"] for item in outputs], ["bad", "good"])
        self.assertIn("工具参数不是有效 JSON", json.loads(outputs[0]["output"])["error"])
        self.assertEqual(outputs[1]["output"], "第二条")

    # ******************************** 会话排队、限流与失败恢复 ********************************

    def test_concurrent_conversations_and_ordered_turns(self):
        """A 的慢工具阻塞同会话的下一条消息，B 会话仍可完成。"""
        entered, release = threading.Event(), threading.Event()

        def call(name):
            if not entered.is_set():
                entered.set()
                if not release.wait(10):
                    self.server.state["errors"].append("另一段对话被慢工具阻塞")
            return {"content": [{"type": "text", "text": name}]}

        self.connect_test_tools({"exclusive": None}, call)
        a, b = self.new_conversation(), self.new_conversation()
        with ThreadPoolExecutor(max_workers=3) as pool:
            first = pool.submit(self.request, a, "POST", {"message": "A1"})
            try:
                self.assertTrue(entered.wait(5), "A1 没有进入慢工具")
                second = pool.submit(self.request, a, "POST", {"message": "A2"})
                other = pool.submit(self.request, b, "POST", {"message": "B1"})
                self.assertEqual(other.result(timeout=5)[0], 200)
                self.assertFalse(second.done())
                self.assertEqual(self.request(a)[1]["messages"], [])
            finally:
                release.set()
            self.assertEqual(first.result(timeout=5)[0], 200)
            self.assertEqual(second.result(timeout=5)[0], 200)
        history = self.request(a)[1]["messages"]
        self.assertEqual([m["content"] for m in history], ["A1", "工具执行完成", "A2", "工具执行完成"])
        self.assertEqual(self.request(b)[1]["messages"][0]["content"], "B1")
        self.assertEqual(self.server.state["errors"], [])

    def test_queue_survives_closed_tabs_and_refuses_overflow(self):
        """发送方断开后队列仍会处理已受理的消息，全局队列满时新消息得到 503。"""
        self.stop_app()
        self.env["DOGO_MAX_UNFINISHED_TURNS"] = "2"
        self.start_app()
        entered, release = threading.Event(), threading.Event()

        def model(request):
            if request["input"][-1]["content"] == "A1":
                entered.set()
                self.assertTrue(release.wait(8))
            return model_text()

        self.server.state["model_output"] = model
        a, b = self.new_conversation(), self.new_conversation()
        first = self.submit_connection(a, "A1")
        try:
            self.assertTrue(entered.wait(5))
            second = self.submit_connection(a, "A2")
            progress = self.wait_progress(a, lambda p: len(p["turns"]) == 2)
            first.close()
            second.close()
            self.assertEqual([(t["message"], t["status"]) for t in progress["turns"]],
                             [("A1", "running"), ("A2", "queued")])
            snapshot = self.request(a)[1]
            self.assertEqual(snapshot["messages"], [])
            self.assertEqual(snapshot["progress"], progress)
            status, error = self.request(b, "POST", {"message": "B1"})
            self.assertEqual(status, 503)
            self.assertIn("尚未入队", error["error"])
            self.assertEqual(self.request(b)[1]["progress"]["turns"], [])
        finally:
            release.set()
        self.wait_progress(a, lambda p: not p["turns"])
        saved = self.request(a)[1]
        self.assertEqual([m["content"] for m in saved["messages"]], ["A1", "完成", "A2", "完成"])
        self.assertEqual(saved["conversation"]["title"], "A1")
        self.assertEqual(self.server.state["requests"][-1]["input"][0]["content"], "A1")
        self.assertEqual(self.request(b, "POST", {"message": "B1"})[0], 200)

    def test_web_admission_stop_and_rename(self):
        """受理后可继续排队；停止队首和撤回指定排队项不会丢掉后续消息或改名。"""
        entered, release = threading.Event(), threading.Event()

        def model(request):
            if request["input"][-1]["content"] == "慢回复":
                entered.set()
                release.wait(8)
            return model_text()

        self.server.state["model_output"] = model
        path = self.new_conversation()
        base = path.removesuffix("/messages")
        try:
            for text in ("慢回复", "撤回", "保留"):
                self.assertEqual(self.request(base + "/turns", "POST", {"message": text})[0], 202)
            self.assertTrue(entered.wait(3))
            turns = self.request(path)[1]["progress"]["turns"]
            self.assertEqual([t["status"] for t in turns], ["running", "queued", "queued"])
            self.assertEqual(self.request(base, "DELETE")[0], 409)
            self.assertEqual(self.request(base, "PATCH", {"title": "我的标题"})[0], 200)
            for turn in (turns[1], turns[0]):
                self.assertEqual(self.request(f"{base}/turns/{turn['id']}", "DELETE")[0], 204)
            self.wait_progress(path, lambda p: not p["turns"])
            snapshot = self.request(path)[1]
            self.assertEqual(snapshot["conversation"]["title"], "我的标题")
            self.assertEqual([m["content"] for m in snapshot["messages"]], ["保留", "完成"])
            self.assertIsNone(snapshot["progress"]["last_failure"])
        finally:
            release.set()

    def test_stop_while_waiting_for_an_external_tool(self):
        """外部工具已经收到调用时，停止本轮可以立即运行队列下一项。"""
        entered, release = threading.Event(), threading.Event()

        def tool(_name):
            entered.set()
            release.wait(8)
            return {"content": [{"type": "text", "text": "工具结果"}]}

        def model(request):
            if request["input"][-1].get("content") == "慢工具":
                return model_call("mcp__remote__slow", "call_slow")
            return model_text()

        self.connect_test_tools({"slow": None}, tool)
        self.server.state["model_output"] = model
        path = self.new_conversation()
        turns = path.removesuffix("messages") + "turns"
        connection = self.submit_connection(path, "慢工具")
        try:
            self.assertTrue(entered.wait(5))
            turn = self.request(path)[1]["progress"]["turns"][0]
            self.assertEqual(self.request(turns, "POST", {"message": "继续"})[0], 202)
            self.assertEqual(self.request(f"{turns}/{turn['id']}", "DELETE")[0], 204)
            self.assertEqual(connection.getresponse().status, 409)
            self.wait_progress(path, lambda p: not p["turns"])
            self.assertEqual([m["content"] for m in self.request(path)[1]["messages"]], ["继续", "完成"])
        finally:
            release.set()

    def test_delete_after_upgrading_history(self):
        """旧数据库保留消息和关联，删除会话后旧 URL 不会指向后来创建的新会话。"""
        self.stop_app()
        with sqlite3.connect(self.db_path) as db:
            db.executescript("""
                DROP TABLE conversations;
                CREATE TABLE conversations (id INTEGER PRIMARY KEY, title TEXT NOT NULL DEFAULT '新对话',
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
                INSERT INTO conversations (id, title) VALUES (24, '原有标题');
            """)
            db.execute("INSERT INTO messages (conversation_id, position, payload) VALUES (24, 0, ?)",
                       (json.dumps({"role": "user", "content": "历史正文"}),))
        self.start_app()
        base = "/api/conversations/24"
        data = self.request(base + "/messages")[1]
        self.assertEqual(data["messages"][0]["content"], "历史正文")
        self.assertEqual(data["conversation"]["title"], "原有标题")
        with sqlite3.connect(self.db_path) as db:
            self.assertEqual(db.execute("PRAGMA foreign_key_list(messages)").fetchone()[2], "conversations")
            self.assertFalse(db.execute("PRAGMA foreign_key_check").fetchall())
        self.assertEqual(self.request(base, "DELETE")[0], 204)
        self.assertNotEqual(self.new_conversation(), base + "/messages")
        self.assertEqual(self.request(base + "/messages")[0], 404)
        with sqlite3.connect(self.db_path) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM messages WHERE conversation_id=24").fetchone()[0], 0)

    def test_mcp_edit_and_reconnect(self):
        """失败的配置替换保留原服务；重连沿用已有凭据，列表不返回凭据。"""
        config = {"type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                  "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        self.assertEqual(self.request(SERVERS, "POST", {**config, "name": "remote"})[0], 200)
        path, _ = self.binding("remote")
        self.assertEqual(self.request(path, "PUT", {**config, "url": "invalid"})[0], 400)
        status, data = self.request(path + "/reconnect", "POST")
        self.assertEqual(status, 200, data)
        self.assertTrue(data["servers"][0]["connected"])
        self.server.state["mcp_tools"] = [{"name": "edited", "inputSchema": {"type": "object"}}]
        status, data = self.request(path, "PUT", {**config, "supports_parallel_tool_calls": True})
        self.assertEqual(status, 200, data)
        self.assertEqual(data["servers"][0]["tools"], ["mcp__remote__edited"])
        self.assertNotIn("test-token", json.dumps(data))
        self.assertFalse(self.server.state["errors"])

    def test_rejected_mcp_connection_closes_before_returning(self):
        state = self.server.state
        config = {"type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                  "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}

        def discover(*names):
            state["mcp_tools"] = [{"name": name, "inputSchema": {"type": "object"}} for name in names]

        discover("read")
        for name in ("db", "db__other"):
            self.assertEqual(self.request(SERVERS, "POST", {**config, "name": name})[0], 200)
        self.grant_all("db")
        path, original = self.binding("db")
        # 拒绝候选连接的最后一步，验证 SQLite 保存失败也保留原连接。
        with sqlite3.connect(self.db_path) as db:
            db.execute("""CREATE TRIGGER reject_discovery BEFORE UPDATE ON mcp_bindings
                          BEGIN SELECT RAISE(ABORT, 'reject test write'); END""")

        for reason, names, status in [
            ("服务内重名", ["a.b", "a b"], 400),
            ("跨服务重名", ["other__read"], 400),
            ("工具发现失败", [], 400),
            ("绑定写入失败", ["new"], 500),
        ]:
            with self.subTest(reason=reason), ThreadPoolExecutor(max_workers=1) as pool:
                discover(*names)
                state["fail_discovery"] = reason == "工具发现失败"
                closing, release = threading.Event(), threading.Event()

                def on_delete():
                    closing.set()
                    release.wait(5)

                state["on_mcp_delete"] = on_delete
                attempt = pool.submit(self.request, path, "PUT", config)
                try:
                    self.assertTrue(closing.wait(5), "候选 HTTP 会话没有开始关闭")
                    self.assertEqual(self.binding("db")[1], original)
                    self.assertEqual(self.invoke_tools(1, [("mcp__db__read", {})]),
                                     ({"echo", "mcp__db__read"}, ["HTTP-TEST-RESULT"]))
                    self.assertFalse(attempt.done(), "接口在收到关闭回执前已返回")
                finally:
                    release.set()
                    del state["on_mcp_delete"]
                self.assertEqual(attempt.result(timeout=5)[0], status)

    def test_failure_can_be_recovered_by_multiple_tabs(self):
        """多个页面均可恢复失败原文和原因，重新发消息后清除旧错误，上游限流返回 429。"""
        entered, release = threading.Event(), threading.Event()
        def model(request):
            if request["input"][-1]["content"] == "失败原文":
                entered.set()
                self.assertTrue(release.wait(8))
                return []
            return model_text()
        self.server.state["model_output"] = model
        path = self.new_conversation()
        connection = self.submit_connection(path, "失败原文")
        try:
            self.assertTrue(entered.wait(5))
            turn = self.request(path)[1]["progress"]["turns"][0]
            connection.close()
        finally:
            release.set()
        progress = self.wait_progress(path, lambda p: p["last_failure"] is not None)
        failure = progress["last_failure"]
        self.assertEqual((failure["id"], failure["message"]), (turn["id"], "失败原文"))
        self.assertIn("模型没有返回", failure["error"])
        for _ in range(2):
            recovered = self.request(path)[1]
            self.assertEqual(recovered["messages"], [])
            self.assertEqual(recovered["progress"], progress)
        self.assertEqual(self.request(path, "POST", {"message": "重试"})[0], 200)
        self.assertIsNone(self.request(path)[1]["progress"]["last_failure"])
        self.server.state.update(fail_model=True, model_error_status=429)
        status, error = self.request(path, "POST", {"message": "上游限流"})
        self.assertEqual(status, 429)
        self.assertIn("模型服务限流", error["error"])

    def test_model_slot_is_released_while_waiting_for_a_tool(self):
        """只有一个模型名额时，A 等待工具期间，B 已经可以请求模型并完成回复。"""
        self.stop_app()
        self.env["DOGO_MAX_MODEL_CONCURRENCY"] = "1"
        self.start_app()
        first_model, other_model = threading.Event(), threading.Event()
        tool_started, release_model, release_tool = threading.Event(), threading.Event(), threading.Event()
        def model(request):
            last = request["input"][-1]
            if last.get("content") == "A":
                first_model.set()
                self.assertTrue(release_model.wait(8))
                return model_call("mcp__remote__slow", "slow1")
            if last.get("content") == "B":
                other_model.set()
            return model_text()
        def tool(name):
            tool_started.set()
            self.assertTrue(release_tool.wait(8))
            return {"content": [{"type": "text", "text": name}]}
        self.server.state["model_output"] = model
        self.connect_test_tools({"slow": True}, tool)
        a, b = self.new_conversation(), self.new_conversation()
        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(self.request, a, "POST", {"message": "A"})
            try:
                self.assertTrue(first_model.wait(5))
                other = pool.submit(self.request, b, "POST", {"message": "B"})
                self.wait_progress(b, lambda p: bool(p["turns"]))
                self.assertFalse(other_model.wait(0.2))
                release_model.set()
                self.assertTrue(tool_started.wait(5))
                self.assertEqual(other.result(timeout=5)[0], 200)
                self.assertFalse(first.done())
            finally:
                release_model.set()
                release_tool.set()
            self.assertEqual(first.result(timeout=5)[0], 200)

    # ******************************** MCP 重连、断线与动态注册 ********************************

    def test_reconnected_server_is_used_only_for_new_model_requests(self):
        """重连期间产生的旧工具调用留在原连接，下一次模型请求可以看到新工具。"""
        entered, release = threading.Event(), threading.Event()
        invoked = []
        def tool(name):
            invoked.append(name)
            return {"content": [{"type": "text", "text": name}]}
        def model(request):
            outputs = [m for m in request["input"] if m.get("type") == "function_call_output"]
            if not outputs:
                entered.set()
                self.assertTrue(release.wait(8))
                return model_call("mcp__remote__read", "old")
            if len(outputs) == 1:
                return model_call("mcp__remote__read", "new")
            return model_text()
        self.server.state["model_output"] = model
        self.connect_test_tools({"read": True}, tool)
        with ThreadPoolExecutor(max_workers=1) as pool:
            turn = pool.submit(self.request, self.new_conversation(), "POST", {"message": "切换连接"})
            try:
                self.assertTrue(entered.wait(5))
                self.assertEqual(self.request(self.binding("remote")[0], "DELETE")[0], 200)
                self.connect_test_tools({"read": True, "added": True}, tool)
            finally:
                release.set()
            self.assertEqual(turn.result(timeout=5)[0], 200)
        first, second, third = self.server.state["requests"]
        self.assertNotIn("mcp__remote__added", [t["name"] for t in first["tools"]])
        self.assertIn("mcp__remote__added", [t["name"] for t in second["tools"]])
        self.assertIn("未发出", json.loads(second["input"][-1]["output"])["error"])
        self.assertEqual(third["input"][-1]["output"], "read")
        self.assertEqual(invoked, ["read"])

    def test_disconnect_after_tool_receives_call_reports_unknown_result(self):
        """服务收到调用后断开，模型应收到执行结果未知的提示。"""
        entered, release = threading.Event(), threading.Event()
        def tool(name):
            entered.set()
            self.assertTrue(release.wait(8))
            return {"content": [{"type": "text", "text": name}]}
        def model(request):
            if request["input"][-1].get("role") == "user":
                return model_call("mcp__remote__write", "write1")
            return model_text()
        self.server.state["model_output"] = model
        self.connect_test_tools({"write": None}, tool)
        with ThreadPoolExecutor(max_workers=1) as pool:
            turn = pool.submit(self.request, self.new_conversation(), "POST", {"message": "执行工具"})
            try:
                self.assertTrue(entered.wait(5))
                self.assertEqual(self.request(self.binding("remote")[0] + "/disconnect", "POST")[0], 200)
            finally:
                release.set()
            self.assertEqual(turn.result(timeout=5)[0], 200)
        last = self.server.state["requests"][-1]
        self.assertEqual([t["name"] for t in last["tools"]], ["echo"])
        self.assertIn("结果未知", json.loads(last["input"][-1]["output"])["error"])

    def test_web_chat_and_dynamic_mcp(self):
        """从网页接口注册 stdio 和 HTTP 服务，在同一回合调用两者，并检查断开与来源校验。"""
        status, html = self.request("/")
        self.assertEqual(status, 200)
        self.assertIn(b'id="chat-form"', html)
        self.assertIn(b'id="conversation-list"', html)
        self.assertEqual(self.request("/app.js")[0], 200)
        self.assertEqual(self.request("/style.css")[0], 200)
        self.assertEqual(self.request(SERVERS), (200, {"servers": []}))

        wrapper = Path(self.temp.name) / "stdio.py"
        pid_file = Path(self.temp.name) / "stdio.pid"
        wrapper.write_text(
            "import os, runpy\n"
            "assert 'LLM_API_KEY' not in os.environ\n"
            "assert os.environ['DEMO_KEY'] == 'stdio-secret'\n"
            f"open({str(pid_file)!r}, 'w').write(str(os.getpid()))\n"
            f"runpy.run_path({str(ROOT / 'examples/mcp_demo.py')!r}, run_name='__main__')\n",
            encoding="utf-8",
        )
        stdio = {"name": "local", "type": "stdio", "command": sys.executable,
                 "args": [str(wrapper)], "env": {"DEMO_KEY": "stdio-secret"}}
        status, data = self.request(SERVERS, "POST", stdio)
        self.assertEqual(status, 200, data)
        self.assertEqual(data["servers"][0]["tools"], ["mcp__local__demo_code"])
        self.assertNotIn("stdio-secret", json.dumps(data))
        with sqlite3.connect(self.db_path) as db:
            persisted = "\n".join(db.iterdump())
        for transient in (str(wrapper), sys.executable, "DEMO_KEY", "stdio-secret"):
            self.assertNotIn(transient, persisted)

        http = {"name": "remote", "type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        status, data = self.request(SERVERS, "POST", http)
        self.assertEqual(status, 200, data)
        self.assertEqual(len(data["servers"]), 2)
        self.assertNotIn("test-token", json.dumps(data))
        self.assertEqual(self.request(SERVERS, "POST", stdio)[0], 400)
        self.assertEqual(self.request(SERVERS, "POST", {**stdio, "name": "bad name"})[0], 400)
        self.assertEqual(self.request(SERVERS, "POST", {**stdio, "name": "bad", "unknown": "value"})[0], 400)
        self.assertEqual(self.request(SERVERS, "POST", {"name": "bad", "type": "sse", "url": "http://localhost/sse"})[0], 400)
        self.server.state["duplicate_tools"] = True
        status, data = self.request(SERVERS, "POST", {**http, "name": "conflict"})
        self.assertEqual(status, 400, data)
        self.assertIn("mcp__conflict__a_b", data["error"])
        self.assertEqual(len(self.request(SERVERS)[1]["servers"]), 2)
        self.server.state["duplicate_tools"] = False

        state = self.server.state
        self.grant_all("local")
        self.grant_all("remote")
        state["tools"] = ["mcp__local__demo_code", "mcp__remote__demo_code"]
        state["results"] = {"mcp__local__demo_code": "dogo-mcp-7264",
                            "mcp__remote__demo_code": "HTTP-TEST-RESULT"}
        chat = self.new_conversation()
        status, result = self.request(chat, "POST", {"message": "请调用工具"})
        self.assertEqual(status, 200, result)
        self.assertEqual(result["messages"][-1]["content"], "工具执行完成")
        self.assertEqual(self.request(chat)[1]["messages"], [
            {"role": "user", "content": "请调用工具"}, {"role": "assistant", "content": "工具执行完成"}])
        self.assertEqual(self.request(chat, "POST", {"message": "再调用一次"})[0], 200)
        second_turn = state["requests"][-2]["input"]
        self.assertEqual(second_turn[0]["content"], "请调用工具")
        self.assertEqual([part["text"] for part in second_turn[-2]["content"]], ["工具执行", "完成"])
        self.assertEqual(second_turn[-1]["content"], "再调用一次")

        other = self.new_conversation()
        state["fail_model"] = True
        self.assertEqual(self.request(other, "POST", {"message": "失败的请求"})[0], 502)
        failed = self.request(other)[1]
        self.assertEqual(failed["messages"], [])
        self.assertEqual(failed["progress"]["last_failure"]["message"], "失败的请求")
        state["fail_model"] = False

        self.assertEqual(self.request(self.binding("local")[0] + "/disconnect", "POST")[0], 200)
        pid = int(pid_file.read_text())
        for _ in range(30):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.1)
        else:
            self.fail("断开 MCP 后子进程仍在运行")
        state["tools"] = ["mcp__remote__demo_code"]
        self.assertEqual(self.request(other, "POST", {"message": "再次调用"})[0], 200)
        status, data = self.request(SERVERS)
        self.assertEqual({s["name"]: s["connected"] for s in data["servers"]}, {"local": False, "remote": True})
        self.assertEqual(self.request(SERVERS, "POST", stdio,
                                      {"Origin": "http://evil.example"})[0], 403)
        self.assertEqual(self.request(SERVERS, headers={"Host": "evil.example"})[0], 403)
        self.assertEqual(state["errors"], [])

    # ******************************** 会话历史、模型响应与事务 ********************************

    def test_conversations_survive_restart(self):
        """重启后按原顺序回传包含工具调用的完整历史，失败回合不会留下部分消息。"""
        state = self.server.state
        state.update(tools=["echo"], results={"echo": "回声验证"})
        self.assertEqual(self.request("/api/conversations"), (200, {"conversations": []}))
        first, second = self.new_conversation(), self.new_conversation()
        self.assertNotEqual(first, second)
        self.assertEqual(self.request(first, "POST", {"message": "第一段对话"})[0], 200)
        with sqlite3.connect(self.db_path) as db:
            original = [json.loads(row[0]) for row in db.execute("SELECT payload FROM messages ORDER BY position")]
        self.assertEqual([item["type"] for item in original],
                         ["message", "reasoning", "function_call", "function_call_output", "message"])
        self.assertEqual(original[1]["encrypted_content"], "opaque-test-value")
        self.assertEqual(original[2]["call_id"], original[3]["call_id"])
        self.assertNotEqual(original[2]["id"], original[3]["call_id"])
        self.assertEqual(state["requests"][-1]["input"], original[:-1])
        self.assertEqual(self.request(second, "POST", {"message": "独立的话题"})[0], 200)
        self.assertEqual(len(state["requests"][-2]["input"]), 1)

        self.stop_app()
        self.start_app()
        titles = [item["title"] for item in self.request("/api/conversations")[1]["conversations"]]
        self.assertCountEqual(titles, ["第一段对话", "独立的话题"])
        self.assertEqual(self.request(first)[1]["messages"], [
            {"role": "user", "content": "第一段对话"}, {"role": "assistant", "content": "工具执行完成"}])
        self.assertEqual(self.request(second)[1]["messages"][0]["content"], "独立的话题")
        self.assertEqual(self.request(first, "POST", {"message": "接着聊"})[0], 200)
        self.assertEqual(state["requests"][-2]["input"][:-1], original)

        with sqlite3.connect(self.db_path) as db:
            before = self.request(first)[1]["messages"]
            count = db.execute("SELECT count(*) FROM messages").fetchone()[0]
            state["fail_after_tool"] = True
            self.assertEqual(self.request(first, "POST", {"message": "最终回复失败"})[0], 502)
            self.assertEqual(self.request(first)[1]["messages"], before)
            self.assertEqual(db.execute("SELECT count(*) FROM messages").fetchone()[0], count)
        self.assertEqual(self.request("/api/conversations/999/messages")[0], 404)
        self.assertEqual(state["errors"], [])

    def test_responses_text_refusal_and_incomplete_turn(self):
        """正文与拒答均可展示，生成中断或失败时已保存的历史保持完整。"""
        state = self.server.state
        chat = self.new_conversation()
        state["model_response"] = {"status": "completed", "output": [
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "你好"}]}]}
        self.assertEqual(self.request(chat, "POST", {"message": "打个招呼"})[1]["messages"][-1]["content"], "你好")
        state["model_response"]["output"][0]["content"] = [{"type": "refusal", "refusal": "无法完成这个请求"}]
        self.assertEqual(self.request(chat, "POST", {"message": "继续"})[1]["messages"][-1]["content"], "无法完成这个请求")
        before = self.request(chat)[1]["messages"]
        state["model_response"] = {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"},
                                   "output": [{"type": "message", "role": "assistant",
                                               "content": [{"type": "output_text", "text": "部分文字"}]}]}
        status, error = self.request(chat, "POST", {"message": "再问一次"})
        self.assertEqual(status, 502)
        self.assertIn("max_output_tokens", error["error"])
        self.assertEqual(self.request(chat)[1]["messages"], before)
        state["model_response"] = {"status": "failed", "error": {"message": "网关生成失败"}}
        status, error = self.request(chat, "POST", {"message": "重试"})
        self.assertEqual(status, 502)
        self.assertIn("网关生成失败", error["error"])
        self.assertEqual(self.request(chat)[1]["messages"], before)

    def test_resume_saved_tool_calls_from_chat_completions(self):
        """旧库中的工具调用和结果能按 ID 配对还原，续聊时保留原始数据库行。"""
        chat = self.new_conversation()
        conversation_id = int(chat.split("/")[3])
        old_messages = [
            {"role": "user", "content": "原来的提问"},
            {"role": "assistant", "content": None, "tool_calls": [
                {"type": "function", "id": call_id, "function": {"name": "echo", "arguments": '{"text":"ok"}'}}
                for call_id in ("one", "two")]},
            {"role": "tool", "tool_call_id": "one", "content": "ok"},
            {"role": "tool", "tool_call_id": "two", "content": "ok"},
            {"role": "assistant", "content": "原来的回答", "reasoning_content": None},
        ]
        with sqlite3.connect(self.db_path) as db:
            db.executemany("INSERT INTO messages (conversation_id, position, payload) VALUES (?, ?, ?)",
                           [(conversation_id, i, json.dumps(msg)) for i, msg in enumerate(old_messages)])
        self.assertEqual(self.request(chat)[1]["messages"], [
            {"role": "user", "content": "原来的提问"}, {"role": "assistant", "content": "原来的回答"}])
        self.assertEqual(self.request(chat, "POST", {"message": "继续聊"})[0], 200)
        history = self.server.state["requests"][-1]["input"]
        self.assertEqual([(item["type"], item["call_id"]) for item in history if "call_id" in item],
                         [("function_call", "one"), ("function_call", "two"),
                          ("function_call_output", "one"), ("function_call_output", "two")])
        self.assertEqual(history[-2], {"type": "message", "role": "assistant", "content": "原来的回答"})
        with sqlite3.connect(self.db_path) as db:
            rows = db.execute("SELECT position, payload FROM messages WHERE conversation_id = ? ORDER BY position",
                              (conversation_id,)).fetchall()
        self.assertEqual([position for position, _ in rows], list(range(7)))
        self.assertEqual([json.loads(payload) for _, payload in rows[:5]], old_messages)

    def test_incomplete_database_write_rolls_back(self):
        """在一轮写入中途触发 SQLite 错误，验证事务整体回滚并可重新发送。"""
        self.server.state.update(tools=["echo"], results={"echo": "回声验证"})
        chat = self.new_conversation()
        with sqlite3.connect(self.db_path) as db:
            db.execute("""CREATE TRIGGER fail_insert BEFORE INSERT ON messages WHEN NEW.position = 2
                          BEGIN SELECT RAISE(ABORT, 'test write failure'); END""")
        self.assertEqual(self.request(chat, "POST", {"message": "写入中断"})[0], 500)
        with sqlite3.connect(self.db_path) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM messages").fetchone()[0], 0)
            db.execute("DROP TRIGGER fail_insert")
        self.assertEqual(self.request(chat, "POST", {"message": "重试"})[0], 200)
        self.assertEqual(self.request("/api/conversations")[1]["conversations"][0]["title"], "重试")

    # ******************************** 独立 HTTP 演示服务 ********************************

    def test_http_demo(self):
        """启动 examples 中的 HTTP 服务，经由完整对话确认模型收到了演示工具的结果。"""
        demo = subprocess.Popen([sys.executable, str(ROOT / "examples/mcp_http_demo.py"), "0"],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(self.stop_process, demo)
        ready, _, _ = select.select([demo.stdout], [], [], 10)
        self.assertTrue(ready, "HTTP MCP 演示服务未启动")
        url = demo.stdout.readline().strip().split()[-1]
        self.assertTrue(url.startswith("http://127.0.0.1:"), url)
        self.assertEqual(self.request(SERVERS, "POST",
                                      {"name": "remote", "type": "http", "url": url})[0], 200)
        self.grant_all("remote")
        self.server.state["tools"] = ["mcp__remote__demo_code"]
        self.server.state["results"] = {"mcp__remote__demo_code": "dogo-http-7264"}
        self.assertEqual(self.request(self.new_conversation(), "POST", {"message": "调用演示工具"})[0], 200)
        self.assertEqual(self.server.state["errors"], [])

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=3)
        process.stdout.close()
        process.stderr.close()


if __name__ == "__main__":
    unittest.main()
