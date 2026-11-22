use knowtrace_lib::{
    codex,
    config::{ServiceCredentials, ServiceKind, SettingsStore},
    mcp::{self, ReadMemoryRequest, ReadMode},
    model_api::{self, ApiUsage},
    pipeline::Pipeline,
    search::{service_key, SearchEngine, SearchRequest, SearchResponse},
    storage::{write_atomic, Repository, RepositoryInput, Vault},
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Corpus {
    version: u32,
    description: String,
    repositories: Vec<RepositorySpec>,
    memories: Vec<MemorySpec>,
    queries: Vec<QuerySpec>,
}

#[derive(Deserialize)]
struct RepositorySpec {
    name: String,
    remote: String,
}

#[derive(Deserialize)]
struct MemorySpec {
    id: String,
    repository: String,
    role: String,
    title: String,
    summary: String,
    tags: Vec<String>,
    details: String,
    evidence: String,
}

impl MemorySpec {
    fn retrieval_input(&self) -> String {
        format!("{}\n{}\n{}", self.title, self.summary, self.tags.join(" "))
    }

    fn details_section(&self, instance: &str) -> String {
        format!(
            "## 详细说明\n\n{}\n\n样本实例：{instance}。这是人工受控验收语料。\n\n",
            self.details
        )
    }

    fn evidence_section(&self) -> String {
        format!("## 原文依据\n\n> {}\n", self.evidence)
    }

    fn body(&self, instance: &str) -> String {
        format!(
            "## 结论\n\n{}\n\n{}{}",
            self.summary,
            self.details_section(instance),
            self.evidence_section()
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuerySpec {
    id: String,
    repository: String,
    query: String,
    expected_ids: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedVector {
    input: String,
    vector: Vec<f32>,
    usage: ApiUsage,
    latency_ms: u64,
}

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/performance-acceptance/semantic")
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-corpus.json")
}

fn persist(path: &Path, value: &impl Serialize) {
    write_atomic(path, &serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn load_corpus() -> (Corpus, String) {
    let bytes = fs::read(corpus_path()).unwrap();
    let corpus: Corpus = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(corpus.version, 1);
    assert_eq!(corpus.repositories.len(), 3);
    assert_eq!(corpus.memories.len(), 48);
    assert_eq!(corpus.queries.len(), 12);
    let ids: BTreeSet<_> = corpus.memories.iter().map(|memory| &memory.id).collect();
    assert_eq!(ids.len(), corpus.memories.len());
    let inputs: BTreeSet<_> = corpus
        .memories
        .iter()
        .map(MemorySpec::retrieval_input)
        .collect();
    assert_eq!(inputs.len(), 48);
    for repository in &corpus.repositories {
        for (role, count) in [("gold", 6), ("distractor", 10)] {
            assert_eq!(
                corpus
                    .memories
                    .iter()
                    .filter(|memory| memory.repository == repository.name && memory.role == role)
                    .count(),
                count
            );
        }
        assert_eq!(
            corpus
                .queries
                .iter()
                .filter(|query| query.repository == repository.name)
                .count(),
            4
        );
    }
    for memory in &corpus.memories {
        assert!(!memory.evidence.is_empty());
        assert!(!memory.evidence.contains('\n'));
        assert_eq!(memory.summary.trim(), memory.summary);
        assert!(memory.summary.chars().count() <= 150);
    }
    for query in &corpus.queries {
        assert!(!query.expected_ids.is_empty());
        for id in &query.expected_ids {
            assert!(corpus.memories.iter().any(|memory| {
                memory.id == *id && memory.repository == query.repository && memory.role == "gold"
            }));
        }
    }
    (corpus, blake3::hash(&bytes).to_hex().to_string())
}

fn journal(run: &str, value: Value) {
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory().join("api-requests.jsonl"))
        .unwrap();
    let row = json!({"run":run,"at":chrono::Utc::now().to_rfc3339(),"request":value});
    writeln!(output, "{row}").unwrap();
    output.sync_data().unwrap();
}

async fn memory_vectors(
    corpus: &Corpus,
    service: &ServiceCredentials,
    run: &str,
) -> (BTreeMap<String, CachedVector>, Value) {
    let fingerprint = blake3::hash(service_key(service).as_bytes())
        .to_hex()
        .to_string();
    let cache = directory().join("cache").join(&fingerprint);
    fs::create_dir_all(&cache).unwrap();
    let mut vectors = BTreeMap::new();
    let mut new_requests = Vec::new();
    let mut reused = 0;
    for memory in &corpus.memories {
        let input = memory.retrieval_input();
        let input_hash = blake3::hash(input.as_bytes()).to_hex().to_string();
        let path = cache.join(format!("{input_hash}.json"));
        let cached = if path.exists() {
            let cached: CachedVector = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(cached.input, input);
            reused += 1;
            cached
        } else {
            journal(
                run,
                json!({"purpose":"memory-vector","inputHash":input_hash,"input":input,"status":"started"}),
            );
            let started = Instant::now();
            let result = match model_api::embed(service, &input).await {
                Ok(result) => result,
                Err(error) => {
                    journal(
                        run,
                        json!({"purpose":"memory-vector","inputHash":input_hash,"status":"error","wallMs":started.elapsed().as_secs_f64()*1000.0,"usageReported":false,"inputTokens":null,"error":error}),
                    );
                    panic!("真实向量请求失败，见脱敏的 api-requests.jsonl");
                }
            };
            let usage = json!({"inputHash":input_hash,"inputTokens":result.usage.reported.then_some(result.usage.input_tokens),"usageReported":result.usage.reported,"latencyMs":result.latency_ms,"wallMs":started.elapsed().as_secs_f64()*1000.0});
            journal(
                run,
                json!({"purpose":"memory-vector","status":"complete","result":usage}),
            );
            new_requests.push(usage);
            let cached = CachedVector {
                input: input.clone(),
                vector: result.vector,
                usage: result.usage,
                latency_ms: result.latency_ms,
            };
            persist(&path, &cached);
            println!("semantic memory vector {}: cached on disk", memory.id);
            cached
        };
        vectors.insert(input, cached);
    }
    let report = json!({"serviceFingerprintHash":fingerprint,"uniqueRetrievalInputs":vectors.len(),"reusedPersistentVectors":reused,"newApiRequests":new_requests});
    (vectors, report)
}

fn generate_vault(corpus: &Corpus, base: &Path, count: usize) -> (Vault, Value) {
    let root = base.join(format!("vault-{count}"));
    let source_path = base.join(format!("sources-{count}.jsonl"));
    fs::create_dir_all(base).unwrap();
    let mut sources = BufWriter::new(File::create(&source_path).unwrap());
    let mut vault = Vault::open(&root).unwrap();
    let mut gold_files = BTreeMap::new();
    for repository in &corpus.repositories {
        let repository_record = vault
            .add_repository(RepositoryInput {
                name: repository.name.clone(),
                remote: repository.remote.clone(),
            })
            .unwrap();
        let gold: Vec<_> = corpus
            .memories
            .iter()
            .filter(|memory| memory.repository == repository.name && memory.role == "gold")
            .collect();
        let distractors: Vec<_> = corpus
            .memories
            .iter()
            .filter(|memory| memory.repository == repository.name && memory.role == "distractor")
            .collect();
        for index in 0..count {
            let (memory, id) = if index < gold.len() {
                (gold[index], gold[index].id.clone())
            } else {
                let memory = distractors[(index - gold.len()) % distractors.len()];
                (memory, format!("{}-{index:05}", memory.id))
            };
            let source = format!("Codex · semantic-acceptance · {id}");
            let metadata = json!({"id":id,"repository_id":repository_record.id,"repository_name":repository.name,"repository_remote":repository.remote,"title":memory.title,"kind":"技术决策","tags":memory.tags,"updated_at":"2026-11-21T00:00:00Z","source":source,"related_ids":[]});
            let document = format!(
                "---\n{}---\n{}",
                serde_yaml_ng::to_string(&metadata).unwrap(),
                memory.body(&id)
            );
            let path = vault
                .root
                .join(&repository_record.id)
                .join(format!("{id}.md"));
            fs::write(&path, document).unwrap();
            if memory.role == "gold" {
                gold_files.insert(id.clone(), path.display().to_string());
            }
            let original = json!({"id":id,"sessionId":"semantic-acceptance","turnId":id,"repository":repository.name,"userText":format!("请确认人工验收实例 {id} 的技术决定：{}。", memory.title),"assistantText":format!("确认采用以下决定：{}",memory.evidence)});
            writeln!(sources, "{original}").unwrap();
        }
    }
    sources.flush().unwrap();
    let started = Instant::now();
    vault.refresh().unwrap();
    let refresh_ms = started.elapsed().as_secs_f64() * 1000.0;
    let workspace = vault.workspace().unwrap();
    assert!(workspace.warnings.is_empty(), "{:?}", workspace.warnings);
    assert_eq!(workspace.repositories.len(), 3);
    assert!(workspace
        .repositories
        .iter()
        .all(|repository| repository.memory_count == count));
    let manifest = json!({"perRepository":count,"totalMemories":count*3,"path":vault.root,"repositories":workspace.repositories,"goldFiles":gold_files,"sourceJsonl":source_path.canonicalize().unwrap(),"refreshMs":refresh_ms});
    (vault, manifest)
}

fn seed_embeddings(vault: &Vault, vectors: &BTreeMap<String, CachedVector>, key: &str) {
    let _pipeline = Pipeline::open(&vault.root).unwrap();
    let _engine = SearchEngine::open(&vault.root).unwrap();
    let memories = Connection::open(vault.root.join(".aimemory/index.sqlite")).unwrap();
    let mut pipeline = Connection::open(vault.root.join(".aimemory/pipeline.sqlite")).unwrap();
    let transaction = pipeline.transaction().unwrap();
    transaction
        .execute_batch("DELETE FROM embeddings; DELETE FROM query_vectors; DELETE FROM api_usage;")
        .unwrap();
    let mut statement = memories
        .prepare("SELECT id,title,summary,tags FROM memories ORDER BY id")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (id, title, summary, tags) = row.unwrap();
        let tags: Vec<String> = serde_json::from_str(&tags).unwrap();
        let input = format!("{title}\n{summary}\n{}", tags.join(" "));
        let cached = vectors
            .get(&input)
            .expect("SQLite 检索文本与真实向量输入必须逐字一致");
        let hash = blake3::hash(input.as_bytes()).to_hex().to_string();
        let bytes: Vec<u8> = cached
            .vector
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        transaction.execute("INSERT INTO embeddings(memory_id,input_hash,service_key,content,status,vector,dimensions) VALUES(?1,?2,?3,?4,'ready',?5,?6)",params![id,hash,key,input,bytes,cached.vector.len() as i64]).unwrap();
    }
    transaction.commit().unwrap();
}

fn query_usage(root: &Path) -> Value {
    let db = Connection::open(root.join(".aimemory/pipeline.sqlite")).unwrap();
    let (count, tokens): (i64, i64) = db
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(input_tokens),0) FROM api_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    json!({"requests":count,"reportedInputTokens":tokens})
}

async fn search(
    engine: &SearchEngine,
    query: &QuerySpec,
    repository: &Repository,
    service: &ServiceCredentials,
    run: &str,
    phase: &str,
    count: usize,
) -> (SearchResponse, Value) {
    if phase == "first" {
        journal(
            run,
            json!({"purpose":"query-vector","scale":count,"queryId":query.id,"input":query.query,"status":"started"}),
        );
    }
    let started = Instant::now();
    let response = match engine
        .search(
            SearchRequest {
                mode: Default::default(),
                group_of: None,
                offset: None,
                query: query.query.clone(),
                repository_id: Some(repository.id.clone()),
                limit: None,
            },
            service,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            journal(
                run,
                json!({"purpose":"query-vector","scale":count,"queryId":query.id,"phase":phase,"status":"error","wallMs":started.elapsed().as_secs_f64()*1000.0,"usageReported":false,"inputTokens":null,"error":error}),
            );
            panic!("检索失败，见脱敏的 api-requests.jsonl");
        }
    };
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(response.results.len(), 5);
    assert_eq!(response.pending_embeddings, 0);
    assert!(response
        .results
        .iter()
        .all(|hit| hit.memory.repository_id == repository.id));
    let serialized = serde_json::to_value(&response).unwrap();
    assert!(serialized["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.get("markdown").is_none()));
    let payload = json!({"results":response.results.iter().map(|hit|json!({"id":hit.memory.id,"title":hit.memory.title,"summary":hit.memory.summary,"repository":repository.name,"score":hit.score})).collect::<Vec<_>>()});
    let sample = json!({"wallMs":wall_ms,"localSearchMs":response.local_search_ms,"embeddingLatencyMs":response.embedding_latency_ms,"embeddingInputTokens":response.query_usage_reported.then_some(response.embedding_input_tokens),"queryUsageReported":response.query_usage_reported,"queryVectorCached":response.query_vector_cached,"summaryToolPayloadUtf8Bytes":serde_json::to_vec(&payload).unwrap().len(),"toolOutputTokens":null});
    if !response.query_vector_cached {
        journal(
            run,
            json!({"purpose":"query-vector","scale":count,"queryId":query.id,"phase":phase,"status":"complete","result":sample}),
        );
    }
    (response, sample)
}

fn verify_reads(corpus: &Corpus, vault: &Vault, source_path: &Path) -> Vec<Value> {
    let sources: BTreeMap<String, Value> = BufReader::new(File::open(source_path).unwrap())
        .lines()
        .map(|line| serde_json::from_str::<Value>(&line.unwrap()).unwrap())
        .filter(|source| source["id"].as_str().unwrap().starts_with("G-"))
        .map(|source| (source["id"].as_str().unwrap().to_owned(), source))
        .collect();
    let before = query_usage(&vault.root);
    let mut reports = Vec::new();
    for memory in corpus
        .memories
        .iter()
        .filter(|memory| memory.role == "gold")
    {
        assert!(sources[&memory.id]["assistantText"]
            .as_str()
            .unwrap()
            .contains(&memory.evidence));
        let started = Instant::now();
        let sections = mcp::list_sections(vault, &memory.id).unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            sections
                .sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["结论", "详细说明", "原文依据"]
        );
        let mut reads = vec![
            json!({"mode":"list-sections","wallMs":elapsed,"utf8Bytes":serde_json::to_vec(&sections).unwrap().len(),"toolOutputTokens":null}),
        ];
        for (mode_name, mode, section, expected) in [
            (
                "section-default",
                ReadMode::Section,
                None,
                format!("## 结论\n\n{}\n\n", memory.summary),
            ),
            (
                "section-detail",
                ReadMode::Section,
                Some("详细说明".into()),
                memory.details_section(&memory.id),
            ),
            (
                "evidence",
                ReadMode::Evidence,
                None,
                memory.evidence_section(),
            ),
            ("full", ReadMode::Full, None, memory.body(&memory.id)),
        ] {
            let started = Instant::now();
            let result = mcp::read_memory(
                vault,
                ReadMemoryRequest {
                    source_id: None,
                    id: memory.id.clone(),
                    mode,
                    section,
                },
            )
            .unwrap();
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(result.markdown, expected);
            if mode_name == "section-default" {
                assert_eq!(result.source.as_deref(), Some("人工内容"));
            } else {
                assert_eq!(
                    result.source.as_deref(),
                    Some(format!("Codex · semantic-acceptance · {}", memory.id).as_str())
                );
            }
            if mode_name == "full" {
                let record = vault.read(&memory.id).unwrap();
                let raw = fs::read_to_string(&record.summary.path).unwrap();
                assert_eq!(raw.split_once("\n---\n").unwrap().1, result.markdown);
            }
            reads.push(json!({"mode":mode_name,"wallMs":wall_ms,"utf8Bytes":serde_json::to_vec(&result).unwrap().len(),"markdownUtf8Bytes":result.markdown.len(),"toolOutputTokens":null}));
        }
        reports.push(json!({"id":memory.id,"repository":memory.repository,"sourceExact":true,"markdownExact":true,"reads":reads}));
    }
    assert_eq!(before, query_usage(&vault.root));
    reports
}

#[tokio::test]
#[ignore = "AIMEMORY_LIVE_PERFORMANCE=1 才向已配置向量 API 发送固定人工语料"]
async fn real_semantic_recall_and_exact_evidence() {
    assert_eq!(env::var("AIMEMORY_LIVE_PERFORMANCE").as_deref(), Ok("1"));
    let (corpus, corpus_hash) = load_corpus();
    let data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data");
    let settings = SettingsStore::load(&data_dir, codex::default_codex_directory()).unwrap();
    let service = settings.credentials(ServiceKind::Embedding).unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let (vectors, vector_report) = memory_vectors(&corpus, &service, &run).await;
    let mut manifests = Vec::new();
    let mut scales = Vec::new();
    for count in [1_000, 10_000] {
        let (vault, manifest) = generate_vault(&corpus, &directory(), count);
        seed_embeddings(&vault, &vectors, &service_key(&service));
        manifests.push(manifest.clone());
        persist(
            &directory().join("manifest.json"),
            &json!({"run":run,"corpusHash":corpus_hash,"corpusSource":corpus_path(),"vaults":manifests}),
        );
        let repositories = vault.workspace().unwrap().repositories;
        let mut engine = SearchEngine::open(&vault.root).unwrap();
        let mut cases = Vec::new();
        for query in &corpus.queries {
            let repository = repositories
                .iter()
                .find(|repository| repository.name == query.repository)
                .unwrap();
            let (first, first_sample) =
                search(&engine, query, repository, &service, &run, "first", count).await;
            assert!(!first.query_vector_cached);
            let usage = query_usage(&vault.root);
            let (repeated, repeated_sample) = search(
                &engine, query, repository, &service, &run, "repeated", count,
            )
            .await;
            assert!(repeated.query_vector_cached);
            assert_eq!(repeated.embedding_input_tokens, 0);
            assert_eq!(
                first
                    .results
                    .iter()
                    .map(|hit| &hit.memory.id)
                    .collect::<Vec<_>>(),
                repeated
                    .results
                    .iter()
                    .map(|hit| &hit.memory.id)
                    .collect::<Vec<_>>()
            );
            let other = repositories
                .iter()
                .find(|candidate| candidate.id != repository.id)
                .unwrap();
            let (cross, cross_sample) = search(
                &engine,
                query,
                other,
                &service,
                &run,
                "cross-repository",
                count,
            )
            .await;
            assert!(cross.query_vector_cached);
            assert_eq!(cross.embedding_input_tokens, 0);
            drop(engine);
            engine = SearchEngine::open(&vault.root).unwrap();
            let (reopened, reopened_sample) = search(
                &engine, query, repository, &service, &run, "reopened", count,
            )
            .await;
            assert!(reopened.query_vector_cached);
            assert_eq!(reopened.embedding_input_tokens, 0);
            assert_eq!(usage, query_usage(&vault.root));
            let rank = first
                .results
                .iter()
                .position(|hit| query.expected_ids.contains(&hit.memory.id))
                .map(|index| index + 1);
            let hits = query
                .expected_ids
                .iter()
                .filter(|id| first.results.iter().any(|hit| hit.memory.id == **id))
                .count();
            cases.push(json!({"id":query.id,"repository":query.repository,"query":query.query,"expectedIds":query.expected_ids,"ranking":first.results.iter().map(|hit|json!({"id":hit.memory.id,"score":hit.score})).collect::<Vec<_>>(),"rank":rank,"recallAt5":hits as f64/query.expected_ids.len() as f64,"top1":rank==Some(1),"reciprocalRank":rank.map(|rank|1.0/rank as f64).unwrap_or(0.0),"first":first_sample,"repeated":repeated_sample,"crossRepository":cross_sample,"reopened":reopened_sample}));
            persist(&directory().join(format!("queries-{count}.json")), &cases);
            println!(
                "semantic {count}/repo {}: expected rank {:?}",
                query.id, rank
            );
        }
        assert_eq!(query_usage(&vault.root)["requests"], 12);
        let mean = |field: &str| {
            cases
                .iter()
                .map(|case| case[field].as_f64().unwrap())
                .sum::<f64>()
                / cases.len() as f64
        };
        let metrics = json!({"cases":cases.len(),"recallAt5":mean("recallAt5"),"top1":cases.iter().filter(|case|case["top1"]==true).count() as f64/cases.len() as f64,"mrrAt5":mean("reciprocalRank")});
        let reads = verify_reads(
            &corpus,
            &vault,
            Path::new(manifest["sourceJsonl"].as_str().unwrap()),
        );
        scales.push(json!({"perRepository":count,"totalMemories":count*3,"metrics":metrics,"queries":cases,"reads":reads,"queryApiUsage":query_usage(&vault.root)}));
        persist(
            &directory().join("report.json"),
            &json!({"run":run,"corpusHash":corpus_hash,"model":service.model,"limitations":[corpus.description,"磁盘冷快照并非操作系统文件缓存冷启动；本测试由产品 SearchEngine 和 MCP 本地读取函数执行，stdio协议另验。","首次查询清空本测试生成库的查询缓存；重复、跨仓和重开引擎复用查询向量。工具输出未使用 tokenizer，token值为null而非0。","MRR只观察前五条，报告为MRR@5。未根据返回结果修改题目或期望答案；不预设模型召回通过线。"],"memoryVectors":vector_report,"scales":scales}),
        );
    }
}

#[test]
#[ignore = "离线生成三仓各 1,000/10,000 条 Markdown 与 SQLite，不读取凭据或调用网络"]
fn prepare_semantic_fixtures() {
    let (corpus, corpus_hash) = load_corpus();
    let run = uuid::Uuid::new_v4().to_string();
    let mut manifests = Vec::new();
    let mut checks = Vec::new();
    for count in [1_000, 10_000] {
        let (vault, mut manifest) = generate_vault(&corpus, &directory(), count);
        let _pipeline = Pipeline::open(&vault.root).unwrap();
        let db = Connection::open(vault.root.join(".aimemory/pipeline.sqlite")).unwrap();
        let embeddings: i64 = db
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(embeddings, 0, "离线准备库应尚未生成向量");
        manifest["embeddingStatus"] = json!("unconfigured");
        let reads = verify_reads(
            &corpus,
            &vault,
            Path::new(manifest["sourceJsonl"].as_str().unwrap()),
        );
        checks.push(json!({"perRepository":count,"goldMemoriesChecked":reads.len(),"reads":reads}));
        manifests.push(manifest);
        persist(
            &directory().join("manifest.json"),
            &json!({"run":run,"stage":"offline-unembedded","corpusHash":corpus_hash,"corpusSource":corpus_path(),"apiRequests":0,"vectorsGenerated":0,"vaults":manifests}),
        );
        persist(
            &directory().join("prepare-report.json"),
            &json!({"run":run,"stage":"offline-unembedded","corpusHash":corpus_hash,"apiRequests":0,"vectorsGenerated":0,"scales":checks}),
        );
        println!(
            "offline semantic fixtures: {count}/repo, 18 gold memories verified, no embeddings"
        );
    }
}

#[test]
fn controlled_source_matches_real_markdown_and_sqlite() {
    let (corpus, _) = load_corpus();
    let root = directory().join("offline-checks");
    fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    let (vault, manifest) = generate_vault(&corpus, temporary.path(), 7);
    let templates: BTreeSet<_> = corpus
        .memories
        .iter()
        .map(MemorySpec::retrieval_input)
        .collect();
    let memories = vault.search(None, "").unwrap();
    assert_eq!(memories.len(), 21);
    for memory in memories {
        let input = format!(
            "{}\n{}\n{}",
            memory.title,
            memory.summary,
            memory.tags.join(" ")
        );
        assert!(templates.contains(&input));
    }
    let _pipeline = Pipeline::open(&vault.root).unwrap();
    let reads = verify_reads(
        &corpus,
        &vault,
        Path::new(manifest["sourceJsonl"].as_str().unwrap()),
    );
    assert_eq!(reads.len(), 18);
}
