mod archive;
pub mod code_context;
pub mod codex;
pub mod config;
mod git;
pub mod harness;
#[cfg(test)]
mod integration_tests;
pub mod mcp;
mod mcp_connection;
mod mcp_http;
mod memory_markdown;
pub mod model_api;
pub mod pipeline;
pub mod search;
pub mod storage;
pub mod theme;
pub mod transfer;
#[cfg(test)]
mod transfer_flow_tests;
mod update;
pub mod usage;
mod workspace;

use config::{ServiceInput, ServiceKind, SettingsStore, SettingsView, UpdateSettings};
use pipeline::{ArchiveStatus, Pipeline};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use storage::{
    MemoryGraph, MemoryInput, MemoryRecord, MemorySummary, Repository, RepositoryInput, Vault,
    Workspace,
};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    vault: Mutex<Option<Vault>>,
    settings_path: PathBuf,
    default_root: PathBuf,
    settings: Mutex<SettingsStore>,
    pipeline: Mutex<Option<Pipeline>>,
    recovered_roots: Mutex<BTreeSet<PathBuf>>,
    scanning: AtomicBool,
    last_error: Mutex<Option<String>>,
    search: Mutex<Option<Arc<search::SearchEngine>>>,
    data_directory: PathBuf,
    transfers: Mutex<transfer::TransferState>,
    workspace_transition: tokio::sync::RwLock<()>,
}

impl AppState {
    fn workspace_root(&self) -> Result<Option<PathBuf>, String> {
        Ok(self
            .vault
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|vault| vault.root.clone()))
    }

    fn with_workspace<T>(
        &self,
        root: &Path,
        work: impl FnOnce(&mut Vault, &mut Pipeline) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut current = self.vault.lock().map_err(|e| e.to_string())?;
        let mut pipeline = self.pipeline.lock().map_err(|e| e.to_string())?;
        if let (Some(vault), Some(pipeline)) = (current.as_mut(), pipeline.as_mut()) {
            if vault.root == root {
                return work(vault, pipeline);
            }
        }
        // Requests already sent finish in the vault where they started.
        let mut vault = Vault::open(root)?;
        let mut pipeline = Pipeline::open(root)?;
        work(&mut vault, &mut pipeline)
    }
}

#[tauri::command]
async fn workspace_open(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Workspace, String> {
    let _transition = state.workspace_transition.write().await;
    workspace::open(&state, path)
}

#[tauri::command]
async fn workspace_migrate(path: String, state: State<'_, AppState>) -> Result<Workspace, String> {
    workspace::migrate(&state, Path::new(&path))
}

#[tauri::command]
async fn workspace_refresh(state: State<'_, AppState>) -> Result<Workspace, String> {
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let vault = vault.as_mut().ok_or("请先打开记忆库")?;
    vault.refresh()?;
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        pipeline.sync_embeddings(
            vault,
            &archive::embedding_key(&config.embedding),
            archive::embedding_ready(&config.embedding),
        )?;
    }
    vault.workspace()
}

#[tauri::command]
async fn repository_add(
    input: RepositoryInput,
    state: State<'_, AppState>,
) -> Result<Repository, String> {
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .as_mut()
        .ok_or("请先打开记忆库")?
        .add_repository(input)
}

#[tauri::command]
async fn repository_rename(
    id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<Workspace, String> {
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let vault = vault.as_mut().ok_or("请先打开记忆库")?;
    vault.rename_repository(&id, &name)?;
    vault.workspace()
}

#[tauri::command]
async fn repository_delete(id: String, state: State<'_, AppState>) -> Result<Workspace, String> {
    let _transition = state.workspace_transition.write().await;
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let vault = vault.as_mut().ok_or("请先打开记忆库")?;
    let pipeline = state.pipeline.lock().map_err(|e| e.to_string())?;
    if let Some(pipeline) = pipeline.as_ref() {
        if pipeline.has_in_flight_requests()? {
            return Err("后台归档请求正在完成，请稍后再删除仓库".into());
        }
    }
    vault.delete_repository(&id)?;
    if let Some(pipeline) = pipeline.as_ref() {
        pipeline.remove_deleted_memories(&id)?;
    }
    vault.workspace()
}

#[tauri::command]
async fn memory_search(
    repository_id: Option<String>,
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<MemorySummary>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mut memories = vault
        .as_ref()
        .ok_or("请先打开记忆库")?
        .search(repository_id.as_deref(), &query)?;
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        for memory in &mut memories {
            pipeline.annotate(memory)?;
        }
    }
    Ok(memories)
}

#[tauri::command]
async fn memory_graph(state: State<'_, AppState>) -> Result<MemoryGraph, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.as_ref().ok_or("请先打开记忆库")?.graph()
}

#[tauri::command]
async fn semantic_search(
    request: search::SearchRequest,
    state: State<'_, AppState>,
) -> Result<search::SearchResponse, String> {
    let _workspace = state.workspace_transition.read().await;
    let engine = state
        .search
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .cloned()
        .ok_or("请先打开记忆库")?;
    let key = archive::embedding_key(
        &state
            .settings
            .lock()
            .map_err(|e| e.to_string())?
            .config()
            .embedding,
    );
    engine
        .search_with(request, Some(key), || {
            let settings = state.settings.lock().map_err(|e| e.to_string())?;
            if !archive::embedding_ready(&settings.config().embedding) {
                return Err("未配置向量服务凭据".into());
            }
            settings.credentials(ServiceKind::Embedding)
        })
        .await
}

#[tauri::command]
async fn mcp_info(state: State<'_, AppState>) -> Result<mcp_connection::McpInfo, String> {
    let root = state.workspace_root()?.ok_or("请先打开记忆库")?;
    Ok(mcp_connection::connection_info(
        &state.data_directory,
        &root,
    ))
}

#[tauri::command]
async fn mcp_http_status(
    runtime: State<'_, mcp_http::HttpRuntime>,
) -> Result<mcp_http::HttpStatus, String> {
    runtime.0.lock().await.status().await
}

#[tauri::command]
async fn mcp_http_start(
    state: State<'_, AppState>,
    runtime: State<'_, mcp_http::HttpRuntime>,
) -> Result<mcp_http::HttpStatus, String> {
    let options = mcp::Options {
        data_dir: state.data_directory.clone(),
        cwd: state.data_directory.clone(),
    };
    runtime.0.lock().await.start(options).await
}

#[tauri::command]
async fn mcp_http_stop(
    runtime: State<'_, mcp_http::HttpRuntime>,
) -> Result<mcp_http::HttpStatus, String> {
    runtime.0.lock().await.stop().await
}

#[tauri::command]
async fn memory_read(id: String, state: State<'_, AppState>) -> Result<MemoryRecord, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mut memory = vault.as_ref().ok_or("请先打开记忆库")?.read(&id)?;
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        pipeline.annotate(&mut memory.summary)?;
    }
    Ok(memory)
}

#[tauri::command]
async fn memory_save(
    input: MemoryInput,
    state: State<'_, AppState>,
) -> Result<MemoryRecord, String> {
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let vault = vault.as_mut().ok_or("请先打开记忆库")?;
    let mut memory = vault.save(input)?;
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        pipeline.sync_memory_embedding(
            &memory.summary.id,
            &archive::embedding_key(&config.embedding),
            archive::embedding_ready(&config.embedding),
        )?;
        pipeline.annotate(&mut memory.summary)?;
    }
    Ok(memory)
}

#[tauri::command]
async fn memory_adopt_suggestion(
    id: String,
    source_id: String,
    revision: String,
    state: State<'_, AppState>,
) -> Result<MemoryRecord, String> {
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mut memory = vault
        .as_mut()
        .ok_or("请先打开记忆库")?
        .adopt_suggestion(&id, &source_id, &revision)?;
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        pipeline.sync_memory_embedding(
            &id,
            &archive::embedding_key(&config.embedding),
            archive::embedding_ready(&config.embedding),
        )?;
        pipeline.annotate(&mut memory.summary)?;
    }
    Ok(memory)
}

#[tauri::command]
async fn settings_get(state: State<'_, AppState>) -> Result<SettingsView, String> {
    settings_view(&state)
}

fn settings_view(state: &AppState) -> Result<SettingsView, String> {
    let mut view = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .view()?;
    if let Some(pipeline) = state
        .pipeline
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
    {
        view.archive_started_at = archive::started_at(pipeline, &view)?;
    }
    Ok(view)
}

#[tauri::command]
async fn theme_get(state: State<'_, AppState>) -> Result<theme::ThemeConfig, String> {
    theme::load(&state.data_directory)
}

#[tauri::command]
async fn theme_save(
    theme: theme::ThemeConfig,
    state: State<'_, AppState>,
) -> Result<theme::ThemeConfig, String> {
    theme::save(&state.data_directory, theme)
}

#[tauri::command]
async fn settings_save(
    input: UpdateSettings,
    state: State<'_, AppState>,
) -> Result<SettingsView, String> {
    apply_settings_change(&state, SettingsChange::Form(input))
}

enum SettingsChange {
    Form(UpdateSettings),
    Import(transfer::PortableConfig),
}

fn apply_settings_change(state: &AppState, change: SettingsChange) -> Result<SettingsView, String> {
    let previous = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    let mut requested = previous.clone();
    match &change {
        SettingsChange::Form(input) => {
            requested.archive_enabled = input.archive_enabled;
            requested.archive_sources = input.archive_sources.clone();
            requested.codex_directory = input.codex_directory.trim().into();
            requested.harness_directory = input.harness_directory.trim().into();
        }
        SettingsChange::Import(input) => requested.archive_enabled = input.archive_enabled,
    }
    let baselines = archive::collect_activations(&previous, &requested)?;
    let activation = if baselines.is_empty() {
        None
    } else {
        Some((state.workspace_root()?.ok_or("请先打开记忆库")?, baselines))
    };
    let config = {
        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        match change {
            SettingsChange::Form(input) => {
                settings.update(input)?;
            }
            SettingsChange::Import(input) => {
                let mut next = previous.clone();
                next.chat.base_url = input.chat.base_url;
                next.chat.model = input.chat.model;
                next.chat.provider = input.chat.provider;
                next.embedding.base_url = input.embedding.base_url;
                next.embedding.model = input.embedding.model;
                next.embedding.provider = input.embedding.provider;
                next.archive_enabled = input.archive_enabled;
                next.run_in_background = input.keep_in_tray;
                settings.apply_import(next)?;
            }
        }
        if let Some((root, baselines)) = activation {
            let enabled_at =
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            state.with_workspace(&root, |_, pipeline| {
                for baseline in &baselines {
                    baseline.apply(pipeline, &enabled_at)?;
                }
                Ok(())
            })?;
            settings.set_archive_started_at(Some(enabled_at))?;
        }
        settings.view()?
    };
    if let Some(root) = state.workspace_root()? {
        state.with_workspace(&root, |vault, pipeline| {
            pipeline.sync_embeddings(
                vault,
                &archive::embedding_key(&config.embedding),
                archive::embedding_ready(&config.embedding),
            )
        })?;
    }
    settings_view(state)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferApplyResult {
    kind: transfer::TransferKind,
    added_memories: usize,
    existing_memories: usize,
    new_links: usize,
    invalid_links: usize,
    settings: Option<SettingsView>,
    theme: Option<theme::ThemeConfig>,
    workspace: Option<Workspace>,
}

#[tauri::command]
async fn transfer_export(
    kind: transfer::TransferKind,
    path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let path = Path::new(&path);
    match kind {
        transfer::TransferKind::Memories => {
            let vault = state.vault.lock().map_err(|error| error.to_string())?;
            transfer::export_memories(path, vault.as_ref().ok_or("请先打开记忆库")?)
        }
        transfer::TransferKind::Config => {
            let settings = state.settings.lock().map_err(|error| error.to_string())?;
            let theme = theme::load(&state.data_directory)?;
            transfer::export_config(path, settings.config(), &theme)
        }
        transfer::TransferKind::Theme => {
            transfer::export_theme(path, &theme::load(&state.data_directory)?)
        }
    }
}

#[tauri::command]
async fn theme_export(path: String, theme: theme::ThemeConfig) -> Result<String, String> {
    transfer::export_theme(Path::new(&path), &theme.validate()?)
}

#[tauri::command]
async fn theme_import(path: String) -> Result<theme::ThemeConfig, String> {
    transfer::read_theme(Path::new(&path))
}

#[tauri::command]
async fn transfer_preview(
    kind: transfer::TransferKind,
    path: String,
    state: State<'_, AppState>,
) -> Result<transfer::ImportPreview, String> {
    let config = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .config()
        .clone();
    let theme = theme::load(&state.data_directory)?;
    let vault = state.vault.lock().map_err(|error| error.to_string())?;
    state
        .transfers
        .lock()
        .map_err(|error| error.to_string())?
        .preview(
            kind,
            Path::new(&path),
            vault.as_ref().ok_or("请先打开记忆库")?,
            &config,
            &theme,
        )
}

#[tauri::command]
async fn transfer_discard(token: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .transfers
        .lock()
        .map_err(|error| error.to_string())?
        .discard(&token);
    Ok(())
}

#[tauri::command]
async fn transfer_apply(
    token: String,
    state: State<'_, AppState>,
) -> Result<TransferApplyResult, String> {
    apply_transfer(&state, &token)
}

fn apply_transfer(state: &AppState, token: &str) -> Result<TransferApplyResult, String> {
    let root = state.workspace_root()?.ok_or("请先打开记忆库")?;
    let prepared = state
        .transfers
        .lock()
        .map_err(|error| error.to_string())?
        .take(token, &root)?;
    let mut result = TransferApplyResult {
        kind: transfer::TransferKind::Memories,
        added_memories: 0,
        existing_memories: 0,
        new_links: 0,
        invalid_links: 0,
        settings: None,
        theme: None,
        workspace: None,
    };
    match prepared {
        transfer::PreparedImport::Memories(plan) => {
            let config = state
                .settings
                .lock()
                .map_err(|error| error.to_string())?
                .config()
                .clone();
            let (counts, workspace) = state.with_workspace(&root, |vault, pipeline| {
                let counts = plan.apply(vault)?;
                pipeline.sync_embeddings(
                    vault,
                    &archive::embedding_key(&config.embedding),
                    archive::embedding_ready(&config.embedding),
                )?;
                Ok((counts, vault.workspace()?))
            })?;
            result.added_memories = counts.added_memories;
            result.existing_memories = counts.existing_memories;
            result.new_links = counts.new_links;
            result.invalid_links = counts.invalid_links;
            result.workspace = Some(workspace);
        }
        transfer::PreparedImport::Config(config) => {
            result.kind = transfer::TransferKind::Config;
            let appearance = config.theme.clone();
            result.settings = Some(apply_settings_change(
                &state,
                SettingsChange::Import(config),
            )?);
            if let Some(theme) = appearance {
                theme::save_validated(&state.data_directory, &theme)?;
                result.theme = Some(theme);
            }
        }
        transfer::PreparedImport::Theme(theme) => {
            result.kind = transfer::TransferKind::Theme;
            theme::save_validated(&state.data_directory, &theme)?;
            result.theme = Some(theme);
        }
    }
    Ok(result)
}

#[tauri::command]
async fn api_test(
    service: ServiceKind,
    input: ServiceInput,
    state: State<'_, AppState>,
) -> Result<model_api::ConnectionTest, String> {
    let credentials = {
        state
            .settings
            .lock()
            .map_err(|e| e.to_string())?
            .test_credentials(service, input)?
    };
    model_api::test_connection(&credentials, service).await
}

#[tauri::command]
async fn api_models(
    service: ServiceKind,
    base_url: String,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let credentials = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .models_credentials(service, &base_url, api_key)?;
    model_api::list_models(&credentials).await
}

#[tauri::command]
async fn archive_status(state: State<'_, AppState>) -> Result<ArchiveStatus, String> {
    let config = state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .clone();
    let error = state.last_error.lock().map_err(|e| e.to_string())?.clone();
    let pipeline = state.pipeline.lock().map_err(|e| e.to_string())?;
    let Some(pipeline) = pipeline.as_ref() else {
        return Ok(ArchiveStatus {
            enabled: config.archive_enabled,
            started_at: config.archive_started_at,
            ..ArchiveStatus::default()
        });
    };
    let started_at = archive::started_at(pipeline, &config)?;
    pipeline.status(
        config.archive_enabled,
        started_at,
        state.scanning.load(Ordering::Relaxed),
        error.or(pipeline.meta("scan_warning")?),
    )
}

#[tauri::command]
async fn archive_retry(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(pipeline) = state.pipeline.lock().map_err(|e| e.to_string())?.as_ref() {
        pipeline.retry_failed()?;
    }
    *state.last_error.lock().map_err(|e| e.to_string())? = None;
    Ok(())
}

#[tauri::command]
async fn memory_open_file(
    id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let memory = vault.as_ref().ok_or("请先打开记忆库")?.read(&id)?;
    app.opener()
        .open_path(memory.summary.path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedWorkspace {
    root: PathBuf,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name(if cfg!(debug_assertions) {
                    "KnowTrace Dev"
                } else {
                    "KnowTrace"
                })
                .build(),
        )
        .setup(|app| {
            let data_dir = config::application_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let settings = SettingsStore::load(&data_dir, codex::default_codex_directory())?;
            app.manage(mcp_http::HttpRuntime::default());
            app.manage(usage::Runtime::new(&data_dir)?);
            app.manage(AppState {
                vault: Mutex::new(None),
                settings_path: data_dir.join("workspace.json"),
                default_root: data_dir.join("memories"),
                settings: Mutex::new(settings),
                pipeline: Mutex::new(None),
                recovered_roots: Mutex::new(BTreeSet::new()),
                scanning: AtomicBool::new(false),
                last_error: Mutex::new(None),
                search: Mutex::new(None),
                data_directory: data_dir.clone(),
                transfers: Mutex::new(transfer::TransferState::default()),
                workspace_transition: tokio::sync::RwLock::new(()),
            });
            let open = tauri::menu::MenuItem::with_id(app, "open", "打开知迹", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "完全退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&open, &quit])?;
            tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("知迹 · KnowTrace · 本地记忆归档")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            archive::start(app.handle());
            usage::start(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            usage::usage_scan,
            usage::usage_overview,
            usage::usage_sessions,
            usage::usage_detail,
            usage::usage_preferences_save,
            usage::usage_prices_get,
            usage::usage_prices_save,
            usage::usage_issues,
            usage::usage_open_source,
            usage::usage_export,
            update::app_update_check,
            update::app_update_download,
            workspace_open,
            workspace_migrate,
            workspace_refresh,
            repository_add,
            repository_rename,
            repository_delete,
            memory_search,
            memory_graph,
            semantic_search,
            mcp_info,
            mcp_http_status,
            mcp_http_start,
            mcp_http_stop,
            memory_read,
            memory_save,
            memory_adopt_suggestion,
            memory_open_file,
            settings_get,
            settings_save,
            theme_get,
            theme_save,
            theme_export,
            theme_import,
            transfer_export,
            transfer_preview,
            transfer_apply,
            transfer_discard,
            api_test,
            api_models,
            archive_status,
            archive_retry
        ])
        .run(tauri::generate_context!())
        .expect("无法启动知迹 KnowTrace");
}
