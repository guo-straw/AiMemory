use super::*;

#[tokio::test]
async fn duplicate_groups_preserve_ids_repository_scope_paging_and_cached_queries_without_credentials(
) {
    let root = fixture();
    let (service, requests) = serve_responses(vec![(200, response(vec![1.0, 0.0, 0.0]))]);
    let key = service_key(&service);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "grouped");
    let other = repository(&mut vault, "other");
    let mut members = Vec::new();
    for _ in 0..7 {
        members.push(
            ready_memory(
                &mut vault,
                &mut pipeline,
                &repo,
                "相同检索文本",
                vec![1.0, 0.0, 0.0],
                &key,
            )
            .summary
            .id,
        );
    }
    for index in 0..6 {
        ready_memory(
            &mut vault,
            &mut pipeline,
            &repo,
            &format!("不同决定 {index}"),
            vec![0.5, 0.5, index as f32],
            &key,
        );
    }
    let outside = ready_memory(
        &mut vault,
        &mut pipeline,
        &other,
        "相同检索文本",
        vec![1.0, 0.0, 0.0],
        &key,
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    let first = engine
        .search(request("同义改写问题", Some(&repo)), &service)
        .await
        .unwrap();
    assert_eq!(first.results.len(), 5);
    assert_eq!(first.results[0].group_count, 7);
    members.sort();
    assert_eq!(first.results[0].memory.id, members[0]);
    let page = engine
        .search_with(
            SearchRequest {
                repository_id: Some(repo.id.clone()),
                group_of: Some(members[4].clone()),
                limit: Some(3),
                offset: Some(3),
                ..Default::default()
            },
            None,
            || panic!("group lookup cannot request credentials"),
        )
        .await
        .unwrap();
    assert_eq!(page.total, Some(7));
    assert_eq!(page.has_more, Some(true));
    assert_eq!(
        page.results
            .iter()
            .map(|h| &h.memory.id)
            .collect::<Vec<_>>(),
        members[3..6].iter().collect::<Vec<_>>()
    );
    let exact = engine
        .search_with(request(&members[4], Some(&repo)), None, || {
            panic!("ID lookup cannot request credentials")
        })
        .await
        .unwrap();
    assert_eq!(exact.results[0].memory.id, members[4]);
    assert_eq!(exact.search_mode, "id");
    let outside = engine
        .search_with(request(&outside.summary.id, Some(&repo)), None, || {
            panic!("ID lookup cannot request credentials")
        })
        .await
        .unwrap();
    assert!(outside.results.is_empty());
    drop(engine);
    let engine = SearchEngine::open(root.path()).unwrap();
    let cached = engine
        .search_with(
            request("同义改写问题", Some(&repo)),
            Some(key),
            || Err("凭据已移除".into()),
        )
        .await
        .unwrap();
    assert!(cached.query_vector_cached);
    assert_eq!(cached.remote_requests, 0);
    assert!(cached.fallback_reason.is_none());
    requests.recv().unwrap();
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn keyword_history_keeps_current_summary_and_remote_failure_falls_back_once() {
    let root = fixture();
    let (service, requests) =
        serve_responses(vec![(503, json!({"error":{"message":"maintenance"}}))]);
    let key = service_key(&service);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "fallback");
    let memory = ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "导出方式",
        vec![1.0, 0.0, 0.0],
        &key,
    );
    let memory = vault.save(MemoryInput { id:Some(memory.summary.id),repository_id:repo.id.clone(),title:memory.summary.title,kind:memory.summary.kind,tags:memory.summary.tags,markdown:format!("{}\n## 变更记录\n\n旧代理缓冲 SSE，已放弃。\n\n## 待采用建议\n\n暂不采用 websocket。\n",memory.markdown),related_ids:Vec::new(),revision:Some(memory.revision),source:memory.source }).unwrap();
    let engine = SearchEngine::open(root.path()).unwrap();
    let mut input = request("SSE", Some(&repo));
    input.mode = SearchMode::Keyword;
    let local = engine
        .search_with(input, None, || panic!("keyword cannot request credentials"))
        .await
        .unwrap();
    assert_eq!(local.results[0].memory.summary, memory.summary.summary);
    assert!(local.results[0].matches_history_only);
    assert_eq!(local.results[0].matched_sections[0].status, "historical");
    assert!(local.fallback_reason.is_none());
    let failed = engine
        .search(request("轮询", Some(&repo)), &service)
        .await
        .unwrap();
    assert_eq!(failed.results.len(), 1);
    assert_eq!(failed.search_mode, "keyword");
    assert_eq!(failed.remote_requests, 1);
    assert!(!failed.query_usage_reported);
    assert!(failed.fallback_reason.unwrap().contains("503"));
    requests.recv().unwrap();
    assert!(requests.try_recv().is_err());
    let credentials = engine
        .search_with(request("另一个问法", Some(&repo)), Some(key), || {
            Err("凭据不可用".into())
        })
        .await
        .unwrap();
    assert_eq!(credentials.remote_requests, 0);
    assert!(credentials.query_usage_reported);
    assert!(credentials.fallback_reason.unwrap().contains("凭据"));
    let db = Connection::open(root.path().join(".aimemory/pipeline.sqlite")).unwrap();
    let attempted: i64 = db
        .query_row("SELECT SUM(remote_requests) FROM search_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(attempted, 1);
}

#[tokio::test]
async fn default_search_fuses_literal_and_pending_matches_with_ready_semantic_candidates() {
    let root = fixture();
    let (service, requests) = serve_responses(vec![(200, response(vec![1.0, 0.0, 0.0]))]);
    let key = service_key(&service);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "hybrid");
    let ready = ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "相关技术决定",
        vec![1.0, 0.0, 0.0],
        &key,
    );
    let pending = vault
        .save(MemoryInput {
            id: None,
            repository_id: repo.id.clone(),
            title: "固定灰度版本".into(),
            kind: "技术决策".into(),
            tags: Vec::new(),
            markdown: "## 结论\n\nlamp.grayscale.fixed-versions 覆盖请求头。".into(),
            related_ids: Vec::new(),
            revision: None,
            source: None,
        })
        .unwrap();
    let engine = SearchEngine::open(root.path()).unwrap();
    let response = engine
        .search(
            request("怎么配置`lamp.grayscale.fixed-versions`？", Some(&repo)),
            &service,
        )
        .await
        .unwrap();
    assert_eq!(response.search_mode, "hybrid");
    assert_eq!(response.score_kind, "rrf");
    assert_eq!(response.pending_embeddings, 1);
    assert!(response
        .results
        .iter()
        .any(|h| h.memory.id == pending.summary.id));
    assert!(response
        .results
        .iter()
        .any(|h| h.memory.id == ready.summary.id));
    requests.recv().unwrap();
}
use crate::{
    model_api::{tests::serve_responses, ApiUsage, EmbeddingResult},
    pipeline::Pipeline,
    storage::{MemoryInput, MemoryRecord, Repository, RepositoryInput, Vault},
};
use serde_json::json;
use std::{fs, path::PathBuf};

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn repository(vault: &mut Vault, name: &str) -> Repository {
    vault
        .add_repository(RepositoryInput {
            name: name.into(),
            remote: format!("https://github.com/aimemory-tests/{name}.git"),
        })
        .unwrap()
}

fn ready_memory(
    vault: &mut Vault,
    pipeline: &mut Pipeline,
    repository: &Repository,
    title: &str,
    vector: Vec<f32>,
    key: &str,
) -> MemoryRecord {
    let memory = vault
        .save(MemoryInput {
            id: None,
            repository_id: repository.id.clone(),
            title: title.into(),
            kind: "技术决策".into(),
            tags: vec!["轮询".into()],
            markdown: "## 结论\n每两秒查询任务。\n\n## 原文依据\n> 离开页面时停止。\n".into(),
            related_ids: Vec::new(),
            revision: None,
            source: Some("真实文件测试".into()),
        })
        .unwrap();
    pipeline.sync_embeddings(vault, key, true).unwrap();
    let job = pipeline.next_embedding().unwrap().unwrap();
    assert_eq!(job.memory_id, memory.summary.id);
    pipeline
        .embedding_complete(
            &job,
            Ok(EmbeddingResult {
                vector,
                usage: ApiUsage {
                    input_tokens: 4,
                    output_tokens: 0,
                    reported: true,
                },
                latency_ms: 1,
            }),
        )
        .unwrap();
    memory
}

fn request(query: &str, repository: Option<&Repository>) -> SearchRequest {
    SearchRequest {
        mode: Default::default(),
        group_of: None,
        offset: None,
        query: query.into(),
        repository_id: repository.map(|repository| repository.id.clone()),
        limit: None,
    }
}

fn response(vector: Vec<f32>) -> serde_json::Value {
    json!({"data":[{"embedding":vector}],"usage":{"prompt_tokens":6,"total_tokens":6}})
}

#[tokio::test]
async fn current_repository_defaults_to_five_summaries_and_reuses_query_across_repositories_and_restart(
) {
    let root = fixture();
    let (service, requests) = serve_responses(vec![(200, response(vec![1.0, 0.0, 0.0]))]);
    let key = service_key(&service);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let frontend = repository(&mut vault, "frontend");
    let backend = repository(&mut vault, "backend");
    let mut ids = Vec::new();
    for index in 0..7 {
        ids.push(
            ready_memory(
                &mut vault,
                &mut pipeline,
                &frontend,
                &format!("前端决定 {index}"),
                vec![1.0, index as f32 / 10.0, 0.0],
                &key,
            )
            .summary
            .id,
        );
    }
    let other = ready_memory(
        &mut vault,
        &mut pipeline,
        &backend,
        "后端约束",
        vec![1.0, 0.0, 0.0],
        &key,
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    let first = engine
        .search(request("为什么改成轮询", Some(&frontend)), &service)
        .await
        .unwrap();
    assert_eq!(first.results.len(), 5);
    assert_eq!(first.results[0].memory.id, ids[0]);
    assert!(first
        .results
        .iter()
        .all(|hit| hit.memory.repository_id == frontend.id));
    assert!(!first.query_vector_cached);
    assert_eq!(first.embedding_input_tokens, 6);
    assert!(first.query_usage_reported);
    assert_eq!(requests.recv().unwrap().path, "/v1/embeddings");
    let serialized = serde_json::to_value(&first).unwrap();
    assert!(serialized["results"][0].get("markdown").is_none());

    let second = engine
        .search(request("  为什么改成轮询  ", Some(&backend)), &service)
        .await
        .unwrap();
    assert_eq!(second.results[0].memory.id, other.summary.id);
    assert!(second.query_vector_cached);
    assert_eq!(second.embedding_input_tokens, 0);
    drop(engine);
    let reopened = SearchEngine::open(root.path()).unwrap();
    let repeated = reopened
        .search(request("为什么改成轮询", Some(&frontend)), &service)
        .await
        .unwrap();
    assert!(repeated.query_vector_cached);
    assert_eq!(repeated.results.len(), 5);
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn two_engine_instances_share_a_single_in_flight_http_request() {
    let root = fixture();
    let (service, requests) = serve_responses(vec![(200, response(vec![1.0, 0.0, 0.0]))]);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "轮询决定",
        vec![1.0, 0.0, 0.0],
        &service_key(&service),
    );
    let first = SearchEngine::open(root.path()).unwrap();
    let second = SearchEngine::open(root.path()).unwrap();
    let (a, b) = tokio::join!(
        first.search(request("相同查询", Some(&repo)), &service),
        second.search(request("相同查询", Some(&repo)), &service)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.query_vector_cached, b.query_vector_cached);
    assert_eq!(a.embedding_input_tokens + b.embedding_input_tokens, 6);
    requests.recv().unwrap();
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn external_index_changes_exclude_stale_vectors_and_new_embeddings_become_visible() {
    let root = fixture();
    let (service, requests) = serve_responses(vec![(200, response(vec![1.0, 0.0, 0.0]))]);
    let key = service_key(&service);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    let memory = ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "原始标题",
        vec![1.0, 0.0, 0.0],
        &key,
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    assert_eq!(
        engine
            .search(request("轮询", Some(&repo)), &service)
            .await
            .unwrap()
            .results
            .len(),
        1
    );
    requests.recv().unwrap();
    let changed = vault
        .save(MemoryInput {
            id: Some(memory.summary.id),
            repository_id: repo.id.clone(),
            title: "更新后的标题".into(),
            kind: memory.summary.kind,
            tags: memory.summary.tags,
            markdown: memory.markdown,
            related_ids: memory.outgoing_related_ids,
            revision: Some(memory.revision),
            source: memory.source,
        })
        .unwrap();
    let stale = engine
        .search(request("轮询", Some(&repo)), &service)
        .await
        .unwrap();
    assert_eq!(stale.search_mode, "keyword");
    assert_eq!(stale.results[0].memory.title, changed.summary.title);
    assert!(stale.fallback_reason.is_some());
    assert_eq!(stale.pending_embeddings, 1);
    pipeline.sync_embeddings(&vault, &key, true).unwrap();
    let job = pipeline.next_embedding().unwrap().unwrap();
    pipeline
        .embedding_complete(
            &job,
            Ok(EmbeddingResult {
                vector: vec![1.0, 0.0, 0.0],
                usage: ApiUsage::default(),
                latency_ms: 1,
            }),
        )
        .unwrap();
    let fresh = engine
        .search(request("轮询", Some(&repo)), &service)
        .await
        .unwrap();
    assert_eq!(fresh.results[0].memory.title, changed.summary.title);
    assert!(fresh.query_vector_cached);
    assert_eq!(fresh.pending_embeddings, 0);
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn edit_during_remote_query_is_rechecked_before_results_are_returned() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, request_received) = tokio::sync::oneshot::channel();
    let (release, response_allowed) = tokio::sync::oneshot::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                }
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        accepted.send(()).unwrap();
        response_allowed.blocking_recv().unwrap();
        let response = response(vec![1.0, 0.0, 0.0]).to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
    });
    let root = fixture();
    let service = ServiceCredentials {
        base_url: format!("http://{address}/v1"),
        model: "test-model".into(),
        api_key: String::new(),
    };
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    let memory = ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "旧标题",
        vec![1.0, 0.0, 0.0],
        &service_key(&service),
    );
    let engine = Arc::new(SearchEngine::open(root.path()).unwrap());
    let query = request("轮询", Some(&repo));
    let querying = {
        let engine = engine.clone();
        let service = service.clone();
        tokio::spawn(async move { engine.search(query, &service).await })
    };
    request_received.await.unwrap();
    vault
        .save(MemoryInput {
            id: Some(memory.summary.id),
            repository_id: repo.id.clone(),
            title: "查询期间更新的新标题".into(),
            kind: memory.summary.kind,
            tags: memory.summary.tags,
            markdown: memory.markdown,
            related_ids: memory.outgoing_related_ids,
            revision: Some(memory.revision),
            source: memory.source,
        })
        .unwrap();
    pipeline
        .sync_embeddings(&vault, &service_key(&service), true)
        .unwrap();
    release.send(()).unwrap();
    let result = querying.await.unwrap().unwrap();
    assert_eq!(result.results[0].memory.title, "查询期间更新的新标题");
    assert_eq!(result.search_mode, "keyword");
    assert_eq!(result.pending_embeddings, 1);
    assert_eq!(result.embedding_input_tokens, 6);
    server.join().unwrap();
}

#[tokio::test]
async fn service_change_does_not_search_vectors_from_another_model() {
    let root = fixture();
    let (service, _requests) = serve_responses(Vec::new());
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "轮询",
        vec![1.0, 0.0, 0.0],
        &service_key(&service),
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    let different = ServiceCredentials {
        model: "changed-model".into(),
        ..service
    };
    let result = engine
        .search(request("轮询", Some(&repo)), &different)
        .await
        .unwrap();
    assert_eq!(result.results[0].memory.title, "轮询");
    assert_eq!(result.search_mode, "keyword");
    assert_eq!(result.pending_embeddings, 1);
    assert_eq!(result.embedding_input_tokens, 0);
}

#[tokio::test]
async fn dimension_mismatch_is_explicit_and_zero_query_vector_releases_its_claim() {
    let root = fixture();
    let (service, requests) = serve_responses(vec![
        (200, response(vec![1.0, 0.0, 0.0])),
        (200, response(vec![0.0, 0.0, 0.0])),
        (200, response(vec![1.0, 0.0, 0.0])),
    ]);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "轮询",
        vec![1.0, 0.0],
        &service_key(&service),
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    assert!(engine
        .search(request("维数", Some(&repo)), &service)
        .await
        .unwrap()
        .fallback_reason
        .unwrap()
        .contains("维数"));
    assert!(engine
        .search(request("零向量", Some(&repo)), &service)
        .await
        .unwrap()
        .fallback_reason
        .unwrap()
        .contains("无法计算"));
    assert!(engine
        .search(request("零向量", Some(&repo)), &service)
        .await
        .unwrap()
        .fallback_reason
        .unwrap()
        .contains("维数"));
    for _ in 0..3 {
        requests.recv().unwrap();
    }
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn missing_usage_is_distinguished_from_a_cached_query_with_zero_cost() {
    let root = fixture();
    let (service, requests) =
        serve_responses(vec![(200, json!({"data":[{"embedding":[1.0,0.0,0.0]}]}))]);
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let repo = repository(&mut vault, "frontend");
    ready_memory(
        &mut vault,
        &mut pipeline,
        &repo,
        "轮询",
        vec![1.0, 0.0, 0.0],
        &service_key(&service),
    );
    let engine = SearchEngine::open(root.path()).unwrap();
    let first = engine
        .search(request("轮询", Some(&repo)), &service)
        .await
        .unwrap();
    assert!(!first.query_usage_reported);
    assert!(!first.query_vector_cached);
    let cached = engine
        .search(request("轮询", Some(&repo)), &service)
        .await
        .unwrap();
    assert!(cached.query_usage_reported);
    assert!(cached.query_vector_cached);
    requests.recv().unwrap();
}

#[test]
fn malformed_vectors_and_invalid_search_input_are_validated_at_entry() {
    assert!(decode_vector(&[0; 7], 2).is_err());
    assert!(decode_vector(&f32::NAN.to_le_bytes(), 1).is_err());
    assert!(validate_request(&request("  ", None)).is_err());
    let mut input = request("轮询", None);
    input.limit = Some(21);
    assert!(validate_request(&input).is_err());
}

fn benchmark_index(root: &Path, count: usize, dimensions: usize, key: &str) {
    let vault = Vault::open(root).unwrap();
    let _pipeline = Pipeline::open(root).unwrap();
    drop(vault);
    let mut memories = Connection::open(root.join(".aimemory/index.sqlite")).unwrap();
    let mut pipeline = Connection::open(root.join(".aimemory/pipeline.sqlite")).unwrap();
    let memory_transaction = memories.transaction().unwrap();
    let vector_transaction = pipeline.transaction().unwrap();
    for index in 0..count {
        let id = format!("BENCH-{index:05}");
        let title = format!("异步导出任务决策 {index}");
        let summary = "任务按两秒间隔查询，终态后停止，离开页面时清理计时器。";
        let content = format!("{title}\n{summary}\n轮询");
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let vector: Vec<u8> = (0..dimensions)
            .flat_map(|dimension| {
                let value = ((index * 31 + dimension * 17) % 1021) as f32 / 1021.0 - 0.5;
                value.to_le_bytes()
            })
            .collect();
        memory_transaction.execute("INSERT INTO memories VALUES(?1,'benchmark',?2,?3,?4,'技术决策','[\"轮询\"]','2026-11-21T00:00:00Z','## 结论\n任务轮询。',NULL,'revision')",params![id,format!("{index}.md"),title,summary]).unwrap();
        vector_transaction.execute("INSERT INTO embeddings(memory_id,input_hash,service_key,content,status,vector,dimensions) VALUES(?1,?2,?3,?4,'ready',?5,?6)",params![id,hash,key,content,vector,dimensions as i64]).unwrap();
    }
    memory_transaction.commit().unwrap();
    vector_transaction.commit().unwrap();
}

#[tokio::test]
#[ignore = "生成 1,000/10,000 条 1536 维真实 SQLite 索引并测量本地检索 P95"]
async fn benchmark_1000_and_10000() {
    let dimensions = 1536;
    let query = (0..dimensions)
        .map(|dimension| ((dimension * 17) % 1021) as f32 / 1021.0 - 0.5)
        .collect::<Vec<_>>();
    for count in [1_000, 10_000] {
        let root = fixture();
        let (service, requests) = serve_responses(vec![(200, response(query.clone()))]);
        let key = service_key(&service);
        benchmark_index(root.path(), count, dimensions, &key);
        let input = SearchRequest {
            mode: Default::default(),
            group_of: None,
            offset: None,
            query: "性能验收查询".into(),
            repository_id: Some("benchmark".into()),
            limit: None,
        };
        let seed = SearchEngine::open(root.path()).unwrap();
        let seeded = seed.search(input.clone(), &service).await.unwrap();
        assert!(!seeded.query_vector_cached);
        assert_eq!(requests.recv().unwrap().path, "/v1/embeddings");
        drop(seed);

        let snapshot_engine = SearchEngine::open(root.path()).unwrap();
        let snapshot_started = Instant::now();
        let snapshot = snapshot_engine.snapshot(&key).unwrap();
        let cold_snapshot_ms = snapshot_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(snapshot.entries.len(), count);
        drop(snapshot);
        drop(snapshot_engine);

        let engine = SearchEngine::open(root.path()).unwrap();
        let cold_started = Instant::now();
        let cold = engine.search(input.clone(), &service).await.unwrap();
        let cold_request_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
        assert!(cold.query_vector_cached);
        assert_eq!(cold.results.len(), 5);
        let mut samples = Vec::new();
        let mut local_samples = Vec::new();
        for _ in 0..40 {
            let started = Instant::now();
            let result = engine.search(input.clone(), &service).await.unwrap();
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            local_samples.push(result.local_search_ms);
            assert!(result.query_vector_cached);
            assert_eq!(result.embedding_input_tokens, 0);
            assert_eq!(result.results.len(), 5);
            assert_eq!(result.pending_embeddings, 0);
        }
        samples.sort_by(f64::total_cmp);
        local_samples.sort_by(f64::total_cmp);
        let p95 = samples[(samples.len() * 95 / 100) - 1];
        let local_p95 = local_samples[(local_samples.len() * 95 / 100) - 1];
        println!("semantic_search count={count} dimensions={dimensions} cold_snapshot_ms={cold_snapshot_ms:.2} cold_cached_request_ms={cold_request_ms:.2} warm_cached_request_p95_ms={p95:.2} warm_local_p95_ms={local_p95:.2} profile={}", if cfg!(debug_assertions) { "debug" } else { "release" });
        assert!(requests.try_recv().is_err());
        if !cfg!(debug_assertions) {
            assert!(p95 < 200.0, "Local search P95 target exceeded: {p95:.2} ms");
        }
    }
}
