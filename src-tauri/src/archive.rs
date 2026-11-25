use crate::{
    codex::{self, CodexTurn, RolloutCursor},
    config::{AppConfig, ServiceConfig, ServiceCredentials, ServiceKind},
    harness,
    model_api::{self, ArchiveTopic},
    pipeline::{EmbeddingJob, Pipeline},
    AppState,
};
use chrono::Utc;

#[cfg(test)]
mod tests;
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

pub enum SourceBaseline {
    Codex(String, Vec<(PathBuf, RolloutCursor)>),
    Harness(String, Vec<(PathBuf, harness::Cursor)>),
}

impl SourceBaseline {
    pub fn apply(&self, pipeline: &mut Pipeline, enabled_at: &str) -> Result<(), String> {
        match self {
            Self::Codex(directory, cursors) => pipeline.baseline(directory, enabled_at, cursors),
            Self::Harness(directory, cursors) => {
                pipeline.baseline_harness(directory, enabled_at, cursors)
            }
        }
    }
}

pub fn collect_activations(
    previous: &AppConfig,
    next: &AppConfig,
) -> Result<Vec<SourceBaseline>, String> {
    let mut baselines = Vec::new();
    if next.archives("codex")
        && (!previous.archives("codex") || previous.codex_directory != next.codex_directory)
    {
        baselines.push(SourceBaseline::Codex(
            next.codex_directory.clone(),
            collect_baseline(&next.codex_directory)?,
        ));
    }
    let harness_directory = harness::root_directory(Path::new(&next.harness_directory))
        .display()
        .to_string();
    let previous_harness = harness::root_directory(Path::new(&previous.harness_directory));
    if next.archives("harness")
        && (!previous.archives("harness") || previous_harness != Path::new(&harness_directory))
    {
        baselines.push(SourceBaseline::Harness(
            harness_directory.clone(),
            collect_harness_baseline(&harness_directory)?,
        ));
    }
    Ok(baselines)
}

pub fn started_at(pipeline: &Pipeline, config: &AppConfig) -> Result<Option<String>, String> {
    let mut times = Vec::new();
    for (source, key) in [("codex", "enabled_at"), ("harness", "harness_enabled_at")] {
        if config
            .archive_sources
            .iter()
            .any(|selected| selected == source)
        {
            if let Some(time) = pipeline.meta(key)? {
                times.push(time);
            }
        }
    }
    Ok(times.into_iter().min())
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
    codex::list_rollouts(Path::new(directory))?
        .iter()
        .map(|path| Ok((path.clone(), codex::snapshot_rollout(path)?)))
        .collect()
}

fn collect_harness_baseline(directory: &str) -> Result<Vec<(PathBuf, harness::Cursor)>, String> {
    harness::list_sessions(Path::new(directory))?
        .iter()
        .map(|path| Ok((path.clone(), harness::snapshot_session(path)?)))
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
    if let Ok(mut error) = state.last_error.lock() {
        *error = if scanned.is_empty() {
            None
        } else {
            Some(scanned.join("；"))
        };
    }
    let next = state.with_workspace(&root, |vault, pipeline| {
        if pipeline.materialize(vault)? {
            pipeline.sync_embeddings(
                vault,
                &embedding_key(&config.embedding),
                embedding_ready(&config.embedding),
            )?;
        }
        let Some(turn) = pipeline.next_turn_for_sources(&config.archive_sources)? else {
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

fn scan(state: &AppState, root: &Path, config: &AppConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if config.archives("codex") {
        if let Err(error) = scan_codex(state, root, config) {
            errors.push(format!("Codex：{error}"));
        }
    }
    if config.archives("harness") {
        if let Err(error) = scan_harness(state, root, config) {
            errors.push(format!("DeepSeek Harness：{error}"));
        }
    }
    errors
}

fn scan_codex(state: &AppState, root: &Path, config: &AppConfig) -> Result<(), String> {
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

fn scan_harness(state: &AppState, root: &Path, config: &AppConfig) -> Result<(), String> {
    let directory = harness::root_directory(Path::new(&config.harness_directory))
        .display()
        .to_string();
    let initialized = state.with_workspace(root, |_, pipeline| {
        Ok(pipeline.meta("harness_directory")?.as_deref() == Some(&directory))
    })?;
    if !initialized {
        let baseline =
            SourceBaseline::Harness(directory.clone(), collect_harness_baseline(&directory)?);
        let enabled_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        state.with_workspace(root, |_, pipeline| baseline.apply(pipeline, &enabled_at))?;
    }
    let (enabled_at, cursors) = state.with_workspace(root, |_, pipeline| {
        Ok((
            pipeline
                .meta("harness_enabled_at")?
                .ok_or("Harness 归档起点尚未建立")?,
            pipeline.harness_cursors()?,
        ))
    })?;
    let mut errors = Vec::new();
    for path in harness::list_sessions(Path::new(&directory))? {
        let cursor = cursors.get(&path).cloned().unwrap_or_default();
        match harness::read_turns(&path, &cursor, &enabled_at) {
            Ok(batch) => state.with_workspace(root, |_, pipeline| {
                if pipeline.meta("harness_enabled_at")?.as_deref() != Some(&enabled_at) {
                    return Ok(());
                }
                pipeline.ingest_harness(&path, &batch.cursor, &batch.turns)?;
                if let Some(warning) = batch.warnings.last() {
                    pipeline.set_meta("scan_warning", warning)?;
                }
                Ok(())
            })?,
            Err(error) => errors.push(format!("{}：{error}", path.display())),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
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
