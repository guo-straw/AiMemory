use super::*;
use crate::{
    codex::{CodexTurn, RolloutCursor},
    config::ServiceKind,
    model_api::{self, ApiUsage, ChatExtraction, EmbeddingResult},
    search::{service_key, SearchEngine},
    storage::{ArchiveSource, MemoryInput},
};
use std::collections::BTreeMap;

async fn vector(
    service: &crate::config::ServiceCredentials,
    input: &str,
    cache: &Path,
    requests: &mut Vec<Value>,
    purpose: &str,
) -> EmbeddingResult {
    let hash = blake3::hash(format!("{}\n{input}", service_key(service)).as_bytes())
        .to_hex()
        .to_string();
    let path = cache.join(format!("{hash}.json"));
    if path.exists() {
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        requests.push(json!({"purpose":purpose,"hash":hash,"cached":true,"inputTokens":0}));
        return EmbeddingResult {
            vector: serde_json::from_value(value["vector"].clone()).unwrap(),
            usage: ApiUsage {
                input_tokens: 0,
                output_tokens: 0,
                reported: true,
            },
            latency_ms: 0,
        };
    }
    let result = model_api::embed(service, input).await.unwrap();
    fs::write(path, serde_json::to_vec(&result).unwrap()).unwrap();
    requests.push(json!({"purpose":purpose,"hash":hash,"cached":false,"inputTokens":result.usage.input_tokens,"usageReported":result.usage.reported,"latencyMs":result.latency_ms}));
    result
}

async fn archive_acceptance(
    project: &Path,
    run: &Path,
    cache: &Path,
    settings: &SettingsStore,
) -> Value {
    let service = settings.credentials(ServiceKind::Chat).unwrap();
    let root = run.join("archive-vault");
    let mut vault = Vault::open(&root).unwrap();
    let mut pipeline = Pipeline::open(&root).unwrap();
    let materials = [
        (
            "导出进度需求已经确定：通过 SSE 推送状态。",
            "导出任务采用 SSE 推送状态。",
        ),
        (
            "同一导出需求有变更：旧代理会缓冲 SSE，改为每两秒轮询。",
            "已确定导出进度改为两秒轮询，因为旧代理会缓冲 SSE。",
        ),
        (
            "继续导出进度这一项：终态或离开页面时停止轮询。",
            "导出进度在终态和离开页面时停止计时器。",
        ),
        ("好的。", "好的。"),
        (
            "另一个独立需求：头像上传支持 PNG。",
            "头像上传范围已经确定为支持 PNG 格式。",
        ),
    ];
    let mut id = None::<String>;
    let mut cases = Vec::new();
    let mut actual_requests = 0;
    let mut effective = 0;
    let mut manual = String::new();
    for (index, (user, assistant)) in materials.iter().enumerate() {
        let turn = CodexTurn {
            source_id: format!("live-archive:{index}"),
            session_id: "live-archive".into(),
            turn_id: format!("turn-{index}"),
            completed_at: format!("2026-11-21T10:00:0{index}Z"),
            cwd: project.display().to_string(),
            repository_remote: Some("https://github.com/aimemory-holdout/archive.git".into()),
            repository_name: "archive".into(),
            user_text: (*user).into(),
            assistant_text: (*assistant).into(),
            session_path: root.join("turns.jsonl").display().to_string(),
            code_repositories: Vec::new(),
        };
        pipeline
            .ingest(
                Path::new(&turn.session_path),
                &RolloutCursor::default(),
                std::slice::from_ref(&turn),
            )
            .unwrap();
        let topics = pipeline.archive_topics(&turn).unwrap();
        let candidates_ok =
            index != 1 && index != 2 || topics.iter().any(|t| Some(&t.id) == id.as_ref());
        let input = json!({"user":user,"assistant":assistant,"topics":topics,"service":service_key(&service),"sourceHash":blake3::hash(model_api::EXTRACTION_PROMPT.as_bytes()).to_hex().as_str()});
        let hash = blake3::hash(input.to_string().as_bytes())
            .to_hex()
            .to_string();
        let path = cache.join(format!("chat-{hash}.json"));
        let cached = path.exists();
        pipeline.mark_extracting(&turn.source_id).unwrap();
        let extraction: ChatExtraction = if cached {
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
        } else {
            let result = model_api::chat_extract(&service, &turn, &topics)
                .await
                .unwrap();
            fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
            actual_requests += 1;
            result
        };
        let expected = if index == 3 {
            "skip"
        } else if index == 1 || index == 2 {
            "update"
        } else {
            "create"
        };
        let action = if extraction.memories.is_empty() {
            "skip"
        } else if extraction.memories.len() == 1 && extraction.memories[0].target_id.is_some() {
            "update"
        } else {
            "create"
        };
        let target_ok = expected != "update"
            || extraction.memories.len() == 1
                && extraction.memories[0].target_id.as_ref() == id.as_ref();
        pipeline
            .extraction_complete(&turn.source_id, &extraction)
            .unwrap();
        pipeline.materialize(&mut vault).unwrap();
        if index == 0 {
            id = Some(vault.search(None, "").unwrap()[0].id.clone());
        }
        let correct = candidates_ok
            && action == expected
            && target_ok
            && (expected == "skip" || extraction.memories.len() == 1);
        if correct && expected != "skip" {
            effective += 1;
        }
        cases.push(json!({"turn":index,"expectedAction":expected,"action":action,"candidateIncluded":candidates_ok,"targetCorrect":target_ok,"correct":correct,"cachedModelResponse":cached,"usage":extraction.usage}));
        if index == 1 {
            let record = vault.read(id.as_ref().unwrap()).unwrap();
            manual = "采用两秒轮询；用户决定保留这个表述。".into();
            vault
                .save(MemoryInput {
                    id: Some(record.summary.id),
                    repository_id: record.summary.repository_id,
                    title: record.summary.title,
                    kind: record.summary.kind,
                    tags: record.summary.tags,
                    markdown: crate::memory_markdown::replace_conclusion(&record.markdown, &manual),
                    related_ids: record.outgoing_related_ids,
                    revision: Some(record.revision),
                    source: record.source,
                })
                .unwrap();
        }
    }
    let record = vault.read(id.as_ref().unwrap()).unwrap();
    let status = pipeline.status(false, None, false, None).unwrap();
    let preserved = record.current_conclusion == manual && record.conclusion_owner == "manual";
    let before = serde_json::to_value(&status).unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let after = serde_json::to_value(pipeline.status(false, None, false, None).unwrap()).unwrap();
    assert_eq!(before, after);
    json!({"model":service.model,"cases":cases,"effectiveArchives":effective,"chatInputTokens":status.chat_input_tokens,"chatOutputTokens":status.chat_output_tokens,"inputTokensPerEffectiveArchive":(effective>0).then(||status.chat_input_tokens as f64/effective as f64),"outputTokensPerEffectiveArchive":(effective>0).then(||status.chat_output_tokens as f64/effective as f64),"manualConclusionPreserved":preserved,"sourceCount":record.evidence_sources.len(),"suggestionCount":record.suggestions.len(),"actualNewRequests":actual_requests,"usageIncomplete":status.usage_incomplete,"costBasis":"Usage belongs to the saved real model responses; cached responses are replayed without a new model call."})
}

#[tokio::test]
#[ignore = "AIMEMORY_OPTIMIZATION_LIVE=1 使用现有服务检验独立留出集与归档动作，保留真实响应缓存"]
async fn independent_holdout_and_real_archive_actions() {
    assert_eq!(
        std::env::var("AIMEMORY_OPTIMIZATION_LIVE").as_deref(),
        Ok("1")
    );
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let raw = fs::read(project.join("src-tauri/tests/fixtures/search-holdout.json")).unwrap();
    let corpus: Value = serde_json::from_slice(&raw).unwrap();
    let base = project.join("dev-data/optimization-acceptance");
    let cache = base.join("live-response-cache");
    fs::create_dir_all(&cache).unwrap();
    let run = base.join(format!(
        "holdout-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S")
    ));
    let root = run.join("vault");
    let settings = SettingsStore::load(
        &project.join("dev-data"),
        crate::codex::default_codex_directory(),
    )
    .unwrap();
    let service = settings.credentials(ServiceKind::Embedding).unwrap();
    let key = service_key(&service);
    let mut vault = Vault::open(&root).unwrap();
    let mut repos = BTreeMap::new();
    for name in ["frontend", "backend"] {
        let repo = vault
            .add_repository(RepositoryInput {
                name: name.into(),
                remote: format!("https://github.com/aimemory-holdout/{name}.git"),
            })
            .unwrap();
        repos.insert(name.to_string(), repo);
    }
    for memory in corpus["memories"].as_array().unwrap() {
        let id = memory["id"].as_str().unwrap();
        let repo = &repos[memory["repository"].as_str().unwrap()];
        let tags: Vec<String> = serde_json::from_value(memory["tags"].clone()).unwrap();
        let mut body = format!(
            "## 结论\n\n{}\n\n## 原文依据\n\n> {}\n",
            memory["conclusion"].as_str().unwrap(),
            memory["evidence"].as_str().unwrap()
        );
        if let Some(history) = memory["history"].as_str() {
            body.push_str(&format!("\n## 变更记录\n\n{history}\n"));
        }
        if memory["manual"] == true {
            let meta = json!({"id":id,"repository_id":repo.id,"repository_remote":repo.remote,"title":memory["title"],"kind":"技术决策","tags":tags,"conclusion_owner":"manual","source":"人工确认"});
            fs::write(
                root.join(&repo.id).join(format!("{id}.md")),
                format!(
                    "---\n{}---\n{body}",
                    serde_yaml_ng::to_string(&meta).unwrap()
                ),
            )
            .unwrap();
            vault.refresh().unwrap();
        } else {
            vault
                .save_archive(
                    id,
                    MemoryInput {
                        id: None,
                        repository_id: repo.id.clone(),
                        title: memory["title"].as_str().unwrap().into(),
                        kind: "技术决策".into(),
                        tags,
                        markdown: body,
                        related_ids: Vec::new(),
                        revision: None,
                        source: Some("独立合成留出集".into()),
                    },
                    &ArchiveSource {
                        source_id: format!("source-{id}"),
                        session_id: format!("session-{id}"),
                        turn_id: "turn-1".into(),
                        input_truncated: false,
                        message_role: Some("user".into()),
                    },
                    &[],
                )
                .unwrap();
        }
    }
    let mut pipeline = Pipeline::open(&root).unwrap();
    pipeline.sync_embeddings(&vault, &key, true).unwrap();
    let mut requests = Vec::new();
    while let Some(job) = pipeline.next_embedding().unwrap() {
        let result = vector(&service, &job.content, &cache, &mut requests, "memory").await;
        pipeline.embedding_complete(&job, Ok(result)).unwrap();
    }
    drop(SearchEngine::open(&root).unwrap());
    let db = rusqlite::Connection::open(root.join(".aimemory/pipeline.sqlite")).unwrap();
    for query in corpus["queries"].as_array().unwrap() {
        if query["mode"] == "id" {
            continue;
        }
        let text = query["query"].as_str().unwrap();
        let result = vector(&service, text, &cache, &mut requests, "query").await;
        let hash = blake3::hash(format!("{key}\n{text}").as_bytes())
            .to_hex()
            .to_string();
        let bytes = result
            .vector
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect::<Vec<_>>();
        db.execute("INSERT INTO query_vectors(cache_key,owner,expires_at,vector,dimensions) VALUES(?1,'holdout-setup',0,?2,?3)",rusqlite::params![hash,bytes,result.vector.len() as i64]).unwrap();
    }
    let config = run.join("config");
    let mut local = SettingsStore::load(&config, run.join("codex")).unwrap();
    local
        .update(UpdateSettings {
            chat: ServiceInput {
                provider: None,
                base_url: service.base_url.clone(),
                model: "unused".into(),
                api_key: None,
            },
            embedding: ServiceInput {
                provider: None,
                base_url: service.base_url.clone(),
                model: service.model.clone(),
                api_key: None,
            },
            codex_directory: run.join("codex").display().to_string(),
            archive_enabled: false,
            run_in_background: false,
        })
        .unwrap();
    let client = Client::start(&config, &root, &project).await;
    let baseline = BaselineClient::start(
        &root,
        crate::config::ServiceCredentials {
            base_url: service.base_url.clone(),
            model: service.model.clone(),
            api_key: String::new(),
        },
    )
    .await;
    let mut cases = Vec::new();
    let mut hit5 = 0;
    let mut top1 = 0;
    let mut answered = 0;
    for query in corpus["queries"].as_array().unwrap() {
        let repo = &repos[query["repository"].as_str().unwrap()];
        let mut args = json!({"cwd":project,"repository":repo.id,"query":query["query"]});
        if query["mode"] == "id" {
            args["mode"] = json!("id");
        }
        let default = client.call("search_memories", args.clone()).await;
        assert_eq!(default["isError"], false);
        let result = &default["structuredContent"];
        let mut keyword = args.clone();
        keyword["mode"] = json!("keyword");
        let keyword = client.call("search_memories", keyword).await;
        let semantic = if query["mode"] == "id" {
            Value::Null
        } else {
            call(&baseline.service, args).await
        };
        let expected = query["expectedIds"].as_array().unwrap();
        let rank = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .position(|h| expected.contains(&h["id"]))
            .map(|n| n + 1);
        let mut evidence_calls = None;
        let mut manual_calls = None;
        let mut conclusion_bytes = 0;
        let mut source_bytes = 0;
        let mut full_evidence_bytes = 0;
        if !expected.is_empty() {
            answered += 1;
            hit5 += usize::from(rank.is_some());
            top1 += usize::from(rank == Some(1));
            if let Some(position) = rank {
                let id = &result["results"][position - 1]["id"];
                let headings = client.call("list_memory_sections", json!({"id":id})).await;
                let current = client.call("read_memory", json!({"id":id})).await;
                conclusion_bytes = current["structuredContent"]["markdown"]
                    .as_str()
                    .unwrap()
                    .len();
                if headings["structuredContent"]["sections"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["sourceCount"].as_u64().unwrap_or(0) > 0)
                    && id != "H-10"
                {
                    let directory = client
                        .call(
                            "list_memory_sections",
                            json!({"id":id,"section":"原文依据"}),
                        )
                        .await;
                    let source = &directory["structuredContent"]["sources"][0]["sourceId"];
                    let evidence = client
                        .call(
                            "read_memory",
                            json!({"id":id,"mode":"evidence","sourceId":source}),
                        )
                        .await;
                    let wanted = corpus["memories"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|m| m["id"] == *id)
                        .unwrap();
                    assert_eq!(
                        evidence["structuredContent"]["markdown"],
                        wanted["evidence"]
                    );
                    source_bytes = evidence["structuredContent"]["markdown"]
                        .as_str()
                        .unwrap()
                        .len();
                    evidence_calls = Some(5);
                    let full = client
                        .call("read_memory", json!({"id":id,"mode":"evidence"}))
                        .await;
                    full_evidence_bytes = full["structuredContent"]["markdown"]
                        .as_str()
                        .unwrap()
                        .len();
                } else {
                    manual_calls = Some(3);
                    assert_eq!(current["structuredContent"]["conclusionSource"], "manual");
                }
            }
        }
        cases.push(json!({"id":query["id"],"category":query["category"],"expectedIds":expected,"rank":rank,"default":result,"keyword":keyword["structuredContent"],"pureSemanticBaseline":semantic,"firstEvidenceToolCalls":evidence_calls,"manualConclusionToolCalls":manual_calls,"conclusionReadAlsoVerified":!expected.is_empty() && rank.is_some(),"summaryToolUtf8Bytes":serde_json::to_vec(&default).unwrap().len(),"conclusionUtf8Bytes":conclusion_bytes,"singleSourceUtf8Bytes":source_bytes,"evidenceSectionUtf8Bytes":full_evidence_bytes,"noAnswerCandidateCount":expected.is_empty().then(||result["results"].as_array().unwrap().len()),"candidatesInvalidAsAnswers":expected.is_empty().then(||result["results"].as_array().unwrap().len())}));
    }
    baseline.stop().await;
    let archive = archive_acceptance(&project, &run, &cache, &settings).await;
    let report = json!({"fixtureHash":blake3::hash(&raw).to_hex().as_str(),"model":service.model,"answeredQuestions":answered,"hitAt5":hit5,"top1":top1,"cases":cases,"preparationRequests":requests,"archive":archive,"tokenizerTokens":null,"tokenizerNote":"No caller tokenizer is configured; context sizes are UTF-8 bytes. Summary bytes include the actual HTTP tool result envelope. Pure semantic baseline is served over HTTP from the pinned original kernel."});
    fs::write(
        run.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "independent holdout report: {}",
        run.join("report.json").display()
    );
    assert_eq!(hit5, answered);
    assert_eq!(archive["effectiveArchives"], 4);
    assert_eq!(archive["manualConclusionPreserved"], true);
    assert!(archive["cases"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["correct"] == true));
}
