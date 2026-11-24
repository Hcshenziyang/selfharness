"""可删除的本地 Streamable HTTP MCP 演示服务。"""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sys


class McpHandler(BaseHTTPRequestHandler):
    def do_POST(self):
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
        # 这个演示服务不提供服务器主动推送。
        self.send_response(405)
        self.end_headers()

    def send_json(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    server = ThreadingHTTPServer(("127.0.0.1", port), McpHandler)
    print(f"HTTP MCP 演示服务： http://127.0.0.1:{server.server_port}/mcp", flush=True)
    server.serve_forever()
