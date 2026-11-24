"""模拟模型和 HTTP MCP，测试网页接口实际连接和工具调用。"""

from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen
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


class Handler(BaseHTTPRequestHandler):
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
        self.reply(204)

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/v1/responses":
            self.model(request)
        elif self.path == "/mcp":
            self.mcp(request)
        else:
            self.reply(404)

    def model(self, request):
        state = self.server.state
        state["requests"].append(request)
        history = request.get("input")
        tools = request.get("tools", [])
        if (self.headers.get("Authorization") != "Bearer model-secret" or request.get("model") != "model"
                or request.get("store") is not False or request.get("stream") is not False
                or request.get("parallel_tool_calls") is not True
                or not isinstance(history, list) or not history or "messages" in request or "thinking" in request
                or any(tool.get("type") != "function" or "function" in tool or "parameters" not in tool
                       for tool in tools)):
            state["errors"].append("Responses 请求格式或认证错误")
            self.reply(400)
            return
        if state["fail_model"] or (state.get("fail_after_tool") and history[-1].get("type") == "function_call_output"):
            self.reply(500)
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
        if self.headers.get("Authorization") != "Bearer test-token" or self.headers.get("X-Test") != "test-header":
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


class WebIntegration(unittest.TestCase):
    def setUp(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.state = {"tools": [], "results": {}, "errors": [], "requests": [], "fail_model": False}
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.temp = tempfile.TemporaryDirectory(prefix="dogo-web-test-")
        self.db_path = Path(self.temp.name) / "history.sqlite3"
        self.env = dict(os.environ, LLM_API_KEY="model-secret", LLM_MODEL="model", DOGO_WEB_PORT="0",
                        DOGO_DB_PATH=str(self.db_path),
                        LLM_API_URL=f"http://127.0.0.1:{self.server.server_port}/v1/responses",
                        NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
        self.process = None
        self.addCleanup(self.cleanup)
        self.start_app()

    def start_app(self):
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

    def new_conversation(self):
        status, info = self.request("/api/conversations", "POST")
        self.assertEqual(status, 200, info)
        self.assertEqual(info["title"], "新对话")
        return f"/api/conversations/{info['id']}/messages"

    def connect_test_tools(self, hints, callback, *, server_parallel=False):
        state = self.server.state
        state["mcp_tools"] = [{"name": name, "inputSchema": {"type": "object"},
                               **({"annotations": {"readOnlyHint": hint}} if hint is not None else {})}
                              for name, hint in hints.items()]
        state["call_mcp"] = callback
        config = {"name": "remote", "type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                  "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        if server_parallel:
            config["supports_parallel_tool_calls"] = True
        status, response = self.request("/api/mcp/servers", "POST", config)
        self.assertEqual(status, 200, response)
        state["tools"] = [f"mcp__remote__{name}" for name in hints]
        state["results"] = {f"mcp__remote__{name}": name for name in hints}

    def test_parallel_batch_and_exclusive_tool(self):
        self.check_parallel_batch(server_parallel=False)

    def test_server_parallel_override(self):
        self.server.state["json_tool_results"] = True
        self.check_parallel_batch(server_parallel=True)

    def check_parallel_batch(self, server_parallel):
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

    def test_concurrent_conversations_and_ordered_turns(self):
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

    def test_web_chat_and_dynamic_mcp(self):
        status, html = self.request("/")
        self.assertEqual(status, 200)
        self.assertIn(b'id="chat-form"', html)
        self.assertIn(b'id="conversation-list"', html)
        self.assertEqual(self.request("/app.js")[0], 200)
        self.assertEqual(self.request("/style.css")[0], 200)
        self.assertEqual(self.request("/api/mcp/servers"), (200, {"servers": []}))

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
        status, data = self.request("/api/mcp/servers", "POST", stdio)
        self.assertEqual(status, 200, data)
        self.assertEqual(data["servers"][0]["tools"], ["mcp__local__demo_code"])
        self.assertNotIn("stdio-secret", json.dumps(data))

        http = {"name": "remote", "type": "http", "url": f"http://127.0.0.1:{self.server.server_port}/mcp",
                "headers": {"Authorization": "Bearer test-token", "X-Test": "test-header"}}
        status, data = self.request("/api/mcp/servers", "POST", http)
        self.assertEqual(status, 200, data)
        self.assertEqual(len(data["servers"]), 2)
        self.assertNotIn("test-token", json.dumps(data))
        self.assertEqual(self.request("/api/mcp/servers", "POST", stdio)[0], 400)
        self.assertEqual(self.request("/api/mcp/servers", "POST", {**stdio, "name": "bad name"})[0], 400)
        self.assertEqual(self.request("/api/mcp/servers", "POST", {**stdio, "name": "bad", "unknown": "value"})[0], 400)
        self.assertEqual(self.request("/api/mcp/servers", "POST", {"name": "bad", "type": "sse", "url": "http://localhost/sse"})[0], 400)
        self.server.state["duplicate_tools"] = True
        status, data = self.request("/api/mcp/servers", "POST", {**http, "name": "conflict"})
        self.assertEqual(status, 400, data)
        self.assertIn("mcp__conflict__a_b", data["error"])
        self.assertEqual(len(self.request("/api/mcp/servers")[1]["servers"]), 2)
        self.server.state["duplicate_tools"] = False

        state = self.server.state
        state["tools"] = ["mcp__local__demo_code", "mcp__remote__demo_code"]
        state["results"] = {"mcp__local__demo_code": "dogo-mcp-7264",
                            "mcp__remote__demo_code": "HTTP-TEST-RESULT"}
        chat = self.new_conversation()
        status, result = self.request(chat, "POST", {"message": "请调用工具"})
        self.assertEqual(status, 200, result)
        self.assertEqual(result["messages"][-1]["content"], "工具执行完成")
        self.assertEqual(self.request(chat), (200, {"messages": [
            {"role": "user", "content": "请调用工具"}, {"role": "assistant", "content": "工具执行完成"}]}))
        self.assertEqual(self.request(chat, "POST", {"message": "再调用一次"})[0], 200)
        second_turn = state["requests"][-2]["input"]
        self.assertEqual(second_turn[0]["content"], "请调用工具")
        self.assertEqual([part["text"] for part in second_turn[-2]["content"]], ["工具执行", "完成"])
        self.assertEqual(second_turn[-1]["content"], "再调用一次")

        other = self.new_conversation()
        state["fail_model"] = True
        self.assertEqual(self.request(other, "POST", {"message": "失败的请求"})[0], 502)
        self.assertEqual(self.request(other), (200, {"messages": []}))
        state["fail_model"] = False

        self.assertEqual(self.request("/api/mcp/servers/local", "DELETE")[0], 200)
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
        status, data = self.request("/api/mcp/servers")
        self.assertEqual([server["name"] for server in data["servers"]], ["remote"])
        self.assertEqual(self.request("/api/mcp/servers", "POST", stdio,
                                      {"Origin": "http://evil.example"})[0], 403)
        self.assertEqual(self.request("/api/mcp/servers", headers={"Host": "evil.example"})[0], 403)
        self.assertEqual(state["errors"], [])

    def test_conversations_survive_restart(self):
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
            before = self.request(first)
            count = db.execute("SELECT count(*) FROM messages").fetchone()[0]
            state["fail_after_tool"] = True
            self.assertEqual(self.request(first, "POST", {"message": "最终回复失败"})[0], 502)
            self.assertEqual(self.request(first), before)
            self.assertEqual(db.execute("SELECT count(*) FROM messages").fetchone()[0], count)
        self.assertEqual(self.request("/api/conversations/999/messages")[0], 404)
        self.assertEqual(state["errors"], [])

    def test_responses_text_refusal_and_incomplete_turn(self):
        state = self.server.state
        chat = self.new_conversation()
        state["model_response"] = {"status": "completed", "output": [
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "你好"}]}]}
        self.assertEqual(self.request(chat, "POST", {"message": "打个招呼"})[1]["messages"][-1]["content"], "你好")
        state["model_response"]["output"][0]["content"] = [{"type": "refusal", "refusal": "无法完成这个请求"}]
        self.assertEqual(self.request(chat, "POST", {"message": "继续"})[1]["messages"][-1]["content"], "无法完成这个请求")
        before = self.request(chat)
        state["model_response"] = {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"},
                                   "output": [{"type": "message", "role": "assistant",
                                               "content": [{"type": "output_text", "text": "部分文字"}]}]}
        status, error = self.request(chat, "POST", {"message": "再问一次"})
        self.assertEqual(status, 502)
        self.assertIn("max_output_tokens", error["error"])
        self.assertEqual(self.request(chat), before)
        state["model_response"] = {"status": "failed", "error": {"message": "网关生成失败"}}
        status, error = self.request(chat, "POST", {"message": "重试"})
        self.assertEqual(status, 502)
        self.assertIn("网关生成失败", error["error"])
        self.assertEqual(self.request(chat), before)

    def test_resume_saved_tool_calls_from_chat_completions(self):
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

    def test_http_demo(self):
        demo = subprocess.Popen([sys.executable, str(ROOT / "examples/mcp_http_demo.py"), "0"],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(self.stop_process, demo)
        ready, _, _ = select.select([demo.stdout], [], [], 10)
        self.assertTrue(ready, "HTTP MCP 演示服务未启动")
        url = demo.stdout.readline().strip().split()[-1]
        self.assertTrue(url.startswith("http://127.0.0.1:"), url)
        self.assertEqual(self.request("/api/mcp/servers", "POST",
                                      {"name": "remote", "type": "http", "url": url})[0], 200)
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
