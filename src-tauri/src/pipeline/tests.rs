use super::*;

#[test]
fn deleting_a_repository_discards_its_queued_writes_and_allows_future_work() {
    use crate::{
        code_context::CodeRepository,
        storage::{normalize_remote, repository_id},
    };
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let mut source = turn("queued");
    let frontend =
        repository_id(&normalize_remote(source.repository_remote.as_deref().unwrap()).unwrap());
    let backend = vault
        .add_repository(RepositoryInput {
            name: "backend".into(),
            remote: "https://github.com/example/backend".into(),
        })
        .unwrap();
    source.code_repositories = vec![CodeRepository {
        id: backend.id.clone(),
        name: backend.name.clone(),
        remote: backend.remote.clone(),
        root: "/project/backend".into(),
        files: vec!["src/api.rs".into()],
    }];
    let mut result = extraction();
    result.memories[0].repository_id = Some(frontend.clone());
    result.memories[0].related_indices = vec![1];
    result.memories[1].repository_id = Some(backend.id.clone());
    result.memories[1].code_paths = vec!["src/api.rs".into()];
    ingest(&mut pipeline, &source);
    pipeline
        .extraction_complete(&source.source_id, &result)
        .unwrap();
    vault.delete_repository(&backend.id).unwrap();
    pipeline.remove_deleted_memories(&backend.id).unwrap();
    pipeline.materialize(&mut vault).unwrap();
    assert_eq!(vault.workspace().unwrap().repositories.len(), 1);
    assert_eq!(vault.search(Some(&frontend), "").unwrap().len(), 1);
    assert!(vault.graph().unwrap().edges.is_empty());
    pipeline.sync_embeddings(&vault, "fixture", true).unwrap();
    assert_eq!(
        pipeline
            .status(false, None, false, None)
            .unwrap()
            .written_memories,
        1
    );
    source.source_id = "new-work".into();
    source.turn_id = "new-work".into();
    result.memories.remove(0);
    ingest(&mut pipeline, &source);
    pipeline
        .extraction_complete(&source.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    assert_eq!(vault.search(Some(&backend.id), "").unwrap().len(), 1);
    assert_eq!(vault.workspace().unwrap().repositories.len(), 2);
}

#[test]
fn cross_repository_archives_link_siblings_and_same_code_without_merging_unrelated_topics() {
    use crate::code_context::CodeRepository;
    use crate::storage::{normalize_remote, repository_id};
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let mut source = turn("full-stack");
    let frontend_remote = normalize_remote(source.repository_remote.as_deref().unwrap()).unwrap();
    let frontend_id = repository_id(&frontend_remote);
    let backend_remote = "https://github.com/example/backend";
    let backend_id = repository_id(backend_remote);
    source.code_repositories = vec![
        CodeRepository {
            id: frontend_id.clone(),
            name: "frontend-web".into(),
            remote: frontend_remote,
            root: "/project/frontend-web".into(),
            files: vec!["src/Status.vue".into(), "src/Other.vue".into()],
        },
        CodeRepository {
            id: backend_id.clone(),
            name: "backend".into(),
            remote: backend_remote.into(),
            root: "/project/backend".into(),
            files: vec!["src/status.rs".into()],
        },
    ];
    let mut result = extraction();
    result.memories[0].repository_id = Some(frontend_id.clone());
    result.memories[0].code_paths = vec!["src/Status.vue".into()];
    result.memories[0].related_indices = vec![1];
    result.memories[1].repository_id = Some(backend_id.clone());
    result.memories[1].code_paths = vec!["src/status.rs".into()];
    ingest(&mut pipeline, &source);
    pipeline
        .extraction_complete(&source.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let first = vault.search(Some(&frontend_id), "").unwrap().remove(0);
    let backend = vault.search(Some(&backend_id), "").unwrap().remove(0);
    assert_eq!(vault.graph().unwrap().edges.len(), 1);
    assert_eq!(
        vault.read(&backend.id).unwrap().related_ids,
        vec![first.id.clone()]
    );
    assert_eq!(
        vault.read(&first.id).unwrap().code_paths,
        vec!["src/Status.vue"]
    );
    let mut later = source.clone();
    later.source_id = "later-source".into();
    later.turn_id = "later".into();
    later.session_id = "later-session".into();
    later.code_repositories.truncate(1);
    later.user_text = "任务状态轮询继续复用已有后端接口。".into();
    let topics = pipeline.archive_topics(&later).unwrap();
    assert!(topics
        .iter()
        .any(|topic| topic.id == first.id && topic.can_update));
    assert!(topics
        .iter()
        .any(|topic| topic.id == backend.id && !topic.can_update));
    result.memories.truncate(1);
    result.memories[0].related_indices.clear();
    result.memories[0].related_ids = vec![backend.id.clone()];
    ingest(&mut pipeline, &later);
    pipeline
        .extraction_complete(&later.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    assert_eq!(vault.search(Some(&frontend_id), "").unwrap().len(), 2);
    let latest = vault
        .search(Some(&frontend_id), "")
        .unwrap()
        .into_iter()
        .find(|m| m.id != first.id)
        .unwrap();
    assert_eq!(vault.read(&latest.id).unwrap().related_ids.len(), 2);
    later.source_id = "independent-source".into();
    later.turn_id = "independent".into();
    result.memories[0].code_paths = vec!["src/Other.vue".into()];
    result.memories[0].related_ids.clear();
    ingest(&mut pipeline, &later);
    pipeline
        .extraction_complete(&later.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let independent = vault
        .search(Some(&frontend_id), "")
        .unwrap()
        .into_iter()
        .find(|m| m.id != first.id && m.id != latest.id)
        .unwrap();
    assert!(vault.read(&independent.id).unwrap().related_ids.is_empty());
    drop(vault);
    let mut vault = Vault::open(root.path()).unwrap();
    vault.rebuild().unwrap();
    assert_eq!(vault.graph().unwrap().edges.len(), 3);
    assert_eq!(
        vault
            .memories_for_code(&frontend_id, &["src/Status.vue".into()])
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn lexical_archive_candidates_recover_an_old_topic_across_sessions() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let first = turn("old-config");
    ingest(&mut pipeline, &first);
    let mut result = extraction();
    result.memories.truncate(1);
    result.memories[0].title = "网关规则".into();
    result.memories[0].summary = "配置 lamp.grayscale.fixed-versions 按服务固定版本。".into();
    result.memories[0].tags.clear();
    pipeline
        .extraction_complete(&first.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let id = vault.search(None, "").unwrap()[0].id.clone();
    for index in 0..20 {
        let mut recent = turn(&format!("recent-{index}"));
        recent.session_id = "recent-session".into();
        ingest(&mut pipeline, &recent);
        result.memories[0].title = format!("独立页面功能 {index}");
        result.memories[0].summary = format!("页面 {index} 的独立功能已经确定。");
        pipeline
            .extraction_complete(&recent.source_id, &result)
            .unwrap();
        pipeline.materialize(&mut vault).unwrap();
    }
    let mut query = turn("query");
    query.session_id = "recent-session".into();
    query.user_text = "为lamp.grayscale.fixed-versions补充说明".into();
    let topics = pipeline.archive_topics(&query).unwrap();
    assert!(topics.len() <= 16);
    assert!(topics.iter().any(|topic| topic.id == id));
}

#[test]
fn legacy_auto_hash_is_restored_from_saved_extraction_and_replay_keeps_source_once() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let first = turn("baseline");
    ingest(&mut pipeline, &first);
    let mut result = extraction();
    result.memories.truncate(1);
    pipeline
        .extraction_complete(&first.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let memory = vault.search(None, "").unwrap().remove(0);
    let raw = fs::read_to_string(&memory.path)
        .unwrap()
        .lines()
        .filter(|line| {
            !line.starts_with("archive_summary_hash:") && !line.starts_with("conclusion_owner:")
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&memory.path, raw).unwrap();
    vault.refresh().unwrap();
    let second = turn("followup");
    ingest(&mut pipeline, &second);
    result.memories[0].target_id = Some(memory.id.clone());
    result.memories[0].summary = "终态和卸载时停止轮询。".into();
    pipeline
        .extraction_complete(&second.source_id, &result)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    let changed = vault.read(&memory.id).unwrap();
    assert_eq!(changed.current_conclusion, result.memories[0].summary);
    assert_eq!(changed.conclusion_owner, "auto");
    assert!(changed
        .markdown
        .contains("每两秒查询任务状态，终态时停止。"));
    pipeline
        .db
        .execute(
            "UPDATE turns SET status='pending' WHERE source_id=?1",
            [&second.source_id],
        )
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    assert_eq!(vault.read(&memory.id).unwrap().revision, changed.revision);
    assert_eq!(vault.read(&memory.id).unwrap().evidence_sources.len(), 2);
    assert_eq!(
        pipeline
            .status(false, None, false, None)
            .unwrap()
            .chat_input_tokens,
        34
    );
}

#[tokio::test]
async fn rejected_model_action_records_reported_usage_without_creating_a_memory() {
    use crate::model_api::{chat_extract, tests::serve_responses};
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let mut pipeline = Pipeline::open(root.path()).unwrap();
    let turn = turn("rejected");
    ingest(&mut pipeline, &turn);
    pipeline.mark_extracting(&turn.source_id).unwrap();
    let (service, _requests) = serve_responses(vec![(
        200,
        serde_json::json!({"choices":[{"message":{"content":"{\"memories\":[{\"op\":\"delete\"}]}"}}],"usage":{"prompt_tokens":19,"completion_tokens":7}}),
    )]);
    let error = chat_extract(&service, &turn, &[]).await.unwrap_err();
    pipeline
        .extraction_rejected(&turn.source_id, &error)
        .unwrap();
    assert!(!pipeline.materialize(&mut vault).unwrap());
    assert!(vault.search(None, "").unwrap().is_empty());
    let status = pipeline.status(false, None, false, None).unwrap();
    assert_eq!(status.chat_input_tokens, 19);
    assert_eq!(status.chat_output_tokens, 7);
    assert!(!status.usage_incomplete);
}
use crate::{
    model_api::{ApiUsage, ExtractedMemory},
    storage::MemoryRecord,
};
use std::fs;

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn turn(id: &str) -> CodexTurn {
    CodexTurn {
        source_id: format!("session:{id}"),
        session_id: "session".into(),
        turn_id: id.into(),
        completed_at: "2026-11-21T03:00:01.000Z".into(),
        cwd: "/project/frontend-web".into(),
        repository_remote: Some("git@github.com:example/frontend-web.git".into()),
        repository_name: "frontend-web".into(),
        user_text: "任务完成或页面卸载时停止查询。".into(),
        assistant_text: "每两秒查询任务状态，失败时保留原因。".into(),
        session_path: "/codex/sessions/rollout-test.jsonl".into(),
        code_repositories: Vec::new(),
    }
}

fn extraction() -> ChatExtraction {
    ChatExtraction {
        memories: vec![
            ExtractedMemory {
                op: None,
                target_id: None,
                title: "轮询到终态时停止".into(),
                kind: "技术决策".into(),
                summary: "每两秒查询任务状态，终态时停止。".into(),
                details: "页面卸载时同时清理计时器。".into(),
                evidence: "任务完成或页面卸载时停止查询。".into(),
                tags: vec!["轮询".into()],
                ..Default::default()
            },
            ExtractedMemory {
                op: None,
                target_id: None,
                title: "失败时保留原因".into(),
                kind: "实现约束".into(),
                summary: "任务失败时返回可读原因。".into(),
                details: "停止查询后仍可阅读失败原因。".into(),
                evidence: "失败时保留原因。".into(),
                tags: vec!["失败原因".into()],
                ..Default::default()
            },
        ],
        usage: ApiUsage {
            input_tokens: 17,
            output_tokens: 9,
            reported: true,
        },
        latency_ms: 31,
        input_truncated: false,
    }
}

fn ingest(pipeline: &mut Pipeline, turn: &CodexTurn) {
    pipeline
        .ingest(
            Path::new(&turn.session_path),
            &RolloutCursor::default(),
            std::slice::from_ref(turn),
        )
        .unwrap();
}

fn cache_extraction(pipeline: &Pipeline, turn: &CodexTurn) {
    pipeline.mark_extracting(&turn.source_id).unwrap();
    pipeline
        .extraction_complete(&turn.source_id, &extraction())
        .unwrap();
}

fn archive_id(turn: &CodexTurn, index: usize) -> String {
    format!(
        "ARC-{}",
        &blake3::hash(format!("{}:{index}", turn.source_id).as_bytes()).to_hex()[..24]
    )
}

fn edit_input(memory: MemoryRecord) -> MemoryInput {
    MemoryInput {
        id: Some(memory.summary.id),
        repository_id: memory.summary.repository_id,
        title: memory.summary.title,
        kind: memory.summary.kind,
        tags: memory.summary.tags,
        markdown: memory.markdown,
        related_ids: memory.outgoing_related_ids,
        revision: Some(memory.revision),
        source: memory.source,
    }
}

fn embedding() -> EmbeddingResult {
    EmbeddingResult {
        vector: vec![0.25, -0.5, 1.0],
        usage: ApiUsage {
            input_tokens: 5,
            output_tokens: 0,
            reported: true,
        },
        latency_ms: 7,
    }
}

fn complete_next_embedding(pipeline: &mut Pipeline) -> EmbeddingJob {
    let job = pipeline.next_embedding().unwrap().unwrap();
    pipeline.embedding_complete(&job, Ok(embedding())).unwrap();
    job
}

#[test]
fn repeated_source_scan_never_reschedules_chat_after_cached_extraction_or_saved_restart() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("deduplicated");
    ingest(&mut pipeline, &source);
    ingest(&mut pipeline, &source);
    assert_eq!(
        pipeline
            .db
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        pipeline.next_turn().unwrap().unwrap().source_id,
        source.source_id
    );
    pipeline.mark_extracting(&source.source_id).unwrap();
    assert!(pipeline.next_turn().unwrap().is_none());
    pipeline
        .extraction_complete(&source.source_id, &extraction())
        .unwrap();
    ingest(&mut pipeline, &source);
    assert!(pipeline.next_turn().unwrap().is_none());
    drop(pipeline);
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    assert!(pipeline.next_turn().unwrap().is_none());
    pipeline.materialize(&mut vault).unwrap();
    ingest(&mut pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    assert!(pipeline.next_turn().unwrap().is_none());
    assert_eq!(vault.search(None, "").unwrap().len(), 2);
    let status = pipeline
        .status(true, Some("2026-11-21T03:00:00Z".into()), false, None)
        .unwrap();
    assert_eq!(status.processed_turns, 1);
    assert_eq!(status.written_memories, 2);
    assert_eq!(status.chat_input_tokens, 17);
    assert_eq!(status.chat_output_tokens, 9);
    drop(pipeline);
    let mut reopened = Pipeline::open(directory.path()).unwrap();
    ingest(&mut reopened, &source);
    assert!(reopened.next_turn().unwrap().is_none());
    assert_eq!(reopened.cursors().unwrap().len(), 1);
}

#[test]
fn empty_extraction_is_remembered_without_rescheduling_a_model_call() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("no-durable-memory");
    ingest(&mut pipeline, &source);
    let mut empty = extraction();
    empty.memories.clear();
    pipeline.mark_extracting(&source.source_id).unwrap();
    pipeline
        .extraction_complete(&source.source_id, &empty)
        .unwrap();
    pipeline.materialize(&mut vault).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    drop(pipeline);
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    ingest(&mut pipeline, &source);
    assert!(pipeline.next_turn().unwrap().is_none());
    assert!(pipeline.next_embedding().unwrap().is_none());
    assert!(vault.search(None, "").unwrap().is_empty());
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.processed_turns, 1);
    assert_eq!(status.written_memories, 0);
    assert_eq!(status.chat_input_tokens, 17);
}

#[test]
fn all_markdown_is_readable_before_vectors_and_float_bytes_are_persisted() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("multiple-memories");
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    let memories = vault.search(None, "").unwrap();
    assert_eq!(memories.len(), 2);
    for memory in &memories {
        let raw = fs::read_to_string(&memory.path).unwrap();
        assert!(raw.contains("## 结论"));
        assert!(raw.contains("## 原文依据"));
        assert!(raw.contains(&source.source_id));
        assert!(!vault.read(&memory.id).unwrap().markdown.is_empty());
    }
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let pending = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(pending.pending_embeddings, 2);
    assert_eq!(pending.embedding_input_tokens, 0);
    let first = complete_next_embedding(&mut pipeline);
    let second = complete_next_embedding(&mut pipeline);
    assert_ne!(first.memory_id, second.memory_id);
    assert!(pipeline.next_embedding().unwrap().is_none());
    let (vector, dimensions, status): (Vec<u8>, usize, String) = pipeline
        .db
        .query_row(
            "SELECT vector,dimensions,status FROM embeddings WHERE memory_id=?1",
            [&first.memory_id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize, r.get(2)?)),
        )
        .unwrap();
    let expected = embedding()
        .vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    assert_eq!(vector, expected);
    assert_eq!(dimensions, 3);
    assert_eq!(status, "ready");
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.pending_embeddings, 0);
    assert_eq!(status.embedding_input_tokens, 10);
    assert_eq!(status.recent_turns[0].embedding_input_tokens, 10);
    assert_eq!(status.recent_turns[0].embedding_latency_ms, Some(14));
}

#[test]
fn partial_disk_failure_retries_cached_memories_and_preserves_the_users_saved_edit() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("partial-save");
    let repo = vault
        .add_repository(RepositoryInput {
            name: source.repository_name.clone(),
            remote: source.repository_remote.clone().unwrap(),
        })
        .unwrap();
    let first_id = archive_id(&source, 0);
    let second_id = archive_id(&source, 1);
    let blocked_path = directory
        .path()
        .join(&repo.id)
        .join(format!("{second_id}.md"));
    fs::create_dir(&blocked_path).unwrap();
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    assert_eq!(vault.search(None, "").unwrap().len(), 1);
    assert!(pipeline.next_turn().unwrap().is_none());
    let failure = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(failure.recent_turns[0].status, "failed");
    assert_eq!(failure.written_memories, 1);
    let mut edit = edit_input(vault.read(&first_id).unwrap());
    edit.title = "用户已经纠正的结论".into();
    edit.markdown = "## 结论\n\n保留用户编辑后的正文。\n".into();
    vault.save(edit).unwrap();
    fs::remove_dir(&blocked_path).unwrap();
    drop(pipeline);
    drop(vault);
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    ingest(&mut pipeline, &source);
    pipeline.retry_failed().unwrap();
    assert!(pipeline.next_turn().unwrap().is_none());
    pipeline.materialize(&mut vault).unwrap();
    let preserved = vault.read(&first_id).unwrap();
    assert_eq!(preserved.summary.title, "用户已经纠正的结论");
    assert!(preserved.markdown.contains("保留用户编辑后的正文"));
    assert_eq!(
        vault.read(&second_id).unwrap().summary.title,
        "失败时保留原因"
    );
    assert_eq!(vault.search(None, "").unwrap().len(), 2);
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.recent_turns[0].status, "saved");
    assert_eq!(status.chat_input_tokens, 17);
    assert_eq!(status.written_memories, 2);
}

#[test]
fn skipped_turns_do_not_schedule_indexes_or_count_as_failures() {
    let directory = fixture();
    let _vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let mut source = turn("without-git-remote");
    source.repository_remote = None;
    ingest(&mut pipeline, &source);
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.recent_turns[0].status, "skipped");
    assert_eq!(status.recent_turns[0].memory_count, 0);
    assert_eq!(status.failed_embeddings, 0);
    assert!(status.failed_embedding_details.is_empty());
    assert_eq!(status.pending_embeddings, 0);
    assert!(pipeline.next_turn().unwrap().is_none());
    assert!(pipeline.next_embedding().unwrap().is_none());
}

#[test]
fn embedding_failure_keeps_markdown_and_retry_does_not_repeat_extraction() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("failed-vector");
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let job = pipeline.next_embedding().unwrap().unwrap();
    let before = vault.read(&job.memory_id).unwrap();
    let raw = fs::read_to_string(&before.summary.path).unwrap();
    pipeline
        .embedding_complete(&job, Err("向量服务返回 HTTP 503".into()))
        .unwrap();
    let mut summary = vault.read(&job.memory_id).unwrap().summary;
    pipeline.annotate(&mut summary).unwrap();
    assert_eq!(summary.embedding_status, "failed");
    assert_eq!(
        summary.embedding_error.as_deref(),
        Some("向量服务返回 HTTP 503")
    );
    assert_eq!(fs::read_to_string(&before.summary.path).unwrap(), raw);
    let mut skipped = turn("skipped-without-git");
    skipped.repository_remote = None;
    ingest(&mut pipeline, &skipped);
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.failed_embeddings, 1);
    assert_eq!(status.failed_embedding_details.len(), 1);
    assert_eq!(status.failed_embedding_details[0].memory_id, job.memory_id);
    assert_eq!(
        status.failed_embedding_details[0].error.as_deref(),
        Some("向量服务返回 HTTP 503")
    );
    let skipped_turn = status
        .recent_turns
        .iter()
        .find(|turn| turn.source_id == skipped.source_id)
        .unwrap();
    assert_eq!(skipped_turn.status, "skipped");
    assert_eq!(skipped_turn.memory_count, 0);
    pipeline.retry_failed().unwrap();
    assert!(pipeline.next_turn().unwrap().is_none());
    let retried = pipeline.next_embedding().unwrap().unwrap();
    assert_eq!(retried.memory_id, job.memory_id);
    pipeline
        .embedding_complete(&retried, Ok(embedding()))
        .unwrap();
    assert!(pipeline
        .status(true, None, false, None)
        .unwrap()
        .failed_embedding_details
        .is_empty());
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .failed_embeddings,
        0
    );
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .chat_input_tokens,
        17
    );
    assert_eq!(fs::read_to_string(&before.summary.path).unwrap(), raw);
}

#[test]
fn evidence_details_and_source_edits_reuse_vectors_but_retrieval_fields_regenerate() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("retrieval-fingerprint");
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    complete_next_embedding(&mut pipeline);
    complete_next_embedding(&mut pipeline);
    let id = archive_id(&source, 0);
    let mut edit = edit_input(vault.read(&id).unwrap());
    edit.markdown = edit
        .markdown
        .replace("页面卸载时同时清理计时器。", "补充实现细节。")
        .replace("任务完成或页面卸载时停止查询。", "原文依据已核对。");
    edit.source = Some("补充的来源说明".into());
    vault.save(edit).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    assert!(pipeline.next_embedding().unwrap().is_none());

    let mut edit = edit_input(vault.read(&id).unwrap());
    edit.title = "更新后的轮询标题".into();
    vault.save(edit).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let title_job = complete_next_embedding(&mut pipeline);
    assert_eq!(title_job.memory_id, id);
    assert!(title_job.content.contains("更新后的轮询标题"));

    let mut edit = edit_input(vault.read(&id).unwrap());
    edit.markdown = edit.markdown.replace(
        "每两秒查询任务状态，终态时停止。",
        "只在任务运行期间定时查询。",
    );
    vault.save(edit).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let summary_job = complete_next_embedding(&mut pipeline);
    assert_ne!(summary_job.input_hash, title_job.input_hash);
    assert!(summary_job.content.contains("只在任务运行期间定时查询"));

    let mut edit = edit_input(vault.read(&id).unwrap());
    edit.tags = vec!["新的检索标签".into()];
    vault.save(edit).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let tag_job = complete_next_embedding(&mut pipeline);
    assert_ne!(tag_job.input_hash, summary_job.input_hash);
    assert!(tag_job.content.contains("新的检索标签"));
    assert!(pipeline.next_turn().unwrap().is_none());
    assert!(pipeline.next_embedding().unwrap().is_none());
}

#[test]
fn model_change_and_restart_queue_vectors_without_any_new_chat_extraction() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("model-change");
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    pipeline
        .sync_embeddings(&vault, "base-url:model-a", false)
        .unwrap();
    assert!(pipeline.next_embedding().unwrap().is_none());
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .pending_embeddings,
        2
    );
    pipeline
        .sync_embeddings(&vault, "base-url:model-a", true)
        .unwrap();
    complete_next_embedding(&mut pipeline);
    complete_next_embedding(&mut pipeline);
    assert!(pipeline.next_embedding().unwrap().is_none());
    pipeline
        .sync_embeddings(&vault, "base-url:model-b", true)
        .unwrap();
    let claimed = pipeline.next_embedding().unwrap().unwrap();
    assert_eq!(claimed.service_key, "base-url:model-b");
    assert!(pipeline.next_turn().unwrap().is_none());
    drop(pipeline);
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    pipeline.recover_interrupted().unwrap();
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .failed_embeddings,
        1
    );
    pipeline.retry_failed().unwrap();
    let reclaimed = pipeline.next_embedding().unwrap().unwrap();
    assert_eq!(reclaimed.memory_id, claimed.memory_id);
    assert_eq!(reclaimed.service_key, "base-url:model-b");
    pipeline
        .embedding_complete(&reclaimed, Ok(embedding()))
        .unwrap();
    complete_next_embedding(&mut pipeline);
    assert!(pipeline.next_turn().unwrap().is_none());
    assert!(pipeline.next_embedding().unwrap().is_none());
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .chat_input_tokens,
        17
    );
}

#[test]
fn stale_vector_response_cannot_overwrite_a_new_retrieval_input() {
    let directory = fixture();
    let mut vault = Vault::open(directory.path()).unwrap();
    let mut pipeline = Pipeline::open(directory.path()).unwrap();
    let source = turn("changed-while-embedding");
    ingest(&mut pipeline, &source);
    cache_extraction(&pipeline, &source);
    pipeline.materialize(&mut vault).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    let old = pipeline.next_embedding().unwrap().unwrap();
    let mut edit = edit_input(vault.read(&old.memory_id).unwrap());
    edit.title = "请求期间修改标题".into();
    vault.save(edit).unwrap();
    pipeline.sync_embeddings(&vault, "service-a", true).unwrap();
    pipeline.embedding_complete(&old, Ok(embedding())).unwrap();
    let (status, vector): (String, Option<Vec<u8>>) = pipeline
        .db
        .query_row(
            "SELECT status,vector FROM embeddings WHERE memory_id=?1",
            [&old.memory_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert!(vector.is_none());
    let current = pipeline.next_embedding().unwrap().unwrap();
    assert_eq!(current.memory_id, old.memory_id);
    assert_ne!(current.input_hash, old.input_hash);
    pipeline
        .embedding_complete(&current, Ok(embedding()))
        .unwrap();
    let usage = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(usage.embedding_input_tokens, 10);
    assert_eq!(usage.recent_turns[0].embedding_input_tokens, 10);
    pipeline
        .embedding_complete(&current, Ok(embedding()))
        .unwrap();
    assert_eq!(
        pipeline
            .status(true, None, false, None)
            .unwrap()
            .embedding_input_tokens,
        10
    );
    let saved_hash: String = pipeline
        .db
        .query_row(
            "SELECT input_hash FROM embeddings WHERE memory_id=?1",
            [&old.memory_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved_hash, current.input_hash);
}
