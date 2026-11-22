use crate::{
    codex::{self, CodexTurn, RolloutCursor},
    config::{AppConfig, ServiceConfig, ServiceCredentials, ServiceKind},
    model_api::{self, ArchiveTopic},
    pipeline::EmbeddingJob,
    AppState,
};
use chrono::Utc;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Duration,
};
use tauri::{AppHandle, Manager};

struct ChatJob {
    root: PathBuf,
    turn: CodexTurn,
    topics: Vec<ArchiveTopic>,
    credentials: ServiceCredentials,
}

pub fn embedding_ready(service: &ServiceConfig) -> bool {
    service.has_api_key
        || url::Url::parse(&service.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .as_deref()
            != Some("api.openai.com")
}

pub fn embedding_key(service: &ServiceConfig) -> String {
    crate::config::service_fingerprint(&service.base_url, &service.model)
}

pub fn initialize_baseline(
    state: &AppState,
    root: &Path,
    directory: &str,
) -> Result<String, String> {
    let cursors = collect_baseline(directory)?;
    let enabled_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    state.with_workspace(root, |_, pipeline| {
        pipeline.baseline(directory, &enabled_at, &cursors)
    })?;
    Ok(enabled_at)
}

pub fn collect_baseline(directory: &str) -> Result<Vec<(PathBuf, RolloutCursor)>, String> {
    let paths = codex::list_rollouts(Path::new(directory))?;
    paths
        .iter()
        .map(|path| Ok((path.clone(), codex::snapshot_rollout(path)?)))
        .collect()
}

pub fn start(app: &AppHandle) {
    let chat_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let prepared = {
                let handle = chat_app.clone();
                tauri::async_runtime::spawn_blocking(move || prepare_chat(&handle)).await
            };
            match prepared {
                Ok(Ok(Some(ChatJob {
                    root,
                    turn,
                    topics,
                    credentials,
                }))) => {
                    let result = model_api::chat_extract(&credentials, &turn, &topics).await;
                    let state = chat_app.state::<AppState>();
                    let config = state.settings.lock().unwrap().config().clone();
                    let committed = state.with_workspace(&root, |vault, pipeline| {
                        match result {
                            Ok(extraction) => {
                                pipeline.extraction_complete(&turn.source_id, &extraction)?;
                                if pipeline.materialize(vault)? {
                                    pipeline.sync_embeddings(
                                        vault,
                                        &embedding_key(&config.embedding),
                                        embedding_ready(&config.embedding),
                                    )?;
                                }
                            }
                            Err(error) => pipeline.extraction_rejected(&turn.source_id, &error)?,
                        }
                        Ok(())
                    });
                    if let Err(error) = committed {
                        set_error(&state, error);
                    }
                }
                Ok(Err(error)) => set_error(&chat_app.state::<AppState>(), error),
                Err(error) => set_error(&chat_app.state::<AppState>(), error.to_string()),
                Ok(Ok(None)) => {}
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let embedding_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let prepared = prepare_embedding(&embedding_app);
            match prepared {
                Ok(Some((root, job, credentials))) => {
                    let result = model_api::embed(&credentials, &job.content).await;
                    let state = embedding_app.state::<AppState>();
                    if let Err(error) = state.with_workspace(&root, |_, pipeline| {
                        pipeline.embedding_complete(&job, result)
                    }) {
                        set_error(&state, error);
                    }
                }
                Err(error) => set_error(&embedding_app.state::<AppState>(), error),
                Ok(None) => {}
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

fn prepare_chat(app: &AppHandle) -> Result<Option<ChatJob>, String> {
    let state = app.state::<AppState>();
    let Ok(_workspace) = state.workspace_transition.try_read() else {
        return Ok(None);
    };
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    if !config.archive_enabled {
        return Ok(None);
    }
    let Some(root) = state.workspace_root()? else {
        return Ok(None);
    };
    state.scanning.store(true, Ordering::Relaxed);
    let scanned = scan(&state, &root, &config);
    state.scanning.store(false, Ordering::Relaxed);
    scanned?;
    if let Ok(mut error) = state.last_error.lock() {
        *error = None;
    }
    let next = state.with_workspace(&root, |vault, pipeline| {
        if pipeline.materialize(vault)? {
            pipeline.sync_embeddings(
                vault,
                &embedding_key(&config.embedding),
                embedding_ready(&config.embedding),
            )?;
        }
        let Some(turn) = pipeline.next_turn()? else {
            return Ok(None);
        };
        match pipeline.archive_topics(&turn) {
            Ok(topics) => Ok(Some((turn, topics))),
            Err(error) => {
                pipeline.turn_failed(&turn.source_id, &error)?;
                Err(error)
            }
        }
    })?;
    let Some((turn, topics)) = next else {
        return Ok(None);
    };
    let credentials = match state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .credentials(ServiceKind::Chat)
    {
        Ok(credentials) => credentials,
        Err(error) => {
            state.with_workspace(&root, |_, pipeline| {
                pipeline.turn_failed(&turn.source_id, &error)
            })?;
            return Err(error);
        }
    };
    state.with_workspace(&root, |_, pipeline| {
        pipeline.mark_extracting(&turn.source_id)
    })?;
    Ok(Some(ChatJob {
        root,
        turn,
        topics,
        credentials,
    }))
}

fn scan(state: &AppState, root: &Path, config: &AppConfig) -> Result<(), String> {
    let initialized = state.with_workspace(root, |_, pipeline| {
        Ok(pipeline.meta("codex_directory")?.as_deref() == Some(&config.codex_directory))
    })?;
    if !initialized {
        initialize_baseline(state, root, &config.codex_directory)?;
    }
    let (enabled_at, cursors) = state.with_workspace(root, |_, pipeline| {
        Ok((
            pipeline.meta("enabled_at")?.ok_or("归档起点尚未建立")?,
            pipeline.cursors()?,
        ))
    })?;
    for path in codex::list_rollouts(Path::new(&config.codex_directory))? {
        let cursor = cursors
            .get(&path)
            .cloned()
            .unwrap_or_else(RolloutCursor::default);
        if fs::metadata(&path).map_err(|e| e.to_string())?.len() == cursor.offset {
            continue;
        }
        let batch = codex::read_rollout(&path, &cursor, &enabled_at)?;
        state.with_workspace(root, |_, pipeline| {
            if pipeline.meta("enabled_at")?.as_deref() != Some(&enabled_at) {
                return Ok(());
            }
            pipeline.ingest(&path, &batch.cursor, &batch.turns)?;
            if let Some(warning) = batch.warnings.last() {
                pipeline.set_meta("scan_warning", warning)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn prepare_embedding(
    app: &AppHandle,
) -> Result<Option<(PathBuf, EmbeddingJob, ServiceCredentials)>, String> {
    let state = app.state::<AppState>();
    let Ok(_workspace) = state.workspace_transition.try_read() else {
        return Ok(None);
    };
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    if !embedding_ready(&config.embedding) {
        return Ok(None);
    }
    let Some(root) = state.workspace_root()? else {
        return Ok(None);
    };
    let job = state.with_workspace(&root, |_, pipeline| pipeline.next_embedding())?;
    let Some(job) = job else {
        return Ok(None);
    };
    let credentials = match state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .credentials(ServiceKind::Embedding)
    {
        Ok(credentials) => credentials,
        Err(error) => {
            state.with_workspace(&root, |_, pipeline| {
                pipeline.embedding_configuration_failed(&job.service_key, &error)
            })?;
            return Err(error);
        }
    };
    Ok(Some((root, job, credentials)))
}

fn set_error(state: &AppState, message: String) {
    if let Ok(mut error) = state.last_error.lock() {
        *error = Some(message);
    }
}
