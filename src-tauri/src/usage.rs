mod parser;
mod store;
#[cfg(test)]
mod tests;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
pub use store::Store;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: i64,
    pub cached: Option<i64>,
    pub cache_write: i64,
    pub output: i64,
    pub reasoning: Option<i64>,
}

impl Tokens {
    fn parse(value: &Value) -> Result<Self, String> {
        let counter = |key: &str| -> Result<Option<i64>, String> {
            match value.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_i64()
                    .filter(|n| *n >= 0)
                    .map(Some)
                    .ok_or_else(|| format!("{key} 必须为非负整数")),
            }
        };
        let tokens = Self {
            input: counter("input_tokens")?.ok_or("缺少 input_tokens")?,
            cached: counter("cached_input_tokens")?.or(counter("cache_read_input_tokens")?),
            cache_write: counter("cache_write_input_tokens")?.unwrap_or(0),
            output: counter("output_tokens")?.ok_or("缺少 output_tokens")?,
            reasoning: counter("reasoning_output_tokens")?,
        };
        if tokens
            .cached
            .is_some_and(|c| c + tokens.cache_write > tokens.input)
            || tokens.reasoning.is_some_and(|r| r > tokens.output)
        {
            return Err("Token 分项超出输入或输出总量".into());
        }
        Ok(tokens)
    }

    fn delta(&self, previous: &Self) -> Result<Self, String> {
        if self.input < previous.input || self.output < previous.output {
            return Err("累计值回退且没有单次用量，待核对来源".into());
        }
        let delta = Self {
            input: self.input - previous.input,
            output: self.output - previous.output,
            cached: self.cached.zip(previous.cached).map(|(a, b)| a - b),
            cache_write: self.cache_write - previous.cache_write,
            reasoning: self
                .reasoning
                .zip(previous.reasoning)
                .filter(|(a, b)| a >= b)
                .map(|(a, b)| a - b),
        };
        if delta.cache_write < 0
            || delta
                .cached
                .is_some_and(|c| c < 0 || c + delta.cache_write > delta.input)
        {
            return Err("累计缓存分项变化不一致，待核对来源".into());
        }
        Ok(delta)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub at: i64,
    pub model: String,
    pub provider: String,
    pub tier: String,
    pub project: String,
    pub role: String,
    pub tokens: Tokens,
    pub cost: Option<f64>,
    pub file: String,
    pub line: i64,
    pub basis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub at: i64,
    pub thread_id: String,
    pub data: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    #[serde(skip)]
    pub source: String,
    pub start: i64,
    pub end: i64,
    pub project: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub buckets: Vec<Bucket>,
}

impl Filters {
    fn validate(&self) -> Result<(), String> {
        if self.start >= self.end {
            return Err("结束时间必须晚于开始时间".into());
        }
        if self.buckets.iter().any(|b| b.start >= b.end) {
            return Err("时间分组范围无效".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub records: i64,
    pub sessions: i64,
    pub input: i64,
    pub cached: i64,
    pub cache_write: i64,
    pub output: i64,
    pub reasoning: i64,
    pub cache_known_input: i64,
    pub reasoning_known_output: i64,
    pub priced_tokens: i64,
    pub priced_records: i64,
    pub cost: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub key: String,
    pub summary: Summary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trend {
    pub label: String,
    pub start: i64,
    pub summary: Summary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub model: String,
    pub project: String,
    pub first_at: i64,
    pub last_at: i64,
    pub summary: Summary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub file: String,
    pub line: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub scanning: bool,
    pub completed: usize,
    pub total: usize,
    pub last_synced_at: Option<i64>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub auto_scan: bool,
    pub interval_seconds: u64,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_scan: true,
            interval_seconds: 60,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub model: String,
    pub provider: String,
    pub tier: String,
    pub input: Option<f64>,
    pub cached: Option<f64>,
    pub cache_write: Option<f64>,
    pub output: Option<f64>,
    pub source: String,
    #[serde(default)]
    pub custom: bool,
}

impl Price {
    fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("模型不能为空".into());
        }
        if [self.input, self.cached, self.cache_write, self.output]
            .into_iter()
            .flatten()
            .any(|n| !n.is_finite() || n < 0.0)
        {
            return Err("参考价格必须为非负数".into());
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub summary: Summary,
    pub models: Vec<Group>,
    pub projects: Vec<Group>,
    pub roles: Vec<String>,
    pub available_models: Vec<String>,
    pub available_projects: Vec<String>,
    pub trend: Vec<Trend>,
    pub quota: Option<QuotaSnapshot>,
    pub issue_count: i64,
    pub status: ScanStatus,
    pub preferences: Preferences,
    pub directory: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    pub items: Vec<Session>,
    pub total: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub session: Session,
    pub records: Vec<UsageRecord>,
    pub total_records: i64,
    pub cost_parts: [f64; 4],
    pub cost_groups: Vec<CostGroup>,
    pub memories: Vec<crate::storage::MemorySummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostGroup {
    pub price: Price,
    pub input: i64,
    pub cached: i64,
    pub cache_write: i64,
    pub output: i64,
}

pub struct Runtime {
    path: PathBuf,
    scanning: AtomicBool,
    status: Mutex<ScanStatus>,
}

impl Runtime {
    pub fn new(directory: &Path) -> Result<Self, String> {
        let path = directory.join("codex-usage.sqlite");
        let store = Store::open(&path)?;
        let last_synced_at = store.last_synced()?;
        Ok(Self {
            path,
            scanning: AtomicBool::new(false),
            status: Mutex::new(ScanStatus {
                last_synced_at,
                ..Default::default()
            }),
        })
    }

    fn status(&self) -> Result<ScanStatus, String> {
        self.status
            .lock()
            .map(|s| s.clone())
            .map_err(|e| e.to_string())
    }
}

fn directory(state: &crate::AppState) -> Result<String, String> {
    Ok(state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .config()
        .codex_directory
        .clone())
}

fn begin_scan(app: &tauri::AppHandle, directory: String, full: bool) -> Result<(), String> {
    let runtime = app.state::<Runtime>();
    if runtime.scanning.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let last_synced_at = runtime.status()?.last_synced_at;
    *runtime.status.lock().map_err(|e| e.to_string())? = ScanStatus {
        scanning: true,
        last_synced_at,
        ..Default::default()
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = app.state::<Runtime>();
        let result = Store::open(&runtime.path).and_then(|mut store| {
            store.scan(Path::new(&directory), full, |done, total| {
                let mut status = runtime.status.lock().unwrap();
                status.completed = done;
                status.total = total;
            })
        });
        let mut status = runtime.status.lock().unwrap();
        match result {
            Ok(errors) => {
                status.errors = errors;
                status.last_synced_at = Some(Utc::now().timestamp_millis());
            }
            Err(e) => status.errors.push(e),
        }
        status.scanning = false;
        runtime.scanning.store(false, Ordering::SeqCst);
    });
    Ok(())
}

pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let result = (|| {
                let runtime = app.state::<Runtime>();
                if Store::open(&runtime.path)?.preferences()?.auto_scan {
                    begin_scan(&app, directory(&app.state::<crate::AppState>())?, false)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                app.state::<Runtime>().status.lock().unwrap().errors = vec![error];
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

#[tauri::command]
pub(crate) async fn usage_scan(
    full: Option<bool>,
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<ScanStatus, String> {
    begin_scan(&app, directory(&state)?, full.unwrap_or(false))?;
    runtime.status()
}

#[tauri::command]
pub(crate) async fn usage_overview(
    mut filters: Filters,
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<Overview, String> {
    prepare_filters(&mut filters, &state)?;
    Store::open(&runtime.path)?.overview(&filters, &directory(&state)?, runtime.status()?)
}

#[tauri::command]
pub(crate) async fn usage_sessions(
    mut filters: Filters,
    page: u32,
    sort: String,
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<SessionPage, String> {
    prepare_filters(&mut filters, &state)?;
    Store::open(&runtime.path)?.sessions(&filters, page, &sort)
}

#[tauri::command]
pub(crate) async fn usage_detail(
    mut filters: Filters,
    thread_id: String,
    page: u32,
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<Detail, String> {
    prepare_filters(&mut filters, &state)?;
    let mut detail = Store::open(&runtime.path)?.detail(&filters, &thread_id, page)?;
    if let Some(vault) = state.vault.lock().map_err(|e| e.to_string())?.as_ref() {
        detail.memories = vault.memories_for_session(&thread_id)?;
    }
    Ok(detail)
}

#[tauri::command]
pub(crate) async fn usage_preferences_save(
    preferences: Preferences,
    runtime: State<'_, Runtime>,
) -> Result<Preferences, String> {
    if ![5, 10, 30, 60].contains(&preferences.interval_seconds) {
        return Err("扫描间隔请选择 5、10、30 或 60 秒".into());
    }
    Store::open(&runtime.path)?.save_preferences(&preferences)?;
    Ok(preferences)
}

#[tauri::command]
pub(crate) async fn usage_prices_get(runtime: State<'_, Runtime>) -> Result<Vec<Price>, String> {
    Store::open(&runtime.path)?.prices()
}

#[tauri::command]
pub(crate) async fn usage_prices_save(
    prices: Vec<Price>,
    runtime: State<'_, Runtime>,
) -> Result<(), String> {
    for price in &prices {
        price.validate()?;
    }
    Store::open(&runtime.path)?.save_prices(&prices)
}

#[tauri::command]
pub(crate) async fn usage_issues(
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<Vec<Issue>, String> {
    Store::open(&runtime.path)?.issues(&store::source_id(Path::new(&directory(&state)?)))
}

#[tauri::command]
pub(crate) async fn usage_open_source(
    record_id: String,
    app: tauri::AppHandle,
    runtime: State<'_, Runtime>,
) -> Result<(), String> {
    let path = Store::open(&runtime.path)?.source_path(&record_id)?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn usage_export(
    mut filters: Filters,
    path: String,
    state: State<'_, crate::AppState>,
    runtime: State<'_, Runtime>,
) -> Result<(), String> {
    prepare_filters(&mut filters, &state)?;
    let records = Store::open(&runtime.path)?.records(&filters, None, None)?;
    write_csv(records, Path::new(&path))
}

fn write_csv(records: Vec<UsageRecord>, path: &Path) -> Result<(), String> {
    let cell = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut csv = String::from("\u{feff}时间,线程,项目,模型,供应商,档位,角色,输入,缓存读取,缓存写入,输出,其中推理,参考费用 USD,来源文件,行号\r\n");
    for r in records {
        let values = vec![
            chrono::DateTime::from_timestamp_millis(r.at)
                .unwrap()
                .to_rfc3339(),
            r.thread_id,
            r.project,
            r.model,
            r.provider,
            r.tier,
            r.role,
            r.tokens.input.to_string(),
            r.tokens.cached.map(|v| v.to_string()).unwrap_or_default(),
            r.tokens.cache_write.to_string(),
            r.tokens.output.to_string(),
            r.tokens
                .reasoning
                .map(|v| v.to_string())
                .unwrap_or_default(),
            r.cost.map(|v| format!("{v:.6}")).unwrap_or_default(),
            r.file,
            r.line.to_string(),
        ];
        csv.push_str(&values.iter().map(|v| cell(v)).collect::<Vec<_>>().join(","));
        csv.push_str("\r\n");
    }
    std::fs::write(path, csv).map_err(|e| e.to_string())
}

fn prepare_filters(filters: &mut Filters, state: &crate::AppState) -> Result<(), String> {
    filters.validate()?;
    filters.source = store::source_id(Path::new(&directory(state)?));
    Ok(())
}
