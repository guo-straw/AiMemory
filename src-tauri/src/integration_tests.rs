use crate::{
    codex::{read_rollout, RolloutCursor},
    config::ServiceCredentials,
    mcp::{read_memory, ReadMemoryRequest, ReadMode},
    model_api::{chat_extract, embed, tests::serve_responses},
    pipeline::Pipeline,
    storage::Vault,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[tokio::test]
async fn codex_turn_writes_markdown_before_embedding_and_retries_without_another_chat_call() {
    let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dev-data/performance-acceptance/archive-idempotency");
    let fixture = test_root
        .join("runs")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&fixture).unwrap();
    let rollout = fixture.join("rollout-archive.jsonl");
    let events = [
        json!({"timestamp":"2026-11-21T03:00:00Z","type":"session_meta","payload":{"id":"integration-session","thread_source":"user","cwd":"/example/project","git":{"repository_url":"https://github.com/aimemory-example/project.git"}}}),
        json!({"timestamp":"2026-11-21T03:01:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1","root_turn_id":"turn-1"}}),
        json!({"timestamp":"2026-11-21T03:01:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"任务状态每两秒查询一次。"}]}}),
        json!({"timestamp":"2026-11-21T03:01:02Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"任务进入终态后停止查询。另一个导出需求已支持 CSV 文件。"}]}}),
        json!({"timestamp":"2026-11-21T03:01:03Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","root_turn_id":"turn-1","last_agent_message":"任务进入终态后停止查询。另一个导出需求已支持 CSV 文件。"}}),
    ];
    fs::write(
        &rollout,
        events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let batch = read_rollout(&rollout, &RolloutCursor::default(), "2026-11-21T03:00:00Z").unwrap();
    assert_eq!(batch.turns.len(), 1);

    let extraction = json!({"memories":[
        {"op":"create","title":"任务状态改用短轮询", "kind":"技术决策", "summary":"每两秒查询一次，终态后停止。", "details":"按任务状态判断轮询停止条件。", "evidence":"任务状态每两秒查询一次。", "tags":["轮询"]},
        {"op":"create","title":"导出 CSV", "kind":"工作记录", "summary":"导出功能支持 CSV。", "details":"使用 CSV 文件交换表格。", "evidence":"另一个导出需求已支持 CSV 文件。", "tags":["导出"]}
    ]});
    let api = CountedApi::start(vec![
        (
            200,
            json!({"choices":[{"finish_reason":"stop","message":{"content":extraction.to_string()}}],"usage":{"prompt_tokens":100,"completion_tokens":80}}),
        ),
        (503, json!({"error":{"message":"暂不可用"}})),
        (
            200,
            json!({"data":[{"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":12,"total_tokens":12}}),
        ),
        (
            200,
            json!({"data":[{"embedding":[0.4,0.5,0.6]}],"usage":{"prompt_tokens":14,"total_tokens":14}}),
        ),
    ]);
    let service = &api.service;
    let vault_path = fixture.join("vault");
    let mut vault = Vault::open(&vault_path).unwrap();
    let mut pipeline = Pipeline::open(&vault_path).unwrap();
    pipeline
        .ingest(&rollout, &batch.cursor, &batch.turns)
        .unwrap();
    let turn = pipeline.next_turn().unwrap().unwrap();
    pipeline.mark_extracting(&turn.source_id).unwrap();
    let topics = pipeline.archive_topics(&turn).unwrap();
    let result = chat_extract(service, &turn, &topics).await.unwrap();
    pipeline
        .extraction_complete(&turn.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    pipeline
        .sync_embeddings(&vault, "test-embedding-service", true)
        .unwrap();

    let written = vault.search(None, "").unwrap();
    assert_eq!(written.len(), 2);
    let contents = written
        .iter()
        .map(|memory| fs::read_to_string(&memory.path).unwrap())
        .collect::<Vec<_>>();
    assert!(contents
        .iter()
        .all(|body| body.contains("codex_turn_id: turn-1")));
    assert!(contents
        .iter()
        .any(|body| body.contains("> 任务状态每两秒查询一次。")));
    assert!(contents
        .iter()
        .any(|body| body.contains("> 另一个导出需求已支持 CSV 文件。")));
    let status = pipeline
        .status(true, Some("2026-11-21T03:00:00Z".into()), false, None)
        .unwrap();
    assert_eq!(status.written_memories, 2);
    assert_eq!(status.pending_embeddings, 2);
    assert_eq!(status.embedding_input_tokens, 0);
    assert_eq!(api.requests(), vec!["/v1/chat/completions"]);
    let before_embedding = archive_snapshot(&vault, &pipeline);

    let job = pipeline.next_embedding().unwrap().unwrap();
    pipeline
        .embedding_complete(&job, embed(service, &job.content).await)
        .unwrap();
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.failed_embeddings, 1);
    assert_eq!(vault.search(None, "").unwrap().len(), 2);
    assert_eq!(api.requests().len(), 2);

    pipeline.retry_failed().unwrap();
    while let Some(job) = pipeline.next_embedding().unwrap() {
        pipeline
            .embedding_complete(&job, embed(service, &job.content).await)
            .unwrap();
    }
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.pending_embeddings, 0);
    assert_eq!(status.failed_embeddings, 0);
    assert_eq!(status.chat_input_tokens, 100);
    assert_eq!(status.chat_output_tokens, 80);
    assert_eq!(status.embedding_input_tokens, 26);
    assert!(status.usage_incomplete);
    assert_eq!(
        api.requests(),
        vec![
            "/v1/chat/completions",
            "/v1/embeddings",
            "/v1/embeddings",
            "/v1/embeddings"
        ]
    );
    let saved = archive_snapshot(&vault, &pipeline);
    assert_eq!(before_embedding["files"], saved["files"]);
    assert!(saved["embeddings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|embedding| embedding["status"] == "ready" && embedding["dimensions"] == 3));

    let cursor = pipeline.cursors().unwrap().remove(&rollout).unwrap();
    let incremental = read_rollout(&rollout, &cursor, "2026-11-21T03:00:00Z").unwrap();
    assert!(incremental.turns.is_empty());
    pipeline
        .ingest(&rollout, &incremental.cursor, &incremental.turns)
        .unwrap();
    assert_eq!(
        run_ready_jobs(&mut vault, &mut pipeline, service).await,
        (0, 0)
    );
    assert_eq!(archive_snapshot(&vault, &pipeline), saved);
    assert_eq!(api.requests().len(), 4);

    let replay = read_rollout(&rollout, &RolloutCursor::default(), "2026-11-21T03:00:00Z").unwrap();
    assert_eq!(replay.turns.len(), 1);
    pipeline
        .ingest(&rollout, &replay.cursor, &replay.turns)
        .unwrap();
    assert_eq!(
        run_ready_jobs(&mut vault, &mut pipeline, service).await,
        (0, 0)
    );
    assert_eq!(archive_snapshot(&vault, &pipeline), saved);
    assert_eq!(api.requests().len(), 4);
    drop(pipeline);
    drop(vault);
    let mut vault = Vault::open(&vault_path).unwrap();
    let mut pipeline = Pipeline::open(&vault_path).unwrap();
    pipeline.recover_interrupted().unwrap();
    let cursor = pipeline.cursors().unwrap().remove(&rollout).unwrap();
    let incremental = read_rollout(&rollout, &cursor, "2026-11-21T03:00:00Z").unwrap();
    assert!(incremental.turns.is_empty());
    pipeline
        .ingest(&rollout, &incremental.cursor, &incremental.turns)
        .unwrap();
    pipeline
        .ingest(&rollout, &replay.cursor, &replay.turns)
        .unwrap();
    assert_eq!(
        run_ready_jobs(&mut vault, &mut pipeline, service).await,
        (0, 0)
    );
    assert_eq!(vault.search(None, "").unwrap().len(), 2);
    assert!(pipeline.next_turn().unwrap().is_none());
    assert!(pipeline.next_embedding().unwrap().is_none());
    assert_eq!(archive_snapshot(&vault, &pipeline), saved);
    assert_eq!(api.requests().len(), 4);

    let report = json!({
        "passed":true,
        "test":"integration_tests::codex_turn_writes_markdown_before_embedding_and_retries_without_another_chat_call",
        "transport":"counted localhost HTTP; no remote provider or real credentials",
        "workerScope":"production Codex reader, Pipeline and model API client; queued vector jobs consumed asynchronously, without native timer loops",
        "fixtureDirectory":fixture,
        "sourceTurns":1,
        "writtenMemories":2,
        "httpRequests":{"total":4,"chat":1,"embedding":3,"embeddingFailures":1},
        "knownUsage":{"chatInput":100,"chatOutput":80,"embeddingInput":26},
        "usageOrigin":"fixed localhost response fixtures; verifies accounting, not remote measured tokens",
        "failureUsage":"503 did not report tokens; usageIncomplete remains true",
        "beforeEmbedding":before_embedding,
        "saved":saved,
        "repeatChecks":[
            {"case":"persisted JSONL cursor", "newHttpRequests":0,"newTokens":0,"stateUnchanged":true},
            {"case":"same source replayed from JSONL start", "newHttpRequests":0,"newTokens":0,"stateUnchanged":true},
            {"case":"reopen SQLite/vault, recover, cursor scan and source replay", "newHttpRequests":0,"newTokens":0,"stateUnchanged":true}
        ]
    });
    let report_json = format!("{}\n", serde_json::to_string_pretty(&report).unwrap());
    fs::write(fixture.join("report.json"), &report_json).unwrap();
    fs::write(test_root.join("report.json"), &report_json).unwrap();
    println!("archive idempotency acceptance: {}", fixture.display());
}

fn write_rollout(
    directory: &Path,
    session: &str,
    owner: &str,
    turns: &[(u8, &str, &str)],
) -> PathBuf {
    let path = directory.join(format!("rollout-{session}.jsonl"));
    let mut file = fs::File::create(&path).unwrap();
    let meta = json!({"timestamp":"2026-11-21T03:00:00Z", "type":"session_meta", "payload":{
        "id":session, "thread_source":"user", "cwd":"/example/desktop",
        "git":{"repository_url":format!("https://github.com/{owner}/desktop.git")}
    }});
    writeln!(file, "{meta}").unwrap();
    for (number, request, reply) in turns {
        let at = format!("2026-11-21T03:{number:02}:00Z");
        let turn = format!("turn-{number}");
        for event in [
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_started","turn_id":turn,"root_turn_id":turn}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":request}]}}),
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"root_turn_id":turn,"last_agent_message":reply}}),
        ] {
            writeln!(file, "{event}").unwrap();
        }
    }
    path
}

#[tokio::test]
async fn archive_groups_follow_ups_and_reuses_topics_only_in_the_same_repository() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("vault");
    let original = [
        (1, "开放应用检查更新。", "设置页已支持检查正式版。"),
        (2, "能直接下载吗？", "应用内可以直接下载安装包。"),
        (3, "好。", "好的。"),
        (4, "另外改一下记忆归档。", "同一需求的记忆会归在一起。"),
        (
            5,
            "刚才的下载适配哪些 Mac？",
            "下载会区分 Apple 芯片和 Intel Mac。",
        ),
    ];
    let separate = [(6, "这个仓库也加检查更新。", "另一个应用的发布源设为内网。")];
    let resumed = [(7, "延续之前的安装包下载。", "下载完成可以在文件夹中显示。")];
    let paths = [
        write_rollout(directory.path(), "main-session", "first-owner", &original),
        write_rollout(directory.path(), "other-project", "other-owner", &separate),
        write_rollout(directory.path(), "next-session", "first-owner", &resumed),
    ];
    let mut vault = Vault::open(&root).unwrap();
    let mut pipeline = Pipeline::open(&root).unwrap();
    let baseline = "2026-11-21T03:00:00Z";
    for path in &paths {
        let batch = read_rollout(path, &RolloutCursor::default(), baseline).unwrap();
        pipeline.ingest(path, &batch.cursor, &batch.turns).unwrap();
    }

    let mut update_id = String::new();
    let mut archive_id = String::new();
    let mut update_summary = "";
    for (number, _, reply) in original.iter().chain(&separate).chain(&resumed) {
        let turn = pipeline.next_turn().unwrap().unwrap();
        assert_eq!(turn.turn_id, format!("turn-{number}"));
        let topics = pipeline.archive_topics(&turn).unwrap();
        pipeline.mark_extracting(&turn.source_id).unwrap();
        let (title, target) = match number {
            2 | 5 | 7 => ("应用更新", Some(&update_id)),
            4 => ("按需求归档", None),
            _ => ("应用更新", None),
        };
        let summary = match number {
            2 => "设置支持检查正式版和直接下载安装包。",
            5 => "设置支持检查正式版，下载时区分 Apple 芯片和 Intel Mac。",
            7 => "设置可检查正式版、按 Mac 芯片下载安装包，下载后可定位文件。",
            _ => reply,
        };
        let memories = if *number == 3 {
            vec![]
        } else {
            vec![
                json!({"op":if target.is_some() { "update" } else { "create" },"target_id":target, "title":title, "kind":"工作记录",
                "summary":summary, "details":reply, "evidence":reply, "tags":[]}),
            ]
        };
        let (service, requests) = serve_responses(vec![(
            200,
            json!({
                "choices":[{"message":{"content":json!({"memories":memories}).to_string()}}],
                "usage":{"prompt_tokens":10,"completion_tokens":2}
            }),
        )]);
        let extraction = chat_extract(&service, &turn, &topics).await.unwrap();
        let request = requests.recv().unwrap();
        let material: Value =
            serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
        let mut ids: Vec<_> = material["topics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["can_update"].as_bool() == Some(true))
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        let mut expected = match number {
            1 | 6 => vec![],
            2..=4 => vec![update_id.as_str()],
            _ => vec![update_id.as_str(), archive_id.as_str()],
        };
        expected.sort_unstable();
        assert_eq!(ids, expected, "wrong candidates for turn {number}");
        if target.is_some() {
            let update = topics.iter().find(|topic| topic.id == update_id).unwrap();
            assert_eq!(update.summary, update_summary);
        }
        pipeline
            .extraction_complete(&turn.source_id, &extraction)
            .unwrap();
        pipeline.materialize(&mut vault).unwrap();
        if *number == 1 {
            update_id = vault.search(None, "应用更新").unwrap()[0].id.clone();
        } else if *number == 4 {
            archive_id = vault.search(None, "按需求归档").unwrap()[0].id.clone();
        }
        if matches!(number, 1 | 2 | 5 | 7) {
            update_summary = summary;
        }
        pipeline
            .sync_embeddings(&vault, "test-embedding-service", true)
            .unwrap();
    }

    let evidence = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: update_id.clone(),
            mode: ReadMode::Evidence,
            section: None,
        },
    )
    .unwrap()
    .markdown;
    for reply in [original[0].2, original[1].2, original[4].2, resumed[0].2] {
        assert_eq!(
            evidence.matches(reply).count(),
            1,
            "missing or repeated evidence: {reply}"
        );
    }
    for reply in [original[2].2, original[3].2, separate[0].2] {
        assert!(!evidence.contains(reply));
    }
    let files = vault.search(None, "").unwrap();
    assert_eq!(files.len(), 3);
    let update = vault.read(&update_id).unwrap();
    assert_eq!(update.summary.summary, update_summary);
    let source = fs::read_to_string(&update.summary.path).unwrap();
    for turn in ["turn-1", "turn-2", "turn-5", "turn-7"] {
        assert!(source.contains(turn), "missing turn attribution: {turn}");
    }
    assert!(source.contains("main-session") && source.contains("next-session"));
    assert_ne!(update_id, archive_id);
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.processed_turns, 7);
    assert_eq!(status.written_memories, 3);
    assert_eq!(status.memory_writes, 6);
    assert_eq!(status.chat_input_tokens, 70);
    assert_eq!(status.chat_output_tokens, 14);
    assert_eq!(status.pending_embeddings, 3);
    let vector = json!({"data":[{"embedding":[0.1,0.2]}],"usage":{"prompt_tokens":6}});
    let api = CountedApi::start(vec![(200, vector); 3]);
    assert_eq!(
        run_ready_jobs(&mut vault, &mut pipeline, &api.service).await,
        (0, 3)
    );
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .embedding_input_tokens,
        18
    );
    let saved = archive_snapshot(&vault, &pipeline);
    drop(pipeline);
    drop(vault);
    let mut vault = Vault::open(&root).unwrap();
    let mut pipeline = Pipeline::open(&root).unwrap();
    pipeline.recover_interrupted().unwrap();
    for path in &paths {
        let replay = read_rollout(path, &RolloutCursor::default(), baseline).unwrap();
        pipeline
            .ingest(path, &replay.cursor, &replay.turns)
            .unwrap();
    }
    assert_eq!(
        run_ready_jobs(&mut vault, &mut pipeline, &api.service).await,
        (0, 0)
    );
    assert_eq!(archive_snapshot(&vault, &pipeline), saved);
    assert_eq!(api.requests(), vec!["/v1/embeddings"; 3]);
}

async fn run_ready_jobs(
    vault: &mut Vault,
    pipeline: &mut Pipeline,
    service: &ServiceCredentials,
) -> (usize, usize) {
    let mut chats = 0;
    while let Some(turn) = pipeline.next_turn().unwrap() {
        pipeline.mark_extracting(&turn.source_id).unwrap();
        let topics = pipeline.archive_topics(&turn).unwrap();
        let extraction = chat_extract(service, &turn, &topics).await.unwrap();
        pipeline
            .extraction_complete(&turn.source_id, &extraction)
            .unwrap();
        pipeline.materialize(vault).unwrap();
        chats += 1;
    }
    pipeline
        .sync_embeddings(vault, "test-embedding-service", true)
        .unwrap();
    let mut embeddings = 0;
    while let Some(job) = pipeline.next_embedding().unwrap() {
        pipeline
            .embedding_complete(&job, embed(service, &job.content).await)
            .unwrap();
        embeddings += 1;
    }
    (chats, embeddings)
}

fn archive_snapshot(vault: &Vault, pipeline: &Pipeline) -> Value {
    let db = rusqlite::Connection::open_with_flags(
        pipeline.root.join(".aimemory/pipeline.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let embeddings = db.prepare("SELECT memory_id,input_hash,service_key,status,hex(vector),dimensions FROM embeddings ORDER BY memory_id").unwrap()
        .query_map([], |row| Ok(json!({
            "id":row.get::<_, String>(0)?,"inputHash":row.get::<_, String>(1)?,
            "serviceKey":row.get::<_, String>(2)?,"status":row.get::<_, String>(3)?,
            "vectorHex":row.get::<_, String>(4)?,"dimensions":row.get::<_, Option<i64>>(5)?
        }))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    let usage = db.prepare("SELECT request_id,input_tokens,latency FROM api_usage ORDER BY request_id").unwrap()
        .query_map([], |row| Ok(json!({
            "requestId":row.get::<_, String>(0)?,"inputTokens":row.get::<_, i64>(1)?,"latencyMs":row.get::<_, i64>(2)?
        }))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    let mut memories = vault.search(None, "").unwrap();
    memories.sort_by(|a, b| a.id.cmp(&b.id));
    let files = memories.iter().map(|memory| {
        let bytes = fs::read(&memory.path).unwrap();
        json!({
            "id":memory.id,"path":memory.path,
            "rawMarkdown":String::from_utf8(bytes.clone()).unwrap(),
            "blake3":blake3::hash(&bytes).to_hex().to_string(),
            "modified":fs::metadata(&memory.path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos().to_string()
        })
    }).collect::<Vec<_>>();
    json!({
        "status":pipeline.status(true,Some("2026-11-21T03:00:00Z".into()),false,None).unwrap(),
        "files":files,"embeddings":embeddings,"apiUsage":usage,"cursors":pipeline.cursors().unwrap()
    })
}

struct CountedApi {
    service: ServiceCredentials,
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CountedApi {
    fn start(responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let mut responses: VecDeque<_> = responses.into();
        let thread = thread::spawn(move || {
            for connection in listener.incoming() {
                let mut stream = connection.unwrap();
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let path = first.split_whitespace().nth(1).unwrap().to_string();
                let mut content_length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = header.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                serde_json::from_slice::<Value>(&body).unwrap();
                received.lock().unwrap().push(path);
                let (status, response) = responses.pop_front().unwrap_or_else(|| {
                    (
                        500,
                        json!({"error":{"message":"unexpected additional model call"}}),
                    )
                });
                let response = response.to_string();
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            }
        });
        Self {
            service: ServiceCredentials {
                base_url: format!("http://{address}/v1"),
                model: "acceptance-only".into(),
                api_key: String::new(),
            },
            address,
            requests,
            stopped,
            thread: Some(thread),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for CountedApi {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        TcpStream::connect(self.address).unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}
