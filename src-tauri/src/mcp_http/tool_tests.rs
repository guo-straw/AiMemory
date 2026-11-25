use crate::{
    config::{ServiceInput, SettingsStore, UpdateSettings},
    model_api::{ApiUsage, EmbeddingResult},
    pipeline::Pipeline,
    storage::{MemoryInput, RepositoryInput, Vault},
};
use rmcp::{
    model::CallToolRequestParams, service::RunningService,
    transport::StreamableHttpClientTransport, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

mod acceptance;

struct Client {
    _server: super::LocalHttpServer,
    service: RunningService<RoleClient, ()>,
}

impl Client {
    async fn start(data_dir: &Path, vault: &Path, cwd: &Path) -> Self {
        fs::create_dir_all(data_dir).unwrap();
        fs::write(
            data_dir.join("workspace.json"),
            json!({"root":vault}).to_string(),
        )
        .unwrap();
        let mut server = super::LocalHttpServer::default();
        let status = server
            .start_at(
                "127.0.0.1:0",
                crate::mcp::Options {
                    data_dir: data_dir.into(),
                    cwd: cwd.into(),
                },
            )
            .await
            .unwrap();
        let service = ().serve(StreamableHttpClientTransport::from_uri(status.url)).await.unwrap();
        assert_eq!(
            service
                .peer_info()
                .unwrap()
                .server_info
                .as_ref()
                .unwrap()
                .name,
            "knowtrace"
        );
        Self {
            _server: server,
            service,
        }
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        let result = self
            .service
            .call_tool(
                CallToolRequestParams::new(name.to_string())
                    .with_arguments(arguments.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        serde_json::to_value(result).unwrap()
    }
}

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn embedding_service() -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        assert!(first.starts_with("POST /v1/embeddings "));
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header == "\r\n" {
                break;
            }
            if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["input"],
            "为什么改成轮询"
        );
        requests.fetch_add(1, Ordering::SeqCst);
        let response=json!({"data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":11,"total_tokens":11}}).to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
    });
    (url, count, handle)
}

#[tokio::test]
async fn http_search_defaults_to_current_repository_five_summaries_and_reads_on_demand() {
    let root = fixture();
    let data_dir = root.path().join("app config");
    let vault_root = root.path().join("memory vault");
    let cwd = root.path().join("frontend repository");
    fs::create_dir_all(&cwd).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&cwd)
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args([
            "remote",
            "add",
            "origin",
            "git@github.com:aimemory-test/frontend.git"
        ])
        .output()
        .unwrap()
        .status
        .success());
    let (base_url, requests, http) = embedding_service();
    let mut settings = SettingsStore::load(&data_dir, root.path().join("codex")).unwrap();
    settings
        .update(UpdateSettings {
            chat: ServiceInput {
                provider: None,
                base_url: base_url.clone(),
                model: "unused-chat".into(),
                api_key: None,
            },
            embedding: ServiceInput {
                provider: None,
                base_url: base_url.clone(),
                model: "embedding-fixture".into(),
                api_key: None,
            },
            codex_directory: root.path().join("codex").display().to_string(),
            harness_directory: root.path().join("harness").display().to_string(),
            archive_sources: vec!["codex".into()],
            archive_enabled: false,
            run_in_background: false,
        })
        .unwrap();
    let mut vault = Vault::open(&vault_root).unwrap();
    let frontend = vault
        .add_repository(RepositoryInput {
            name: "frontend".into(),
            remote: "git@github.com:aimemory-test/frontend.git".into(),
        })
        .unwrap();
    let backend = vault
        .add_repository(RepositoryInput {
            name: "backend".into(),
            remote: "git@github.com:aimemory-test/backend.git".into(),
        })
        .unwrap();
    for (repository, count) in [(&frontend, 6), (&backend, 3)] {
        for index in 0..count {
            vault.save(MemoryInput {
                id:None,repository_id:repository.id.clone(),title:format!("{} 轮询决定 {index}",repository.name),kind:"技术决策".into(),tags:Vec::new(),
                markdown:format!("## 结论\n\n采用两秒轮询 {index}。\n\n## 详细说明\n\nPRIVATE_DETAIL_{index}\n\n## 原文依据\n\n> 旧代理会缓冲 SSE。\n"),
                related_ids:Vec::new(),revision:None,source:Some("Codex · 会话 fixture · 轮次 1".into()),
            }).unwrap();
        }
    }
    let mut pipeline = Pipeline::open(&vault_root).unwrap();
    pipeline
        .sync_embeddings(&vault, &format!("{base_url}\nembedding-fixture"), true)
        .unwrap();
    while let Some(job) = pipeline.next_embedding().unwrap() {
        pipeline
            .embedding_complete(
                &job,
                Ok(EmbeddingResult {
                    vector: vec![1.0, 0.1, 0.0],
                    usage: ApiUsage {
                        input_tokens: 1,
                        output_tokens: 0,
                        reported: true,
                    },
                    latency_ms: 1,
                }),
            )
            .unwrap();
    }
    let client = Client::start(&data_dir, &vault_root, &cwd).await;
    let listed = client.service.list_all_tools().await.unwrap();
    let names = listed
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["search_memories", "list_memory_sections", "read_memory"]
    );
    let first = client
        .call("search_memories", json!({"query":"为什么改成轮询"}))
        .await;
    assert_eq!(first["isError"], false);
    let result = &first["structuredContent"];
    assert_eq!(result["results"].as_array().unwrap().len(), 5);
    assert_eq!(result["queryVectorCached"], false);
    assert_eq!(result["embeddingInputTokens"], 11);
    assert!(result["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|hit| hit["repository"] == "frontend"));
    assert!(!result.to_string().contains("PRIVATE_DETAIL"));
    assert!(!result.to_string().contains("旧代理"));
    let id = result["results"][0]["id"].as_str().unwrap();
    let chapters = client.call("list_memory_sections", json!({"id":id})).await;
    assert_eq!(
        chapters["structuredContent"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let conclusion = client.call("read_memory", json!({"id":id})).await;
    assert!(conclusion["structuredContent"]["markdown"]
        .as_str()
        .unwrap()
        .starts_with("## 结论"));
    assert!(!conclusion.to_string().contains("PRIVATE_DETAIL"));
    let evidence = client
        .call("read_memory", json!({"id":id,"mode":"evidence"}))
        .await;
    assert_eq!(
        evidence["structuredContent"]["markdown"]
            .as_str()
            .unwrap()
            .trim_end(),
        "## 原文依据\n\n> 旧代理会缓冲 SSE。"
    );
    let full = client
        .call("read_memory", json!({"id":id,"mode":"full"}))
        .await;
    assert!(full["structuredContent"]["markdown"]
        .as_str()
        .unwrap()
        .contains("PRIVATE_DETAIL"));
    let cached = client
        .call(
            "search_memories",
            json!({"query":"为什么改成轮询","repository":"backend"}),
        )
        .await;
    assert_eq!(
        cached["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(cached["structuredContent"]["queryVectorCached"], true);
    assert_eq!(cached["structuredContent"]["embeddingInputTokens"], 0);
    let missing = client
        .call("read_memory", json!({"id":"not-found","mode":"full"}))
        .await;
    assert_eq!(missing["isError"], true);
    let unresolved = client
        .call(
            "search_memories",
            json!({"query":"为什么改成轮询","cwd":root.path().display().to_string()}),
        )
        .await;
    assert_eq!(unresolved["isError"], true);
    assert_eq!(
        unresolved["structuredContent"]["repositories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    http.join().unwrap();
}

#[tokio::test]
async fn http_handshake_returns_server_and_tool_metadata() {
    let root = fixture();
    let client = Client::start(root.path(), root.path(), root.path()).await;
    let info = client.service.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "knowtrace");
    assert_eq!(
        info.server_info.as_ref().unwrap().version,
        env!("CARGO_PKG_VERSION")
    );
    let listed = client.service.list_all_tools().await.unwrap();
    assert_eq!(listed.len(), 3);
    for tool in listed {
        let tool = serde_json::to_value(tool).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert!(tool["description"].as_str().unwrap().len() > 0);
    }
}

#[tokio::test]
async fn http_offline_groups_manual_conclusions_and_paged_source_evidence_share_the_real_vault() {
    use crate::{
        model_api::{ArchiveOp, ExtractedMemory},
        storage::ArchiveSource,
    };
    let root = fixture();
    let data_dir = root.path().join("config");
    let vault_root = root.path().join("vault");
    let mut vault = Vault::open(&vault_root).unwrap();
    let repo = vault
        .add_repository(RepositoryInput {
            name: "offline".into(),
            remote: "https://github.com/aimemory-tests/offline.git".into(),
        })
        .unwrap();
    let source = |n: usize| ArchiveSource {
        source_id: format!("source-{n}"),
        session_id: format!("session-{n}"),
        turn_id: format!("turn-{n}"),
        input_truncated: false,
        message_role: Some("user".into()),
    };
    let input = |title: &str, body: &str| MemoryInput {
        id: None,
        repository_id: repo.id.clone(),
        title: title.into(),
        kind: "技术决策".into(),
        tags: Vec::new(),
        markdown: body.into(),
        related_ids: Vec::new(),
        revision: None,
        source: None,
    };
    let first = vault
        .save_archive(
            "ARC-http-current",
            input(
                "导出方式",
                "## 结论\n\n采用 SSE。\n\n## 原文依据\n\n> 最初采用事件流。\n",
            ),
            &source(1),
            &[],
        )
        .unwrap();
    let raw = fs::read_to_string(&first.summary.path)
        .unwrap()
        .replace("采用 SSE。", "采用两秒轮询。");
    fs::write(&first.summary.path, raw).unwrap();
    vault.refresh().unwrap();
    let change = ExtractedMemory {
        op: Some(ArchiveOp::Update),
        target_id: Some(first.summary.id.clone()),
        title: "导出方式".into(),
        kind: "技术决策".into(),
        summary: "终态时停止轮询。".into(),
        details: String::new(),
        evidence: "终态时停止。\n离开页面也停止。".into(),
        tags: Vec::new(),
        ..Default::default()
    };
    vault
        .extend_archive(&first.summary.id, &repo.id, &change, &source(2), None)
        .unwrap();
    for n in 0..7 {
        vault
            .save_archive(
                &format!("ARC-http-dup-{n}"),
                input(
                    "共享决定",
                    "## 结论\n\n共享决定。\n\n## 原文依据\n\n> 另一轮独立证据。\n",
                ),
                &source(n + 10),
                &[],
            )
            .unwrap();
    }
    let legacy = vault
        .save(input("手写文件", "# 手写文件\n\n手工正文摘录。\n"))
        .unwrap();
    let client = Client::start(&data_dir, &vault_root, root.path()).await;
    let local = client
        .call(
            "search_memories",
            json!({"cwd":root.path(),"repository":repo.id,"query":"SSE","mode":"keyword"}),
        )
        .await;
    let result = &local["structuredContent"];
    assert_eq!(result["searchMode"], "keyword");
    assert_eq!(result["remoteRequests"], 0);
    assert!(result.get("fallbackReason").is_none());
    assert_eq!(result["results"][0]["summary"], "采用两秒轮询。");
    assert_eq!(result["results"][0]["matchesHistoryOnly"], true);
    let pending = client
        .call(
            "search_memories",
            json!({"cwd":root.path(),"repository":repo.id,"query":"共享决定"}),
        )
        .await;
    assert_eq!(
        pending["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(pending["structuredContent"]["results"][0]["groupCount"], 7);
    assert_eq!(pending["structuredContent"]["pendingEmbeddings"], 9);
    assert!(pending["structuredContent"]["fallbackReason"]
        .as_str()
        .unwrap()
        .contains("尚无可用向量"));
    let group = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"groupOf":"ARC-http-dup-3","offset":5,"limit":2}),
        )
        .await;
    assert_eq!(group["structuredContent"]["total"], 7);
    assert_eq!(group["structuredContent"]["hasMore"], false);
    assert_eq!(
        group["structuredContent"]["results"][0]["id"],
        "ARC-http-dup-5"
    );
    let exact = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"ARC-http-dup-6"}),
        )
        .await;
    assert_eq!(
        exact["structuredContent"]["results"][0]["id"],
        "ARC-http-dup-6"
    );
    let missing = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"ARC-no-such-id","mode":"id"}),
        )
        .await;
    assert!(missing["structuredContent"]["results"]
        .as_array()
        .unwrap()
        .is_empty());
    let headings = client
        .call("list_memory_sections", json!({"id":first.summary.id}))
        .await;
    assert!(headings["structuredContent"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["title"] == "原文依据" && s["sourceCount"] == 2 && s["expandable"] == true));
    let directory = client
        .call(
            "list_memory_sections",
            json!({"id":first.summary.id,"section":"原文依据","offset":1,"limit":1}),
        )
        .await;
    assert_eq!(
        directory["structuredContent"]["sources"][0]["sourceId"],
        "source-2"
    );
    assert!(directory.to_string().find("离开页面").is_none());
    let quote = client
        .call(
            "read_memory",
            json!({"id":first.summary.id,"mode":"evidence","sourceId":"source-2"}),
        )
        .await;
    assert_eq!(quote["structuredContent"]["markdown"], change.evidence);
    assert_eq!(
        quote["structuredContent"]["evidence"]["sessionId"],
        "session-2"
    );
    let unresolved = client
        .call(
            "read_memory",
            json!({"id":first.summary.id,"mode":"evidence","sourceId":"unknown"}),
        )
        .await;
    assert_eq!(
        unresolved["structuredContent"]["evidence"]["status"],
        "unresolved"
    );
    assert!(unresolved["structuredContent"]["message"]
        .as_str()
        .unwrap()
        .contains("无法在当前文件中精确定位"));
    let current = client
        .call("read_memory", json!({"id":first.summary.id}))
        .await;
    assert_eq!(current["structuredContent"]["conclusionSource"], "manual");
    assert!(current["structuredContent"]["markdown"]
        .as_str()
        .unwrap()
        .contains("两秒轮询"));
    assert!(!current.to_string().contains("终态时停止轮询"));
    let excerpt = client
        .call("read_memory", json!({"id":legacy.summary.id}))
        .await;
    assert_eq!(
        excerpt["structuredContent"]["conclusionSource"],
        "body_excerpt"
    );
    assert_eq!(excerpt["structuredContent"]["markdown"], "手工正文摘录。");
    let invalid = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"共享决定","groupOf":"ARC-http-dup-3"}),
        )
        .await;
    assert_eq!(invalid["isError"], true);
    let mut pipeline = Pipeline::open(&vault_root).unwrap();
    let mut settings = SettingsStore::load(&data_dir, root.path().join("codex")).unwrap();
    let key = crate::archive::embedding_key(&settings.config().embedding);
    pipeline.sync_embeddings(&vault, &key, true).unwrap();
    let job = pipeline.next_embedding().unwrap().unwrap();
    pipeline
        .embedding_complete(
            &job,
            Ok(EmbeddingResult {
                vector: vec![1.0, 0.0, 0.0],
                usage: ApiUsage {
                    input_tokens: 1,
                    output_tokens: 0,
                    reported: true,
                },
                latency_ms: 1,
            }),
        )
        .unwrap();
    let no_key = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"共享决定"}),
        )
        .await;
    assert_eq!(no_key["structuredContent"]["remoteRequests"], 0);
    assert_eq!(no_key["structuredContent"]["pendingEmbeddings"], 8);
    assert!(no_key["structuredContent"]["fallbackReason"]
        .as_str()
        .unwrap()
        .contains("凭据"));
    let stopped = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", stopped.local_addr().unwrap());
    drop(stopped);
    settings
        .update(UpdateSettings {
            chat: ServiceInput {
                provider: None,
                base_url: endpoint.clone(),
                model: "unused".into(),
                api_key: None,
            },
            embedding: ServiceInput {
                provider: None,
                base_url: endpoint,
                model: "stopped-service".into(),
                api_key: None,
            },
            codex_directory: root.path().join("codex").display().to_string(),
            harness_directory: root.path().join("harness").display().to_string(),
            archive_sources: vec!["codex".into()],
            archive_enabled: false,
            run_in_background: false,
        })
        .unwrap();
    pipeline
        .sync_embeddings(
            &vault,
            &crate::archive::embedding_key(&settings.config().embedding),
            true,
        )
        .unwrap();
    let job = pipeline.next_embedding().unwrap().unwrap();
    pipeline
        .embedding_complete(
            &job,
            Ok(EmbeddingResult {
                vector: vec![1.0, 0.0, 0.0],
                usage: ApiUsage {
                    input_tokens: 1,
                    output_tokens: 0,
                    reported: true,
                },
                latency_ms: 1,
            }),
        )
        .unwrap();
    let disconnected = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"共享决定"}),
        )
        .await;
    assert_eq!(disconnected["isError"], false);
    assert_eq!(disconnected["structuredContent"]["searchMode"], "keyword");
    assert_eq!(disconnected["structuredContent"]["remoteRequests"], 1);
    assert_eq!(
        disconnected["structuredContent"]["results"][0]["groupCount"],
        7
    );
    assert_eq!(
        disconnected["structuredContent"]["queryUsageReported"],
        false
    );
    let local = client
        .call(
            "search_memories",
            json!({"repository":repo.id,"query":"共享决定","mode":"keyword"}),
        )
        .await;
    assert_eq!(local["structuredContent"]["remoteRequests"], 0);
}
