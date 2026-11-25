"""可删除的本地 Streamable HTTP MCP 演示服务。"""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sys


# ******************************** HTTP 上的 MCP 请求 ********************************

class McpHandler(BaseHTTPRequestHandler):
    """提供握手、工具清单和 demo_code 调用的本地 HTTP 处理器。"""

    def do_POST(self):
        """按请求 ID 返回 JSON-RPC 结果，无 ID 的通知以 HTTP 202 确认接收。"""
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if "id" not in request:
            self.send_response(202)
            self.end_headers()
            return

        method = request["method"]
        if method == "initialize":
            result = {
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "dogo-http-demo", "version": "1.0.0"},
            }
        elif method == "tools/list":
            result = {"tools": [{
                "name": "demo_code",
                "description": "查询 HTTP MCP 测试暗号",
                "annotations": {"readOnlyHint": True},
                "inputSchema": {"type": "object", "properties": {}},
            }]}
        elif method == "tools/call" and request["params"]["name"] == "demo_code":
            print("HTTP MCP demo: demo_code 已执行", flush=True)
            result = {"content": [{"type": "text", "text": "dogo-http-7264"}]}
        else:
            self.send_json({"jsonrpc": "2.0", "id": request["id"],
                            "error": {"code": -32601, "message": "Method not found"}})
            return
        self.send_json({"jsonrpc": "2.0", "id": request["id"], "result": result})

    def do_GET(self):
        """此示例仅通过 POST 回包，GET 请求以 405 表示未提供推送流。"""
        self.send_response(405)
        self.end_headers()

    def send_json(self, body):
        """将 JSON-RPC 回包编码成字节，并据此设置 HTTP 正文长度。"""
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


# ******************************** 本地服务入口 ********************************

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    server = ThreadingHTTPServer(("127.0.0.1", port), McpHandler)
    print(f"HTTP MCP 演示服务： http://127.0.0.1:{server.server_port}/mcp", flush=True)
    server.serve_forever()
