"""可删除的 stdio MCP 测试服务，只提供一个返回固定文字的工具。"""

import json
import sys


def handle(method, params):
    if method == "initialize":
        return {
            "protocolVersion": params["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "dogo-demo", "version": "1.0.0"},
        }
    if method == "tools/list":
        return {
            "tools": [{
                "name": "demo_code",
                "description": "查询本机 MCP 测试暗号",
                "inputSchema": {"type": "object", "properties": {}},
            }]
        }
    if method == "tools/call" and params["name"] == "demo_code":
        print("MCP demo: demo_code 已执行", file=sys.stderr, flush=True)
        return {"content": [{"type": "text", "text": "dogo-mcp-7264"}]}
    if method == "ping":
        return {}
    return None


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    reply = {"jsonrpc": "2.0", "id": request["id"]}
    result = handle(request["method"], request.get("params", {}))
    if result is None:
        reply["error"] = {"code": -32601, "message": "Method not found"}
    else:
        reply["result"] = result
    print(json.dumps(reply), flush=True)
