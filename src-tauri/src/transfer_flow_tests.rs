use super::*;
use crate::{
    config::{AppConfig, ServiceConfig},
    model_api::{ApiUsage, EmbeddingResult},
};
use serde_json::json;

fn state_in(directory: &Path, started_at: Option<String>) -> AppState {
    let data = directory.join("settings");
    let root = directory.join("vault");
    let codex = directory.join("local-codex");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(codex.join("sessions")).unwrap();
    let config = AppConfig {
        chat: ServiceConfig {
            provider: None,
            base_url: "http://127.0.0.1:19001/v1".into(),
            model: "chat-local".into(),
            has_api_key: true,
        },
        embedding: ServiceConfig {
            provider: None,
            base_url: "http://127.0.0.1:19002/v1".into(),
            model: "embedding-local".into(),
            has_api_key: true,
        },
        codex_directory: codex.display().to_string(),
        harness_directory: directory.join("harness").display().to_string(),
        archive_sources: vec!["codex".into()],
        archive_enabled: false,
        archive_started_at: started_at.clone(),
        run_in_background: true,
    };
    fs::write(
        data.join("services.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let settings = SettingsStore::load(&data, codex).unwrap();
    let vault = Vault::open(&root).unwrap();
    let mut pipeline = Pipeline::open(&vault.root).unwrap();
    if let Some(started_at) = started_at {
        pipeline
            .baseline(&config.codex_directory, &started_at, &[])
            .unwrap();
    }
    AppState {
        settings_path: data.join("workspace.json"),
        default_root: vault.root.clone(),
        vault: Mutex::new(Some(vault)),
        settings: Mutex::new(settings),
        pipeline: Mutex::new(Some(pipeline)),
        recovered_roots: Mutex::new(BTreeSet::new()),
        scanning: AtomicBool::new(false),
        last_error: Mutex::new(None),
        search: Mutex::new(None),
        data_directory: data,
        transfers: Mutex::new(transfer::TransferState::default()),
        workspace_transition: tokio::sync::RwLock::new(()),
    }
}

fn preview_token(state: &AppState, kind: transfer::TransferKind, path: &Path) -> String {
    let config = state.settings.lock().unwrap().config().clone();
    let theme = theme::load(&state.data_directory).unwrap();
    let vault = state.vault.lock().unwrap();
    state
        .transfers
        .lock()
        .unwrap()
        .preview(kind, path, vault.as_ref().unwrap(), &config, &theme)
        .unwrap()
        .token
}

fn write_memory_package(path: &Path) {
    fs::write(path, json!({
        "type":"aimemory.memories", "version":1,
        "memories":[{"id":"IMPORTED-A","repository":"https://github.com/example/frontend-web.git","markdown":"# 导入决定\n\n## 结论\n导入后先读正文，向量后台生成。\n"}],
        "links":[]
    }).to_string()).unwrap();
}

#[test]
fn memory_import_flow_updates_workspace_and_reuses_ready_vector_on_repeat() {
    let directory = tempfile::tempdir().unwrap();
    let state = state_in(directory.path(), None);
    let packet = directory.path().join("memories.json");
    write_memory_package(&packet);
    let token = preview_token(&state, transfer::TransferKind::Memories, &packet);
    {
        let pipeline = state.pipeline.lock().unwrap();
        assert_eq!(
            pipeline
                .as_ref()
                .unwrap()
                .status(false, None, false, None)
                .unwrap()
                .pending_embeddings,
            0
        );
    }
    let result = apply_transfer(&state, &token).unwrap();
    assert_eq!(result.kind, transfer::TransferKind::Memories);
    assert_eq!((result.added_memories, result.existing_memories), (1, 0));
    assert_eq!(
        result
            .workspace
            .unwrap()
            .repositories
            .iter()
            .map(|repository| repository.memory_count)
            .sum::<usize>(),
        1
    );
    let original = {
        let vault = state.vault.lock().unwrap();
        let memory = vault.as_ref().unwrap().read("IMPORTED-A").unwrap();
        assert!(memory.markdown.contains("先读正文"));
        fs::read_to_string(memory.summary.path).unwrap()
    };
    let vector_before = {
        let mut pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_mut().unwrap();
        assert_eq!(
            pipeline
                .status(false, None, false, None)
                .unwrap()
                .pending_embeddings,
            1
        );
        let job = pipeline.next_embedding().unwrap().unwrap();
        assert_eq!(job.memory_id, "IMPORTED-A");
        pipeline
            .embedding_complete(
                &job,
                Ok(EmbeddingResult {
                    vector: vec![0.25, -0.5, 1.0],
                    usage: ApiUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                        reported: true,
                    },
                    latency_ms: 0,
                }),
            )
            .unwrap();
        assert_eq!(
            pipeline
                .status(false, None, false, None)
                .unwrap()
                .pending_embeddings,
            0
        );
        rusqlite::Connection::open(state.default_root.join(".aimemory/pipeline.sqlite"))
            .unwrap()
            .query_row(
                "SELECT vector FROM embeddings WHERE memory_id='IMPORTED-A'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap()
    };
    let token = preview_token(&state, transfer::TransferKind::Memories, &packet);
    let repeated = apply_transfer(&state, &token).unwrap();
    assert_eq!(
        (
            repeated.added_memories,
            repeated.existing_memories,
            repeated.new_links
        ),
        (0, 1, 0)
    );
    assert_eq!(
        repeated
            .workspace
            .unwrap()
            .repositories
            .iter()
            .map(|repository| repository.memory_count)
            .sum::<usize>(),
        1
    );
    {
        let pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_ref().unwrap();
        assert!(pipeline.next_embedding().unwrap().is_none());
        let mut memory = state
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .read("IMPORTED-A")
            .unwrap();
        pipeline.annotate(&mut memory.summary).unwrap();
        assert_eq!(memory.summary.embedding_status, "ready");
        assert_eq!(fs::read_to_string(memory.summary.path).unwrap(), original);
    }
    let vector_after: Vec<u8> =
        rusqlite::Connection::open(state.default_root.join(".aimemory/pipeline.sqlite"))
            .unwrap()
            .query_row(
                "SELECT vector FROM embeddings WHERE memory_id='IMPORTED-A'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(vector_before, vector_after);
}

#[test]
fn config_import_flow_builds_local_baseline_and_preserves_keys_paths_theme_and_progress() {
    let directory = tempfile::tempdir().unwrap();
    let state = state_in(directory.path(), None);
    let local = state.settings.lock().unwrap().config().clone();
    let sessions = Path::new(&local.codex_directory).join("sessions");
    let rollout = sessions.join("rollout-existing.jsonl");
    fs::write(&rollout, format!("{}\n", json!({"timestamp":"2026-11-20T00:00:00Z","type":"session_meta","payload":{"id":"existing-session","thread_source":"user","cwd":"/example/repository","git":{"repository_url":"https://github.com/example/repository.git"}}}))).unwrap();
    let original_offset = fs::metadata(&rollout).unwrap().len();
    let dark = theme::ThemeConfig {
        preset: "石墨夜色".into(),
        accent: "#90AFFF".into(),
        background: "#202227".into(),
        foreground: "#F2F3F5".into(),
        font: "system".into(),
    };
    theme::save(&state.data_directory, dark.clone()).unwrap();
    let memories = directory.path().join("memories.json");
    write_memory_package(&memories);
    let token = preview_token(&state, transfer::TransferKind::Memories, &memories);
    apply_transfer(&state, &token).unwrap();
    let packet = directory.path().join("config.json");
    fs::write(&packet, json!({
        "type":"aimemory.config", "version":1,
        "config":{"chat":{"baseUrl":"http://127.0.0.1:19003/v1","model":"chat-imported"},"embedding":{"baseUrl":"http://127.0.0.1:19004/v1","model":"embedding-imported"},"archiveEnabled":true,"keepInTray":false}
    }).to_string()).unwrap();
    let token = preview_token(&state, transfer::TransferKind::Config, &packet);
    let result = apply_transfer(&state, &token).unwrap();
    let settings = result.settings.unwrap();
    assert_eq!(result.kind, transfer::TransferKind::Config);
    assert!(settings.archive_enabled);
    assert!(!settings.run_in_background);
    assert_eq!(settings.codex_directory, local.codex_directory);
    assert!(settings.chat.has_api_key && settings.embedding.has_api_key);
    assert_eq!(settings.chat.model, "chat-imported");
    assert_eq!(settings.embedding.model, "embedding-imported");
    assert!(settings.archive_started_at.is_some());
    assert!(result.theme.is_none());
    assert_eq!(theme::load(&state.data_directory).unwrap(), dark);
    let enabled_at = {
        let pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_ref().unwrap();
        assert_eq!(
            pipeline.meta("codex_directory").unwrap().as_deref(),
            Some(local.codex_directory.as_str())
        );
        let enabled_at = pipeline.meta("enabled_at").unwrap().unwrap();
        assert_eq!(
            settings.archive_started_at.as_deref(),
            Some(enabled_at.as_str())
        );
        assert_eq!(
            pipeline.cursors().unwrap().get(&rollout).unwrap().offset,
            original_offset
        );
        assert!(pipeline.next_turn().unwrap().is_none());
        assert_eq!(
            pipeline
                .status(true, settings.archive_started_at.clone(), false, None)
                .unwrap()
                .pending_embeddings,
            1
        );
        assert_eq!(
            pipeline.next_embedding().unwrap().unwrap().service_key,
            archive::embedding_key(&settings.embedding)
        );
        enabled_at
    };
    let bytes = format!(
        "{}\n",
        json!({"timestamp":"2026-11-22T00:00:00Z","type":"event_msg","payload":{"type":"token_count"}})
    );
    {
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(bytes.as_bytes())
            .unwrap();
    }
    let advanced_offset = {
        let mut pipeline = state.pipeline.lock().unwrap();
        let pipeline = pipeline.as_mut().unwrap();
        let previous = pipeline.cursors().unwrap().remove(&rollout).unwrap();
        let batch = codex::read_rollout(&rollout, &previous, &enabled_at).unwrap();
        assert!(batch.cursor.offset > previous.offset);
        pipeline
            .ingest(&rollout, &batch.cursor, &batch.turns)
            .unwrap();
        batch.cursor.offset
    };
    let token = preview_token(&state, transfer::TransferKind::Config, &packet);
    let repeated = apply_transfer(&state, &token).unwrap().settings.unwrap();
    assert_eq!(repeated.archive_started_at, settings.archive_started_at);
    let pipeline = state.pipeline.lock().unwrap();
    let pipeline = pipeline.as_ref().unwrap();
    assert_eq!(
        pipeline.meta("enabled_at").unwrap().as_deref(),
        Some(enabled_at.as_str())
    );
    assert_eq!(
        pipeline.cursors().unwrap().get(&rollout).unwrap().offset,
        advanced_offset
    );
    assert_eq!(theme::load(&state.data_directory).unwrap(), dark);
}

#[test]
fn config_theme_and_standalone_theme_import_flow_persist_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let started_at = "2026-11-19T00:00:00Z".to_string();
    let state = state_in(directory.path(), Some(started_at.clone()));
    let config = state.settings.lock().unwrap().config().clone();
    let paper = theme::ThemeConfig {
        preset: "暖纸".into(),
        accent: "#9B5421".into(),
        background: "#FBF6EB".into(),
        foreground: "#352B23".into(),
        font: "serif".into(),
    };
    let packet = directory.path().join("config-theme.json");
    transfer::export_config(&packet, &config, &paper).unwrap();
    let token = preview_token(&state, transfer::TransferKind::Config, &packet);
    assert_eq!(
        theme::load(&state.data_directory).unwrap(),
        theme::ThemeConfig::default()
    );
    let result = apply_transfer(&state, &token).unwrap();
    assert_eq!(result.theme, Some(paper.clone()));
    assert_eq!(theme::load(&state.data_directory).unwrap(), paper);
    let settings = result.settings.unwrap();
    assert_eq!(
        settings.archive_started_at.as_deref(),
        Some(started_at.as_str())
    );
    assert_eq!(settings.codex_directory, config.codex_directory);
    assert!(settings.chat.has_api_key && settings.embedding.has_api_key);
    let separate = directory.path().join("theme.json");
    transfer::export_theme(&separate, &theme::ThemeConfig::default()).unwrap();
    let token = preview_token(&state, transfer::TransferKind::Theme, &separate);
    let result = apply_transfer(&state, &token).unwrap();
    assert_eq!(result.kind, transfer::TransferKind::Theme);
    assert!(result.settings.is_none());
    assert_eq!(result.theme, Some(theme::ThemeConfig::default()));
    assert_eq!(
        theme::load(&state.data_directory).unwrap(),
        theme::ThemeConfig::default()
    );
    let reopened = SettingsStore::load(
        &state.data_directory,
        directory.path().join("different-codex"),
    )
    .unwrap();
    assert_eq!(
        reopened.config().archive_started_at.as_deref(),
        Some(started_at.as_str())
    );
    assert_eq!(reopened.config().codex_directory, config.codex_directory);
    assert!(reopened.config().chat.has_api_key && reopened.config().embedding.has_api_key);
}
