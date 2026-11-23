use super::*;
use crate::{
    codex::{CodexTurn, RolloutCursor},
    config::{ServiceCredentials, SettingsStore},
    model_api::{ApiUsage, ChatExtraction, EmbeddingResult, ExtractedMemory},
    search::{self, SearchRequest},
    storage::{MemoryInput, RepositoryInput},
};
use rusqlite::params;
use std::{
    collections::BTreeSet,
    sync::{atomic::AtomicBool, Mutex},
};

fn fixture() -> tempfile::TempDir {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/workspace-migration-tests");
    fs::create_dir_all(&directory).unwrap();
    tempfile::tempdir_in(directory).unwrap()
}

fn state_in(directory: &Path) -> AppState {
    let data = directory.join("settings");
    fs::create_dir(&data).unwrap();
    let mut settings = SettingsStore::load(&data, directory.join("codex")).unwrap();
    let mut config = settings.config().clone();
    config.embedding.base_url = service().base_url;
    config.embedding.model = service().model;
    settings.apply_import(config).unwrap();
    let vault = Vault::open(&directory.join("source")).unwrap();
    let pipeline = Pipeline::open(&vault.root).unwrap();
    let search = SearchEngine::open(&vault.root).unwrap();
    let settings_path = data.join("workspace.json");
    fs::write(
        &settings_path,
        serde_json::to_vec(&SavedWorkspace {
            root: vault.root.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    AppState {
        default_root: vault.root.clone(),
        vault: Mutex::new(Some(vault)),
        settings_path,
        settings: Mutex::new(settings),
        pipeline: Mutex::new(Some(pipeline)),
        recovered_roots: Mutex::new(BTreeSet::new()),
        scanning: AtomicBool::new(false),
        last_error: Mutex::new(None),
        search: Mutex::new(Some(Arc::new(search))),
        data_directory: data,
        transfers: Mutex::new(crate::transfer::TransferState::default()),
        workspace_transition: tokio::sync::RwLock::new(()),
    }
}

fn destination(directory: &Path) -> PathBuf {
    let path = directory.join("destination");
    fs::create_dir(&path).unwrap();
    path.canonicalize().unwrap()
}

fn service() -> ServiceCredentials {
    ServiceCredentials {
        base_url: "http://127.0.0.1:9/v1".into(),
        model: "migration-fixture".into(),
        api_key: String::new(),
    }
}

fn turn() -> CodexTurn {
    CodexTurn {
        source_id: "codex:migration-session:turn-1".into(),
        session_id: "migration-session".into(),
        turn_id: "turn-1".into(),
        completed_at: "2026-11-21T08:01:00Z".into(),
        cwd: "/example/project".into(),
        repository_remote: Some("https://github.com/example/migration".into()),
        repository_name: "自定义仓库名".into(),
        user_text: "迁移时保留记忆及归档进度。".into(),
        assistant_text: "保留已就绪向量和已使用的 token。".into(),
        session_path: "/example/codex/sessions/rollout-migration.jsonl".into(),
        code_repositories: Vec::new(),
    }
}

fn extraction() -> ChatExtraction {
    ChatExtraction {
        memories: vec![ExtractedMemory {
            op: None,
            target_id: None,
            title: "迁移保留归档进度".into(),
            kind: "实现约束".into(),
            summary: "迁移时保留记忆及归档进度。".into(),
            details: "保留向量、查询缓存和关联。".into(),
            evidence: "迁移时保留记忆及归档进度。".into(),
            tags: vec!["迁移".into()],
            ..Default::default()
        }],
        usage: ApiUsage {
            input_tokens: 17,
            output_tokens: 9,
            reported: true,
        },
        latency_ms: 12,
        input_truncated: false,
    }
}

fn prepare_memories(state: &AppState) -> Vec<(String, String)> {
    let mut vault = state.vault.lock().unwrap();
    let vault = vault.as_mut().unwrap();
    let mut pipeline = state.pipeline.lock().unwrap();
    let pipeline = pipeline.as_mut().unwrap();
    let turn = turn();
    let cursor: RolloutCursor = serde_json::from_value(serde_json::json!({
        "offset":321,"session":null,"active":null
    }))
    .unwrap();
    pipeline
        .baseline(
            "/example/codex",
            "2026-11-21T08:00:00Z",
            &[(PathBuf::from(&turn.session_path), cursor.clone())],
        )
        .unwrap();
    pipeline
        .ingest(
            Path::new(&turn.session_path),
            &cursor,
            std::slice::from_ref(&turn),
        )
        .unwrap();
    pipeline.mark_extracting(&turn.source_id).unwrap();
    pipeline
        .extraction_complete(&turn.source_id, &extraction())
        .unwrap();
    pipeline.materialize(vault).unwrap();
    let archived = vault.search(None, "").unwrap().remove(0);
    vault
        .save(MemoryInput {
            id: None,
            repository_id: archived.repository_id,
            title: "手工关联记忆".into(),
            kind: "工作记录".into(),
            tags: vec!["关联".into()],
            markdown: "## 结论\n\n使用归档记忆的原文依据。\n".into(),
            related_ids: vec![archived.id],
            revision: None,
            source: Some("人工测试数据".into()),
        })
        .unwrap();
    vault
        .add_repository(RepositoryInput {
            name: "没有记忆的自定义仓库".into(),
            remote: "https://github.com/example/empty".into(),
        })
        .unwrap();
    pipeline
        .sync_embeddings(vault, &search::service_key(&service()), true)
        .unwrap();
    while let Some(job) = pipeline.next_embedding().unwrap() {
        pipeline
            .embedding_complete(
                &job,
                Ok(EmbeddingResult {
                    vector: vec![1.0, 0.0],
                    usage: ApiUsage {
                        input_tokens: 7,
                        output_tokens: 0,
                        reported: true,
                    },
                    latency_ms: 3,
                }),
            )
            .unwrap();
    }
    vault
        .search(None, "")
        .unwrap()
        .into_iter()
        .map(|memory| (memory.id, fs::read_to_string(memory.path).unwrap()))
        .collect()
}

#[tokio::test]
async fn migrates_markdown_empty_repositories_relations_and_wal_state_without_model_calls() {
    let directory = fixture();
    let state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    let records = prepare_memories(&state);
    let db = Connection::open(source.join(".aimemory/pipeline.sqlite")).unwrap();
    let key = blake3::hash(format!("{}\n迁移", search::service_key(&service())).as_bytes())
        .to_hex()
        .to_string();
    let vector: Vec<u8> = [1.0f32, 0.0].iter().flat_map(|n| n.to_le_bytes()).collect();
    db.execute(
        "INSERT INTO query_vectors VALUES(?1,'cached-fixture',0,?2,2)",
        params![key, vector],
    )
    .unwrap();
    assert!(
        fs::metadata(source.join(".aimemory/pipeline.sqlite-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = state
        .pipeline
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status(true, None, false, None)
        .unwrap();
    let target = destination(directory.path());
    let migrated = migrate(&state, &target).unwrap();
    assert_eq!(migrated.root, target.to_string_lossy());
    assert_eq!(migrated.repositories.len(), 2);
    assert!(migrated
        .repositories
        .iter()
        .any(|r| r.name == "没有记忆的自定义仓库" && r.memory_count == 0));
    assert!(migrated
        .repositories
        .iter()
        .any(|r| r.name == "自定义仓库名" && r.memory_count == 2));
    assert_eq!(state.workspace_root().unwrap(), Some(target.clone()));
    let selected: SavedWorkspace =
        serde_json::from_slice(&fs::read(&state.settings_path).unwrap()).unwrap();
    assert_eq!(selected.root, target);
    for (id, raw) in &records {
        let vault = state.vault.lock().unwrap();
        let memory = vault.as_ref().unwrap().read(id).unwrap();
        assert!(Path::new(&memory.summary.path).starts_with(&target));
        assert_eq!(&fs::read_to_string(memory.summary.path).unwrap(), raw);
    }
    assert_eq!(
        state
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .graph()
            .unwrap()
            .edges
            .len(),
        1
    );
    {
        let pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_ref().unwrap();
        assert_eq!(
            pipeline.meta("enabled_at").unwrap().as_deref(),
            Some("2026-11-21T08:00:00Z")
        );
        assert_eq!(
            pipeline.cursors().unwrap().values().next().unwrap().offset,
            321
        );
        assert!(pipeline.next_embedding().unwrap().is_none());
        assert!(pipeline.next_turn().unwrap().is_none());
        let after = pipeline.status(true, None, false, None).unwrap();
        assert_eq!(
            (after.processed_turns, after.written_memories),
            (before.processed_turns, before.written_memories)
        );
        assert_eq!(
            (
                after.chat_input_tokens,
                after.chat_output_tokens,
                after.embedding_input_tokens
            ),
            (17, 9, 14)
        );
    }
    drop(state);
    let reopened = Vault::open(&target).unwrap();
    assert_eq!(reopened.workspace().unwrap().repositories.len(), 2);
    assert_eq!(reopened.graph().unwrap().nodes.len(), 2);
    let result = SearchEngine::open(&target)
        .unwrap()
        .search(
            SearchRequest {
                mode: Default::default(),
                group_of: None,
                offset: None,
                query: "迁移".into(),
                repository_id: None,
                limit: None,
            },
            &service(),
        )
        .await
        .unwrap();
    assert!(result.query_vector_cached);
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.embedding_input_tokens, 0);
    assert_eq!(
        db.query_row("SELECT count(*) FROM api_usage", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let target_db = Connection::open(target.join(".aimemory/pipeline.sqlite")).unwrap();
    assert_eq!(
        target_db
            .query_row("SELECT count(*) FROM api_usage", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let old = Vault::open(&source).unwrap();
    assert_eq!(old.graph().unwrap().nodes.len(), 2);
}

#[test]
fn migrates_a_library_containing_only_an_empty_repository_and_same_directory_is_noop() {
    let directory = fixture();
    let state = state_in(directory.path());
    state
        .vault
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .add_repository(RepositoryInput {
            name: "尚未写入记忆的仓库".into(),
            remote: "https://github.com/example/new".into(),
        })
        .unwrap();
    let before = fs::read(&state.settings_path).unwrap();
    let source = state.workspace_root().unwrap().unwrap();
    assert_eq!(migrate(&state, &source).unwrap().repositories.len(), 1);
    assert_eq!(fs::read(&state.settings_path).unwrap(), before);
    let target = destination(directory.path());
    let workspace = migrate(&state, &target).unwrap();
    assert_eq!(
        (
            workspace.repositories.len(),
            workspace.repositories[0].memory_count
        ),
        (1, 0)
    );
    assert_eq!(
        Vault::open(&target)
            .unwrap()
            .workspace()
            .unwrap()
            .repositories[0]
            .name,
        "尚未写入记忆的仓库"
    );
}

#[test]
fn refuses_occupied_or_nested_destinations_without_changing_selection() {
    let directory = fixture();
    let state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    let saved = fs::read(&state.settings_path).unwrap();
    let target = destination(directory.path());
    fs::write(target.join("existing.md"), "保留目标内容").unwrap();
    assert!(migrate(&state, &target).unwrap_err().contains("不是空目录"));
    assert_eq!(
        fs::read_to_string(target.join("existing.md")).unwrap(),
        "保留目标内容"
    );
    let nested = source.join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(migrate(&state, &nested).unwrap_err().contains("内部"));
    assert_eq!(state.workspace_root().unwrap(), Some(source));
    assert_eq!(fs::read(&state.settings_path).unwrap(), saved);
}

#[test]
fn failed_copy_validation_does_not_publish_or_switch_the_library() {
    let directory = fixture();
    let state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    let saved = fs::read(&state.settings_path).unwrap();
    fs::write(source.join(".aimemory/repositories.json"), "invalid JSON").unwrap();
    let target = destination(directory.path());
    assert!(migrate(&state, &target)
        .unwrap_err()
        .contains("仓库配置格式错误"));
    assert!(fs::read_dir(target).unwrap().next().is_none());
    assert_eq!(state.workspace_root().unwrap(), Some(source));
    assert_eq!(fs::read(&state.settings_path).unwrap(), saved);
}

#[cfg(unix)]
#[test]
fn a_real_file_copy_failure_keeps_the_source_selected() {
    let directory = fixture();
    let state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    let saved = fs::read(&state.settings_path).unwrap();
    std::os::unix::fs::symlink(source.join("missing"), source.join("broken-link")).unwrap();
    let target = destination(directory.path());
    assert!(migrate(&state, &target).unwrap_err().contains("无法复制"));
    assert!(fs::read_dir(target).unwrap().next().is_none());
    assert_eq!(state.workspace_root().unwrap(), Some(source));
    assert_eq!(fs::read(&state.settings_path).unwrap(), saved);
}

#[test]
fn refuses_in_flight_work_then_migrates_after_its_result_is_saved() {
    let directory = fixture();
    let state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    prepare_memories(&state);
    let turn = CodexTurn {
        source_id: "codex:migration-session:turn-2".into(),
        turn_id: "turn-2".into(),
        ..turn()
    };
    {
        let mut pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_mut().unwrap();
        pipeline
            .ingest(
                Path::new(&turn.session_path),
                &RolloutCursor::default(),
                std::slice::from_ref(&turn),
            )
            .unwrap();
        pipeline.mark_extracting(&turn.source_id).unwrap();
    }
    let target = destination(directory.path());
    assert!(migrate(&state, &target).unwrap_err().contains("正在处理"));
    assert_eq!(state.workspace_root().unwrap(), Some(source));
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    {
        let pipeline = state.pipeline.lock().unwrap();
        pipeline
            .as_ref()
            .unwrap()
            .extraction_complete(&turn.source_id, &extraction())
            .unwrap();
    }
    migrate(&state, &target).unwrap();
    let mut pipeline = state.pipeline.lock().unwrap();
    let pipeline = pipeline.as_mut().unwrap();
    let mut vault = state.vault.lock().unwrap();
    assert!(pipeline.materialize(vault.as_mut().unwrap()).unwrap());
    assert_eq!(
        pipeline
            .status(false, None, false, None)
            .unwrap()
            .chat_input_tokens,
        34
    );
}

#[test]
fn refuses_a_running_semantic_query_before_copying() {
    let directory = fixture();
    let state = state_in(directory.path());
    let target = destination(directory.path());
    let query = state.workspace_transition.try_read().unwrap();
    assert!(migrate(&state, &target)
        .unwrap_err()
        .contains("检索正在处理"));
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    drop(query);
    migrate(&state, &target).unwrap();
}

#[test]
fn waits_for_an_embedding_result_and_keeps_the_remaining_queue() {
    let directory = fixture();
    let state = state_in(directory.path());
    prepare_memories(&state);
    let job = {
        let vault = state.vault.lock().unwrap();
        let pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_ref().unwrap();
        pipeline
            .sync_embeddings(vault.as_ref().unwrap(), "next-model", true)
            .unwrap();
        pipeline.next_embedding().unwrap().unwrap()
    };
    let target = destination(directory.path());
    assert!(migrate(&state, &target).unwrap_err().contains("正在处理"));
    state
        .pipeline
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .embedding_complete(
            &job,
            Ok(EmbeddingResult {
                vector: vec![0.0, 1.0],
                usage: ApiUsage {
                    input_tokens: 5,
                    output_tokens: 0,
                    reported: true,
                },
                latency_ms: 2,
            }),
        )
        .unwrap();
    migrate(&state, &target).unwrap();
    let mut memory = state
        .vault
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .read(&job.memory_id)
        .unwrap();
    let pipeline = state.pipeline.lock().unwrap();
    let pipeline = pipeline.as_ref().unwrap();
    pipeline.annotate(&mut memory.summary).unwrap();
    assert_eq!(memory.summary.embedding_status, "ready");
    let status = pipeline.status(false, None, false, None).unwrap();
    assert_eq!(status.pending_embeddings, 1);
    assert_eq!(status.embedding_input_tokens, 19);
}

#[test]
fn failure_to_persist_workspace_configuration_does_not_switch_memory_state() {
    let directory = fixture();
    let mut state = state_in(directory.path());
    let source = state.workspace_root().unwrap().unwrap();
    let settings_path = state.settings_path.clone();
    let saved = fs::read(&settings_path).unwrap();
    let blocked = directory.path().join("blocked-settings");
    fs::write(&blocked, "not a directory").unwrap();
    state.settings_path = blocked.join("workspace.json");
    let target = destination(directory.path());
    assert!(migrate(&state, &target).is_err());
    assert_eq!(state.workspace_root().unwrap(), Some(source));
    assert_eq!(fs::read(&settings_path).unwrap(), saved);
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    state.settings_path = settings_path;
    migrate(&state, &target).unwrap();
    assert_eq!(state.workspace_root().unwrap(), Some(target));
}

#[test]
fn settings_views_follow_existing_library_baselines_without_resetting_cursors() {
    let directory = fixture();
    let state = state_in(directory.path());
    let first = state.workspace_root().unwrap().unwrap();
    prepare_memories(&state);
    state
        .settings
        .lock()
        .unwrap()
        .set_archive_started_at(Some("1999-01-01T00:00:00Z".into()))
        .unwrap();
    let second = directory.path().join("existing-library");
    let _vault = Vault::open(&second).unwrap();
    let mut pipeline = Pipeline::open(&second).unwrap();
    let cursor: RolloutCursor = serde_json::from_value(serde_json::json!({
        "offset":987,"session":null,"active":null
    }))
    .unwrap();
    pipeline
        .baseline(
            "/example/codex",
            "2026-11-21T09:00:00Z",
            &[(PathBuf::from("/example/codex/rollout-second.jsonl"), cursor)],
        )
        .unwrap();
    assert_eq!(
        crate::settings_view(&state)
            .unwrap()
            .archive_started_at
            .as_deref(),
        Some("2026-11-21T08:00:00Z")
    );
    open(&state, Some(second.to_string_lossy().into())).unwrap();
    assert_eq!(
        crate::settings_view(&state)
            .unwrap()
            .archive_started_at
            .as_deref(),
        Some("2026-11-21T09:00:00Z")
    );
    assert_eq!(
        state
            .pipeline
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .cursors()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .offset,
        987
    );
    open(&state, Some(first.to_string_lossy().into())).unwrap();
    assert_eq!(
        crate::settings_view(&state)
            .unwrap()
            .archive_started_at
            .as_deref(),
        Some("2026-11-21T08:00:00Z")
    );
    let current = state.pipeline.lock().unwrap();
    let current = current.as_ref().unwrap();
    assert_eq!(
        current.cursors().unwrap().values().next().unwrap().offset,
        321
    );
    assert!(current.next_embedding().unwrap().is_none());
    assert_eq!(
        pipeline.cursors().unwrap().values().next().unwrap().offset,
        987
    );
}

#[test]
fn activating_archive_always_persists_the_new_baseline_even_when_one_was_saved_before() {
    let directory = fixture();
    let state = state_in(directory.path());
    let previous = {
        let mut settings = state.settings.lock().unwrap();
        settings
            .set_archive_started_at(Some("1999-01-01T00:00:00Z".into()))
            .unwrap();
        settings.config().clone()
    };
    fs::create_dir_all(Path::new(&previous.codex_directory).join("sessions")).unwrap();
    let view = crate::apply_settings_change(
        &state,
        crate::SettingsChange::Import(crate::transfer::PortableConfig {
            chat: crate::transfer::PortableService {
                provider: None,
                base_url: previous.chat.base_url,
                model: previous.chat.model,
            },
            embedding: crate::transfer::PortableService {
                provider: None,
                base_url: previous.embedding.base_url,
                model: previous.embedding.model,
            },
            archive_enabled: true,
            keep_in_tray: false,
            theme: None,
        }),
    )
    .unwrap();
    let baseline = state
        .pipeline
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .meta("enabled_at")
        .unwrap();
    assert_ne!(baseline.as_deref(), Some("1999-01-01T00:00:00Z"));
    assert_eq!(view.archive_started_at, baseline);
    assert_eq!(
        state.settings.lock().unwrap().config().archive_started_at,
        baseline
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(state.data_directory.join("services.json")).unwrap())
            .unwrap();
    assert_eq!(saved["archiveStartedAt"].as_str(), baseline.as_deref());
}
