use agentlink_daemon::{config::AgentLinkHome, start_daemon};
use agentlink_mcp::handle_rpc_line;
use serde_json::{json, Value};
use tempfile::TempDir;

#[tokio::test]
async fn mcp_initialize_tools_list_and_local_call() {
    let dir = TempDir::new().unwrap();
    let home = AgentLinkHome::from_path(dir.path());
    home.init("me", Some("http://127.0.0.1:9"), Some(0)).unwrap();

    let daemon = start_daemon(&home, Some(0)).await.unwrap();
    // Write actual ephemeral port back into config.json so McpClient finds it
    let mut cfg = home.load_config().unwrap();
    cfg.listen_port = daemon.addr.port();
    std::fs::write(
        home.config_path(),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    // 1. initialize
    let init_req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
    let init_resp: Value = serde_json::from_str(
        &handle_rpc_line(&init_req.to_string(), &home)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "agentlink");

    // 2. tools/list
    let list_req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    let list_resp: Value = serde_json::from_str(
        &handle_rpc_line(&list_req.to_string(), &home)
            .await
            .unwrap(),
    )
    .unwrap();
    let tools = list_resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);

    // 3. tools/call -> list_peers
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "list_peers", "arguments": {} }
    });
    let call_resp: Value = serde_json::from_str(
        &handle_rpc_line(&call_req.to_string(), &home)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(call_resp["result"]["isError"].is_null());
    let text = call_resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"remote\""));
    assert!(text.contains("\"local\""));

    let _ = daemon.shutdown.send(true);
}

