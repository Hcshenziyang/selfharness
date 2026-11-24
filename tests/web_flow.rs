use std::process::Command;

// 验证网页接口与真实 MCP 连接、Agent Loop 的完整链路。
#[test]
fn web_chat_and_dynamic_mcp() {
    let result = Command::new("python3")
        .arg("tests/web_flow.py")
        .arg(env!("CARGO_BIN_EXE_dogoharness"))
        .output()
        .expect("运行网页集成测试需要 Python 3");
    assert!(
        result.status.success(),
        "网页集成测试失败\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
