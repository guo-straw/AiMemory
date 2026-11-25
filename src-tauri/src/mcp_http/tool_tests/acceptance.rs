use super::*;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{model::*, service::RequestContext, ErrorData, RoleServer, ServerHandler};
use std::{sync::Mutex, time::Instant};
use tokio_util::sync::CancellationToken;

#[path = "holdout.rs"]
mod holdout;

#[allow(dead_code)]
#[path = "../../../tests/fixtures/optimization-baseline/search.rs"]
mod baseline_search;
#[allow(dead_code)]
#[path = "../../../tests/fixtures/optimization-baseline/storage.rs"]
mod baseline_storage;

#[derive(Clone)]
struct BaselineServer {
    root: PathBuf,
    vault: Arc<Mutex<Option<baseline_storage::Vault>>>,
    engine: Arc<baseline_search::SearchEngine>,
    service: crate::config::ServiceCredentials,
}

impl ServerHandler for BaselineServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aimemory-baseline", "0.2.2"))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Value::Object(request.arguments.unwrap_or_default());
        {
            let mut vault = self.vault.lock().unwrap();
            if vault.is_none() {
                *vault = Some(baseline_storage::Vault::open(&self.root).unwrap());
            }
        }
        let response = self
            .engine
            .search(
                baseline_search::SearchRequest {
                    query: args["query"].as_str().unwrap().into(),
                    repository_id: args["repository"].as_str().map(str::to_owned),
                    limit: Some(5),
                },
                &self.service,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        Ok(CallToolResult::structured(json!({"results":response.results.iter().map(|h|json!({"id":h.memory.id,"summary":h.memory.summary,"score":h.score})).collect::<Vec<_>>(),"localSearchMs":response.local_search_ms,"queryVectorCached":response.query_vector_cached,"embeddingInputTokens":response.embedding_input_tokens})).into())
    }
}

struct BaselineClient {
    service: RunningService<RoleClient, ()>,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    url: String,
}

impl BaselineClient {
    async fn start(root: &Path, credentials: crate::config::ServiceCredentials) -> Self {
        let memory = BaselineServer {
            root: root.into(),
            vault: Arc::new(Mutex::new(None)),
            engine: Arc::new(baseline_search::SearchEngine::open(root).unwrap()),
            service: credentials,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = CancellationToken::new();
        let http = StreamableHttpService::new(
            move || Ok(memory.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_cancellation_token(cancellation.child_token()),
        );
        let router = axum::Router::new().nest_service("/mcp", http);
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        let url = format!("http://{address}/mcp");
        let service = ().serve(StreamableHttpClientTransport::from_uri(url.clone())).await.unwrap();
        Self {
            service,
            cancellation,
            task,
            url,
        }
    }
    async fn stop(self) {
        self.service.cancel().await.unwrap();
        self.cancellation.cancel();
        self.task.await.unwrap().unwrap();
    }
}

async fn call(service: &RunningService<RoleClient, ()>, arguments: Value) -> Value {
    let result = service
        .call_tool(
            CallToolRequestParams::new("search_memories")
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    result.structured_content.unwrap()
}

fn copy_vault(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = destination.join(entry.file_name());
        if path.is_dir() {
            copy_vault(&path, &dest);
        } else if path.extension().is_some_and(|s| s == "sqlite") {
            let db = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            db.execute("VACUUM INTO ?1", [dest.display().to_string()])
                .unwrap();
        } else if !["-wal", "-shm"]
            .iter()
            .any(|suffix| path.to_string_lossy().ends_with(suffix))
        {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

fn statistics(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({"n":sorted.len(),"p50Ms":sorted[(sorted.len()-1)/2],"p95Ms":sorted[(sorted.len()*95).div_ceil(100)-1],"samplesMs":samples})
}

fn audit(db: &rusqlite::Connection) {
    db.execute_batch("CREATE TABLE optimization_writes(id TEXT); CREATE TRIGGER optimization_writes AFTER INSERT ON memories BEGIN INSERT INTO optimization_writes VALUES(new.id); END;").unwrap();
}

#[tokio::test]
#[ignore = "从实际存储结果生成桌面交互验收夹具，不请求模型"]
async fn prepare_desktop_ui_fixture() {
    use crate::search::{SearchEngine, SearchMode, SearchRequest};
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let base = project.join("dev-data/optimization-acceptance");
    let mut runs = fs::read_dir(&base)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("holdout-")
                && p.join("report.json").exists()
        })
        .collect::<Vec<_>>();
    runs.sort();
    let source = runs.pop().unwrap();
    let output = base.join("ui");
    let root = output.join(format!("vault-{}", uuid::Uuid::new_v4().simple()));
    copy_vault(&source.join("archive-vault"), &root);
    for entry in fs::read_dir(source.join("vault")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != ".aimemory" {
            copy_vault(&entry.path(), &root.join(entry.file_name()));
        }
    }
    let mut vault = Vault::open(&root).unwrap();
    let workspace = vault.workspace().unwrap();
    let memories = vault.search(None, "").unwrap();
    let mut records = serde_json::Map::new();
    let mut manual = None;
    for memory in &memories {
        let record = vault.read(&memory.id).unwrap();
        if !record.suggestions.is_empty() {
            manual = Some(memory.id.clone());
        }
        records.insert(memory.id.clone(), serde_json::to_value(record).unwrap());
    }
    let id = manual.unwrap();
    let initial = vault.read(&id).unwrap();
    let source_id = initial.suggestions[0].source_id.clone();
    let latest = vault
        .save(MemoryInput {
            id: Some(id.clone()),
            repository_id: initial.summary.repository_id,
            title: initial.summary.title,
            kind: initial.summary.kind,
            tags: initial.summary.tags,
            markdown: crate::memory_markdown::replace_conclusion(
                &initial.markdown,
                "采用两秒轮询；这是最新人工修订。",
            ),
            related_ids: initial.outgoing_related_ids,
            revision: Some(initial.revision),
            source: initial.source,
        })
        .unwrap();
    let adopted = vault
        .adopt_suggestion(&id, &source_id, &latest.revision)
        .unwrap();
    drop(Pipeline::open(&root).unwrap());
    let engine = SearchEngine::open(&root).unwrap();
    let repo = vault.read("H-07").unwrap().summary.repository_id;
    let keyword = engine
        .search_with(
            SearchRequest {
                query: "组件卸载".into(),
                repository_id: Some(repo.clone()),
                mode: SearchMode::Keyword,
                ..Default::default()
            },
            None,
            || panic!("offline fixture"),
        )
        .await
        .unwrap();
    let automatic = engine
        .search_with(
            SearchRequest {
                query: "组件卸载".into(),
                repository_id: Some(repo.clone()),
                ..Default::default()
            },
            None,
            || panic!("offline fixture"),
        )
        .await
        .unwrap();
    let group = engine
        .search_with(
            SearchRequest {
                repository_id: Some(repo),
                group_of: Some("H-07".into()),
                ..Default::default()
            },
            None,
            || panic!("offline fixture"),
        )
        .await
        .unwrap();
    fs::write(output.join("fixture.json"),serde_json::to_vec_pretty(&json!({"workspace":workspace,"memories":memories,"records":records,"manualId":id,"latest":latest,"adopted":adopted,"sourceId":source_id,"keyword":keyword,"automatic":automatic,"group":group})).unwrap()).unwrap();
}

#[tokio::test]
#[ignore = "回放已保存的真实向量，测量旧、新 HTTP 和单文件保存；不调用远程模型"]
async fn fixed_corpus_http_and_single_save_acceptance() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let historical = project.join("dev-data/performance-acceptance/semantic");
    let run = project
        .join("dev-data/optimization-acceptance")
        .join(format!(
            "run-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S")
        ));
    fs::create_dir_all(&run).unwrap();
    let corpus: Value = serde_json::from_slice(
        &fs::read(project.join("src-tauri/tests/fixtures/semantic-corpus.json")).unwrap(),
    )
    .unwrap();
    let credentials = crate::config::ServiceCredentials {
        base_url: "https://api.openai.com/v1".into(),
        model: "text-embedding-3-small".into(),
        api_key: String::new(),
    };
    let mut reports = Vec::new();
    for count in [1000, 10000] {
        let base = run.join(format!("per-repository-{count}"));
        let current = base.join("current");
        let old = base.join("baseline");
        let config = base.join("config");
        println!("copying fixed corpus: {count} per repository");
        copy_vault(&historical.join(format!("vault-{count}")), &current);
        copy_vault(&historical.join(format!("vault-{count}")), &old);
        let mut vault = Vault::open(&current).unwrap();
        let mut baseline = baseline_storage::Vault::open(&old).unwrap();
        let repositories = vault.workspace().unwrap().repositories;
        let mut settings = SettingsStore::load(&config, base.join("codex")).unwrap();
        settings
            .update(UpdateSettings {
                chat: ServiceInput {
                    provider: None,
                    base_url: credentials.base_url.clone(),
                    model: "unused".into(),
                    api_key: None,
                },
                embedding: ServiceInput {
                    provider: None,
                    base_url: credentials.base_url.clone(),
                    model: credentials.model.clone(),
                    api_key: None,
                },
                codex_directory: base.join("codex").display().to_string(),
                harness_directory: base.join("harness").display().to_string(),
                archive_sources: vec!["codex".into()],
                archive_enabled: false,
                run_in_background: false,
            })
            .unwrap();
        let mut new_client = Client::start(&config, &current, project.as_path()).await;
        let mut old_client = BaselineClient::start(&old, credentials.clone()).await;
        let mut cases = Vec::new();
        let mut top1 = 0;
        let mut hit5 = 0;
        let mut old_top1 = 0;
        let mut old_hit5 = 0;
        for query in corpus["queries"].as_array().unwrap() {
            let repository = repositories
                .iter()
                .find(|r| r.name == query["repository"].as_str().unwrap())
                .unwrap();
            let args = json!({"cwd":project,"repository":repository.id,"query":query["query"]});
            let before = call(&old_client.service, args.clone()).await;
            let after = call(&new_client.service, args).await;
            let expected = query["expectedIds"].as_array().unwrap();
            let rank = |result: &Value| {
                result["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|hit| expected.contains(&hit["id"]))
                    .map(|n| n + 1)
            };
            let old_rank = rank(&before);
            let new_rank = rank(&after);
            old_top1 += usize::from(old_rank == Some(1));
            old_hit5 += usize::from(old_rank.is_some());
            top1 += usize::from(new_rank == Some(1));
            hit5 += usize::from(new_rank.is_some());
            assert!(after["queryVectorCached"].as_bool().unwrap());
            assert_eq!(after["remoteRequests"], 0);
            cases.push(json!({"id":query["id"],"baselineRank":old_rank,"rank":new_rank,"baseline":before,"current":after}));
        }
        let first_query = &corpus["queries"][0];
        let repo = repositories
            .iter()
            .find(|r| r.name == first_query["repository"].as_str().unwrap())
            .unwrap();
        let args = json!({"repository":repo.id,"query":first_query["query"]});
        let mut hot = Vec::new();
        let mut old_hot = Vec::new();
        let mut local = Vec::new();
        let mut old_local = Vec::new();
        for _ in 0..40 {
            let started = Instant::now();
            let value = call(&old_client.service, args.clone()).await;
            old_hot.push(started.elapsed().as_secs_f64() * 1000.0);
            old_local.push(value["localSearchMs"].as_f64().unwrap());
            let started = Instant::now();
            let value = call(&new_client.service, args.clone()).await;
            hot.push(started.elapsed().as_secs_f64() * 1000.0);
            local.push(value["localSearchMs"].as_f64().unwrap());
        }
        let url = new_client._server.status().await.unwrap().url;
        let mut first_client = Vec::new();
        let mut old_first_client = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            let service = ()
                .serve(StreamableHttpClientTransport::from_uri(
                    old_client.url.clone(),
                ))
                .await
                .unwrap();
            call(&service, args.clone()).await;
            old_first_client.push(started.elapsed().as_secs_f64() * 1000.0);
            service.cancel().await.unwrap();
            let started = Instant::now();
            let service =
                ().serve(StreamableHttpClientTransport::from_uri(url.clone()))
                    .await
                    .unwrap();
            call(&service, args.clone()).await;
            first_client.push(started.elapsed().as_secs_f64() * 1000.0);
            service.cancel().await.unwrap();
        }
        new_client._server.stop().await.unwrap();
        old_client.stop().await;
        let mut boot = Vec::new();
        let mut old_boot = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            old_client = BaselineClient::start(&old, credentials.clone()).await;
            call(&old_client.service, args.clone()).await;
            old_boot.push(started.elapsed().as_secs_f64() * 1000.0);
            old_client.stop().await;
            let started = Instant::now();
            new_client = Client::start(&config, &current, project.as_path()).await;
            call(&new_client.service, args.clone()).await;
            boot.push(started.elapsed().as_secs_f64() * 1000.0);
            new_client._server.stop().await.unwrap();
        }
        let current_db =
            rusqlite::Connection::open(current.join(".aimemory/index.sqlite")).unwrap();
        let old_db = rusqlite::Connection::open(old.join(".aimemory/index.sqlite")).unwrap();
        audit(&current_db);
        audit(&old_db);
        let mut saves = Vec::new();
        let mut old_saves = Vec::new();
        for n in 0..10 {
            let record = baseline.read("G-F01").unwrap();
            let started = Instant::now();
            baseline
                .save(baseline_storage::MemoryInput {
                    id: Some(record.summary.id),
                    repository_id: record.summary.repository_id,
                    title: record.summary.title,
                    kind: record.summary.kind,
                    tags: record.summary.tags,
                    markdown: format!("{}\n保存性能样本 {n}\n", record.markdown),
                    related_ids: record.outgoing_related_ids,
                    revision: Some(record.revision),
                    source: record.source,
                })
                .unwrap();
            old_saves.push(started.elapsed().as_secs_f64() * 1000.0);
            let record = vault.read("G-F01").unwrap();
            let started = Instant::now();
            vault
                .save(MemoryInput {
                    id: Some(record.summary.id),
                    repository_id: record.summary.repository_id,
                    title: record.summary.title,
                    kind: record.summary.kind,
                    tags: record.summary.tags,
                    markdown: format!("{}\n保存性能样本 {n}\n", record.markdown),
                    related_ids: record.outgoing_related_ids,
                    revision: Some(record.revision),
                    source: record.source,
                })
                .unwrap();
            saves.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let writes: i64 = current_db
            .query_row("SELECT count(*) FROM optimization_writes", [], |r| r.get(0))
            .unwrap();
        let old_writes: i64 = old_db
            .query_row("SELECT count(*) FROM optimization_writes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(writes, 10);
        assert_eq!(old_writes, count * 3 * 10);
        let summary = json!({"perRepository":count,"recall":{"baselineHitAt5":old_hit5,"hitAt5":hit5,"baselineTop1":old_top1,"top1":top1,"questions":cases.len()},"cases":cases,"httpHot":{"baseline":statistics(&old_hot),"current":statistics(&hot)},"localCore":{"baseline":statistics(&old_local),"current":statistics(&local)},"newClientHandshakeAndFirstQuery":{"baseline":statistics(&old_first_client),"current":statistics(&first_client)},"newServerHandshakeAndFirstQuery":{"baseline":statistics(&old_boot),"current":statistics(&boot)},"singleFileSave":{"baseline":statistics(&old_saves),"current":statistics(&saves),"baselineIndexedRows":old_writes,"indexedRows":writes},"remoteRequests":0});
        println!(
            "fixed corpus {count}: hit5={hit5}/12 top1={top1}, HTTP hot p95={} ms",
            summary["httpHot"]["current"]["p95Ms"]
        );
        fs::write(
            base.join("report.json"),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
        reports.push(summary);
        fs::write(run.join("report.json"),serde_json::to_vec_pretty(&json!({"baselineRevision":"bb5cd35842e420330f799d7e2dc46b43dc0e8bfa","baselineDescription":"Original storage and search kernels served by the same rmcp Streamable HTTP transport; query vectors were already cached. Server startup excludes native window rendering.","scales":reports})).unwrap()).unwrap();
        assert_eq!(hit5, 12);
        assert_eq!(top1, 11);
        assert!(local.iter().all(|ms| *ms < 200.0));
    }
    println!(
        "optimization acceptance report: {}",
        run.join("report.json").display()
    );
}
