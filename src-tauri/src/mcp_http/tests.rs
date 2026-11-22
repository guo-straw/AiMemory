use super::*;
use crate::storage::{MemoryInput, RepositoryInput, Vault};
use rmcp::{model::CallToolRequestParams, transport::StreamableHttpClientTransport, ServiceExt};
use serde_json::json;
use std::{fs, path::Path, time::Duration};

fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn options(directory: &Path) -> Options {
    Options {
        data_dir: directory.into(),
        cwd: directory.into(),
    }
}

fn save_memory(root: &Path, text: &str) -> String {
    let mut vault = Vault::open(root).unwrap();
    let repository = vault
        .add_repository(RepositoryInput {
            name: "http-test".into(),
            remote: "https://github.com/aimemory-test/http-test.git".into(),
        })
        .unwrap();
    vault
        .save(MemoryInput {
            id: None,
            repository_id: repository.id,
            title: "应用内 MCP".into(),
            kind: "功能修改".into(),
            tags: vec![],
            markdown: format!("## 结论\n\n{text}\n"),
            related_ids: vec![],
            revision: None,
            source: None,
        })
        .unwrap()
        .summary
        .id
}

#[tokio::test]
async fn http_clients_use_real_tools_and_follow_the_current_vault() {
    let directory = fixture();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    let id = save_memory(&first, "第一份记忆库");
    let second_id = save_memory(&second, "已切换到第二份记忆库");
    let workspace = directory.path().join("workspace.json");
    fs::write(&workspace, json!({"root":first}).to_string()).unwrap();
    let mut server = LocalHttpServer::default();
    let status = server
        .start_at("127.0.0.1:0", options(directory.path()))
        .await
        .unwrap();
    assert!(status.running);
    assert_eq!(
        server
            .start_at("127.0.0.1:0", options(directory.path()))
            .await
            .unwrap()
            .url,
        status.url
    );
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(status.url.clone()))
            .await
            .unwrap();
    let other =
        ().serve(StreamableHttpClientTransport::from_uri(status.url.clone()))
            .await
            .unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["search_memories", "list_memory_sections", "read_memory"]
    );
    for (root, memory_id, expected) in [
        (&first, &id, "第一份记忆库"),
        (&second, &second_id, "已切换到第二份记忆库"),
    ] {
        fs::write(&workspace, json!({"root":root}).to_string()).unwrap();
        let response = client
            .call_tool(
                CallToolRequestParams::new("read_memory")
                    .with_arguments(json!({"id":memory_id}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert!(serde_json::to_string(&response).unwrap().contains(expected));
    }
    let sections = other
        .call_tool(
            CallToolRequestParams::new("list_memory_sections")
                .with_arguments(json!({"id":second_id}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(serde_json::to_string(&sections).unwrap().contains("结论"));
    let stopped = tokio::time::timeout(Duration::from_secs(3), server.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(!stopped.running);
    assert!(!server.stop().await.unwrap().running);
    assert!(reqwest::get(&status.url).await.is_err());
    let restarted = server
        .start_at(
            status
                .url
                .trim_start_matches("http://")
                .trim_end_matches("/mcp"),
            options(directory.path()),
        )
        .await
        .unwrap();
    assert_eq!(restarted.url, status.url);
    server.stop().await.unwrap();
    let _ = client.cancel().await;
    let _ = other.cancel().await;
}

#[tokio::test]
async fn occupied_port_returns_an_error_without_marking_service_running() {
    let directory = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut server = LocalHttpServer::default();
    let error = server
        .start_at(
            &listener.local_addr().unwrap().to_string(),
            options(directory.path()),
        )
        .await
        .err()
        .unwrap();
    assert!(error.contains("无法启动本地 MCP 服务"));
    assert!(!server.status().await.unwrap().running);
}

#[tokio::test]
async fn http_transport_validates_host_and_origin_at_the_request_boundary() {
    let directory = fixture();
    let mut server = LocalHttpServer::default();
    let status = server
        .start_at("127.0.0.1:0", options(directory.path()))
        .await
        .unwrap();
    let client = reqwest::Client::new();
    for (header, value) in [
        ("Host", "unrelated.example"),
        ("Origin", "https://unrelated.example"),
    ] {
        let response = client
            .post(&status.url)
            .header(header, value)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    }
    server.stop().await.unwrap();
}
