use super::*;
use crate::{
    model_api::tests::serve_responses,
    pipeline::Pipeline,
    storage::{Repository, RepositoryInput, Vault},
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

const DIMENSIONS: usize = 1536;
const WARM_SAMPLES: usize = 100;
const COLD_SAMPLES: usize = 5;
const WORKER_ENV: &str = "AIMEMORY_LOCAL_SEARCH_PERFORMANCE_WORKER";
const BASE_QUERY: &str = "性能验收：任务轮询为什么在终态停止";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Measurement {
    wall_ms: f64,
    local_search_ms: f64,
    embedding_latency_ms: u64,
    query_vector_cached: bool,
    embedding_input_tokens: u64,
    result_ids: Vec<String>,
    result_repository_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerInput {
    root: PathBuf,
    output: PathBuf,
    base_url: String,
    model: String,
    query: String,
    repository_id: Option<String>,
    repeat: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerResult {
    first: Measurement,
    repeated: Option<Measurement>,
}

fn build_dataset(root: &Path, count: usize, key: &str) -> Vec<Repository> {
    let mut vault = Vault::open(root).unwrap();
    let repositories = ["frontend", "backend", "shared"]
        .into_iter()
        .map(|name| {
            vault
                .add_repository(RepositoryInput {
                    name: name.into(),
                    remote: format!("https://github.com/aimemory-performance/{name}.git"),
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    for (repository_index, repository) in repositories.iter().enumerate() {
        for index in 0..count {
            let id = format!("PERF-{repository_index}-{index:05}");
            let title = format!("{} 任务轮询决定 {index}", repository.name);
            let markdown = format!(
                "## 结论\n任务按两秒间隔查询，终态后停止，离开页面时清理计时器。\n\n## 详细说明\n{} 仓库的固定性能数据 {index}；合成向量只用于检索性能与仓库过滤验收。\n\n## 原文依据\n> 源轮次 {id}：任务进入终态后停止轮询。\n",
                repository.name
            );
            let meta = json!({
                "id": id, "repository_id": repository.id,
                "repository_name": repository.name, "repository_remote": repository.remote,
                "title": title, "kind": "技术决策", "tags": ["轮询", "性能验收"],
                "related_ids": [], "source": "固定性能夹具，无真实会话数据",
                "created_at": "2026-11-21T00:00:00.000Z",
                "updated_at": "2026-11-21T00:00:00.000Z"
            });
            let raw = format!(
                "---\n{}---\n{markdown}",
                serde_yaml_ng::to_string(&meta).unwrap()
            );
            fs::write(root.join(&repository.id).join(format!("{id}.md")), raw).unwrap();
        }
    }
    // One refresh imports all real Markdown files in the storage transaction.
    vault.refresh().unwrap();
    let workspace = vault.workspace().unwrap();
    assert!(workspace.warnings.is_empty(), "{:?}", workspace.warnings);
    assert_eq!(workspace.repositories.len(), 3);
    assert!(workspace
        .repositories
        .iter()
        .all(|repository| repository.memory_count == count));
    for repository_index in 0..3 {
        for index in [0, count / 2, count - 1] {
            let id = format!("PERF-{repository_index}-{index:05}");
            let memory = vault.read(&id).unwrap();
            assert!(memory.markdown.contains(&format!("源轮次 {id}")));
            assert!(Path::new(&memory.summary.path).is_file());
        }
    }
    drop(vault);

    drop(Pipeline::open(root).unwrap());
    let memories = Connection::open(root.join(".aimemory/index.sqlite")).unwrap();
    let mut pipeline = Connection::open(root.join(".aimemory/pipeline.sqlite")).unwrap();
    let transaction = pipeline.transaction().unwrap();
    let mut insert = transaction.prepare(
        "INSERT INTO embeddings(memory_id,input_hash,service_key,content,status,vector,dimensions)
         VALUES(?1,?2,?3,?4,'ready',?5,?6)",
    ).unwrap();
    let mut statement = memories
        .prepare("SELECT id,title,summary,tags FROM memories ORDER BY id")
        .unwrap();
    let mut rows = statement.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: String = row.get(0).unwrap();
        let title: String = row.get(1).unwrap();
        let summary: String = row.get(2).unwrap();
        let tags: Vec<String> = serde_json::from_str(&row.get::<_, String>(3).unwrap()).unwrap();
        let content = format!("{title}\n{summary}\n{}", tags.join(" "));
        let parts = id.split('-').collect::<Vec<_>>();
        let repository_index = parts[1].parse::<usize>().unwrap();
        let index = parts[2].parse::<usize>().unwrap();
        let vector = (0..DIMENSIONS)
            .flat_map(|dimension| {
                let value = match (index, dimension) {
                    (0 | 1, 0) => 1.0,
                    (1, 1) => 0.01,
                    (0 | 1, _) => 0.0,
                    (_, 0) => 0.05,
                    _ => {
                        ((index * 31 + repository_index * 47 + dimension * 17) % 1021) as f32
                            / 1021.0
                            - 0.5
                    }
                };
                value.to_le_bytes()
            })
            .collect::<Vec<_>>();
        insert
            .execute(params![
                id,
                blake3::hash(content.as_bytes()).to_hex().to_string(),
                key,
                content,
                vector,
                DIMENSIONS as i64
            ])
            .unwrap();
    }
    drop(insert);
    transaction.commit().unwrap();
    pipeline
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    repositories
}

async fn measure(
    engine: &SearchEngine,
    request: SearchRequest,
    service: &ServiceCredentials,
) -> Measurement {
    let started = Instant::now();
    let response = engine.search(request.clone(), service).await.unwrap();
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(response.results.len(), 5, "default limit must be five");
    assert_eq!(response.pending_embeddings, 0);
    assert!(response.query_usage_reported);
    if let Some(repository) = request.repository_id {
        assert!(response
            .results
            .iter()
            .all(|hit| hit.memory.repository_id == repository));
    } else {
        let repositories = response
            .results
            .iter()
            .map(|hit| &hit.memory.repository_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            repositories.len(),
            3,
            "cross-repository search should include all three fixtures"
        );
    }
    Measurement {
        wall_ms,
        local_search_ms: response.local_search_ms,
        embedding_latency_ms: response.embedding_latency_ms,
        query_vector_cached: response.query_vector_cached,
        embedding_input_tokens: response.embedding_input_tokens,
        result_ids: response
            .results
            .iter()
            .map(|hit| hit.memory.id.clone())
            .collect(),
        result_repository_ids: response
            .results
            .iter()
            .map(|hit| hit.memory.repository_id.clone())
            .collect(),
    }
}

fn statistics(values: impl Iterator<Item = f64>) -> Value {
    let mut values = values.collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    json!({
        "sampleCount": values.len(),
        "p50": values[(values.len() * 50).div_ceil(100) - 1],
        "p95": values[(values.len() * 95).div_ceil(100) - 1],
        "max": values.last().unwrap()
    })
}

fn phase_report(samples: &[Measurement], cached: bool) -> Value {
    let mut report = json!({
        "localSearchMs": statistics(samples.iter().map(|sample| sample.local_search_ms)),
        "embeddingLatencyMs": statistics(samples.iter().map(|sample| sample.embedding_latency_ms as f64)),
        "samples": samples
    });
    report[if cached {
        "cachedWallMs"
    } else {
        "wallMsIncludingLocalHttp"
    }] = statistics(samples.iter().map(|sample| sample.wall_ms));
    report
}

fn run_worker(input: WorkerInput, input_path: &Path) -> WorkerResult {
    fs::write(input_path, serde_json::to_vec_pretty(&input).unwrap()).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "search::performance_tests::process_cold_search_worker",
            "--ignored",
            "--nocapture",
        ])
        .env(WORKER_ENV, input_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "cold worker failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&fs::read(input.output).unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "internal fresh-process worker for the multi-repository performance acceptance"]
async fn process_cold_search_worker() {
    let Some(path) = std::env::var_os(WORKER_ENV) else {
        return;
    };
    let input: WorkerInput = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let service = ServiceCredentials {
        base_url: input.base_url,
        model: input.model,
        api_key: "test-only-key".into(),
    };
    let request = SearchRequest {
        mode: Default::default(),
        group_of: None,
        offset: None,
        query: input.query,
        repository_id: input.repository_id,
        limit: None,
    };
    let engine = SearchEngine::open(&input.root).unwrap();
    let first = measure(&engine, request.clone(), &service).await;
    let repeated = if input.repeat {
        Some(measure(&engine, request, &service).await)
    } else {
        None
    };
    fs::write(
        input.output,
        serde_json::to_vec_pretty(&WorkerResult { first, repeated }).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "3 repositories × 1,000/10,000 real Markdown + SQLite records; warm and fresh-process search acceptance"]
async fn multi_repository_local_search_acceptance() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dev-data/performance-acceptance/local-search");
    let output = base.join(format!(
        "run-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    ));
    fs::create_dir_all(&output).unwrap();
    let query_vector = (0..DIMENSIONS)
        .map(|index| if index == 0 { 1.0 } else { 0.0 })
        .collect::<Vec<_>>();
    let mut datasets = Vec::new();
    for count in [1_000, 10_000] {
        let directory = output.join(format!("per-repository-{count}"));
        let root = directory.join("vault");
        fs::create_dir_all(directory.join("workers")).unwrap();
        let expected_calls = 1 + 4 * COLD_SAMPLES;
        let (service, requests) = serve_responses((0..expected_calls).map(|_| (200, json!({"data":[{"embedding":query_vector}],"usage":{"prompt_tokens":6,"total_tokens":6}}))).collect());
        let generation_started = Instant::now();
        let repositories = build_dataset(&root, count, &service_key(&service));
        let generation_ms = generation_started.elapsed().as_secs_f64() * 1000.0;
        let engine = SearchEngine::open(&root).unwrap();
        let prime = measure(
            &engine,
            SearchRequest {
                mode: Default::default(),
                group_of: None,
                offset: None,
                query: BASE_QUERY.into(),
                repository_id: Some(repositories[0].id.clone()),
                limit: None,
            },
            &service,
        )
        .await;
        assert!(!prime.query_vector_cached);
        assert_eq!(prime.embedding_input_tokens, 6);
        assert_eq!(requests.recv().unwrap().path, "/v1/embeddings");
        drop(engine);
        let scopes = repositories
            .iter()
            .map(|repository| (repository.name.clone(), Some(repository.id.clone())))
            .chain(std::iter::once(("all-repositories".into(), None)))
            .collect::<Vec<_>>();
        let mut scope_reports = Vec::new();
        for (scope, repository_id) in scopes {
            let request = SearchRequest {
                mode: Default::default(),
                group_of: None,
                offset: None,
                query: BASE_QUERY.into(),
                repository_id: repository_id.clone(),
                limit: None,
            };
            let engine = SearchEngine::open(&root).unwrap();
            let warmup = measure(&engine, request.clone(), &service).await;
            assert!(warmup.query_vector_cached);
            let mut warm = Vec::new();
            for _ in 0..WARM_SAMPLES {
                let sample = measure(&engine, request.clone(), &service).await;
                assert!(sample.query_vector_cached);
                assert_eq!(sample.embedding_input_tokens, 0);
                warm.push(sample);
            }
            drop(engine);
            assert_eq!(
                requests.try_iter().count(),
                0,
                "warm repeat or scope change made an HTTP request"
            );
            let mut cold_cached = Vec::new();
            let mut cold_fresh = Vec::new();
            let mut cold_repeats = Vec::new();
            for iteration in 0..COLD_SAMPLES {
                for fresh in [false, true] {
                    let name = format!(
                        "{scope}-{}-{iteration}",
                        if fresh { "fresh-query" } else { "cached-query" }
                    );
                    let input = WorkerInput {
                        root: root.clone(),
                        output: directory
                            .join("workers")
                            .join(format!("{name}.result.json")),
                        base_url: service.base_url.clone(),
                        model: service.model.clone(),
                        query: if fresh {
                            format!("{BASE_QUERY} / {scope} / {iteration}")
                        } else {
                            BASE_QUERY.into()
                        },
                        repository_id: repository_id.clone(),
                        repeat: fresh,
                    };
                    let result = run_worker(
                        input,
                        &directory.join("workers").join(format!("{name}.input.json")),
                    );
                    assert_eq!(result.first.query_vector_cached, !fresh);
                    assert_eq!(
                        result.first.embedding_input_tokens,
                        if fresh { 6 } else { 0 }
                    );
                    assert_eq!(
                        requests.try_iter().count(),
                        usize::from(fresh),
                        "fresh query must call once; cached/repeated query must call zero times"
                    );
                    if let Some(repeated) = result.repeated {
                        assert!(repeated.query_vector_cached);
                        assert_eq!(repeated.embedding_input_tokens, 0);
                        cold_repeats.push(repeated);
                    }
                    if fresh {
                        cold_fresh.push(result.first);
                    } else {
                        cold_cached.push(result.first);
                    }
                }
            }
            let scope_report = json!({
                "scope": scope, "repositoryId": repository_id,
                "warmCached": phase_report(&warm, true),
                "processColdSnapshotCachedQuery": phase_report(&cold_cached, true),
                "processColdSnapshotFreshQuery": phase_report(&cold_fresh, false),
                "freshQueryImmediateRepeat": phase_report(&cold_repeats, true)
            });
            println!("per_repository={count} scope={scope} warm_local_p95_ms={:.3} warm_cached_wall_p95_ms={:.3} process_cold_cached_local_p95_ms={:.3} process_cold_fresh_local_p95_ms={:.3}",
                scope_report["warmCached"]["localSearchMs"]["p95"].as_f64().unwrap(), scope_report["warmCached"]["cachedWallMs"]["p95"].as_f64().unwrap(),
                scope_report["processColdSnapshotCachedQuery"]["localSearchMs"]["p95"].as_f64().unwrap(), scope_report["processColdSnapshotFreshQuery"]["localSearchMs"]["p95"].as_f64().unwrap());
            scope_reports.push(scope_report);
        }
        assert_eq!(requests.try_iter().count(), 0);
        let db = Connection::open(root.join(".aimemory/pipeline.sqlite")).unwrap();
        let (recorded_calls, input_tokens): (i64, i64) = db
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(input_tokens),0) FROM api_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recorded_calls, expected_calls as i64);
        assert_eq!(input_tokens, expected_calls as i64 * 6);
        let dataset_report = json!({
            "perRepository": count, "repositoryCount": 3, "totalMemories": count * 3,
            "dimensions": DIMENSIONS, "defaultLimit": 5, "root": root,
            "generationMsExcludedFromSearch": generation_ms, "repositories": repositories,
            "pipelineBytes": fs::metadata(root.join(".aimemory/pipeline.sqlite")).unwrap().len(),
            "indexBytes": fs::metadata(root.join(".aimemory/index.sqlite")).unwrap().len(),
            "cacheProof": {"uncachedQueries": expected_calls,"httpCalls": recorded_calls,"reportedInputTokens": input_tokens,
                "repeatedOrScopeChangedQueries": 4 * (WARM_SAMPLES + 1 + COLD_SAMPLES * 2),
                "repeatAdditionalHttpCalls": 0,"repeatAdditionalInputTokens": 0},
            "scopes": scope_reports
        });
        fs::write(
            directory.join("results.json"),
            serde_json::to_vec_pretty(&dataset_report).unwrap(),
        )
        .unwrap();
        datasets.push(dataset_report);
    }
    let report = json!({
        "schemaVersion": 1, "generatedAt": chrono::Utc::now().to_rfc3339(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "environment": {"os": std::env::consts::OS,"architecture": std::env::consts::ARCH},
        "method": {
            "vectors": "Deterministic synthetic 1536-dimensional vectors; performance and repository filtering only, not semantic recall.",
            "storage": "Real Markdown source files with conclusions and quoted evidence; Vault.refresh bulk transaction; real SQLite embeddings.",
            "warm": "100 measured searches per scope after one excluded warmup; query vector and in-process snapshot cached.",
            "processCold": "Five separate fresh test processes per phase and scope. Each opens a new SearchEngine with no in-process snapshot; operating-system file caches may remain warm. Process startup and engine open are excluded from search wall time.",
            "freshQuery": "A unique query has no persisted cache entry. Local HTTP embeddings supply six reported input tokens; wall time includes that HTTP call. The immediate repeat must add zero calls and zero tokens.",
            "localSearchMs": "SearchResponse.local_search_ms includes snapshot loading and ranking, excludes the query-vector HTTP request.",
            "cachedWallMs": "Wall elapsed around SearchEngine.search for an already persisted query vector; includes query-cache SQLite lookup, excludes UI/MCP transport.",
            "percentiles": "Nearest-rank P50/P95; five-sample cold P95 is the maximum. Data generation and Markdown reads are excluded from search timing."
        },
        "datasets": datasets
    });
    fs::write(
        output.join("results.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    fs::write(
        base.join("results.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    if !cfg!(debug_assertions) {
        for scope in report["datasets"][1]["scopes"].as_array().unwrap() {
            for phase in [
                "warmCached",
                "processColdSnapshotCachedQuery",
                "processColdSnapshotFreshQuery",
            ] {
                let p95 = scope[phase]["localSearchMs"]["p95"].as_f64().unwrap();
                assert!(p95 < 200.0, "10,000 memories per repository: {} {phase} local P95 {p95:.3} ms exceeds 200 ms", scope["scope"]);
            }
        }
    }
    println!(
        "local search acceptance report: {}",
        output.join("results.json").display()
    );
}
