use crate::storage::write_atomic;
mod secrets;
use secrets::EncryptedKeys;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn application_data_dir() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("dev-data"));
    }
    dirs::data_dir()
        .map(|directory| directory.join("app.aimemory.desktop"))
        .ok_or_else(|| "无法定位应用数据目录".into())
}

pub fn service_fingerprint(base_url: &str, model: &str) -> String {
    format!("{base_url}\n{model}")
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Chat,
    Embedding,
}

impl ServiceKind {
    fn account(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embedding => "embedding",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ServiceProvider>,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub has_api_key: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub chat: ServiceConfig,
    pub embedding: ServiceConfig,
    pub codex_directory: String,
    #[serde(default = "default_harness_directory_value")]
    pub harness_directory: String,
    #[serde(default = "default_archive_sources")]
    pub archive_sources: Vec<String>,
    #[serde(default)]
    pub archive_enabled: bool,
    #[serde(default)]
    pub archive_started_at: Option<String>,
    #[serde(default)]
    pub run_in_background: bool,
}

pub type SettingsView = AppConfig;

fn default_harness_directory_value() -> String {
    crate::harness::default_harness_directory()
        .display()
        .to_string()
}

pub fn default_archive_sources() -> Vec<String> {
    vec!["codex".into()]
}

impl AppConfig {
    pub fn archives(&self, client: &str) -> bool {
        self.archive_enabled && self.archive_sources.iter().any(|source| source == client)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceProvider {
    Openai,
    Deepseek,
    Custom,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInput {
    pub provider: Option<ServiceProvider>,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    pub chat: ServiceInput,
    pub embedding: ServiceInput,
    pub codex_directory: String,
    #[serde(default = "default_harness_directory_value")]
    pub harness_directory: String,
    #[serde(default = "default_archive_sources")]
    pub archive_sources: Vec<String>,
    pub archive_enabled: bool,
    pub run_in_background: bool,
}

#[derive(Clone)]
pub struct ServiceCredentials {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

pub struct SettingsStore {
    path: PathBuf,
    legacy_keyring_service: String,
    keys: EncryptedKeys,
    config: AppConfig,
}

impl SettingsStore {
    pub fn load(data_dir: &Path, default_codex_dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let path = data_dir.join("services.json");
        let config = match fs::read_to_string(&path) {
            Ok(json) => {
                serde_json::from_str(&json).map_err(|e| format!("API 配置格式错误：{e}"))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AppConfig {
                chat: ServiceConfig {
                    provider: Some(ServiceProvider::Openai),
                    base_url: "https://api.openai.com/v1".into(),
                    model: "gpt-6.2-astra".into(),
                    has_api_key: false,
                },
                embedding: ServiceConfig {
                    provider: Some(ServiceProvider::Openai),
                    base_url: "https://api.openai.com/v1".into(),
                    model: "text-embedding-3-large".into(),
                    has_api_key: false,
                },
                codex_directory: default_codex_dir.display().to_string(),
                harness_directory: default_harness_directory_value(),
                archive_sources: default_archive_sources(),
                archive_enabled: false,
                archive_started_at: None,
                run_in_background: false,
            },
            Err(error) => return Err(format!("无法读取 API 配置：{error}")),
        };
        let scope = blake3::hash(
            data_dir
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .as_bytes(),
        );
        Ok(Self {
            path,
            keys: EncryptedKeys::new(data_dir),
            legacy_keyring_service: format!("app.aimemory.desktop.{}", &scope.to_hex()[..16]),
            config,
        })
    }

    pub fn view(&self) -> Result<SettingsView, String> {
        Ok(self.config.clone())
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn update(&mut self, mut input: UpdateSettings) -> Result<SettingsView, String> {
        let chat = validate_service(&input.chat, self.config.chat.has_api_key)?;
        let embedding = validate_service(&input.embedding, self.config.embedding.has_api_key)?;
        for (supplied, next, current) in [
            (&mut input.chat, &chat, &self.config.chat),
            (&mut input.embedding, &embedding, &self.config.embedding),
        ] {
            if next.base_url != current.base_url
                && supplied.api_key.is_none()
                && current.has_api_key
            {
                supplied.api_key = Some(String::new());
            }
        }
        let codex_directory = input.codex_directory.trim();
        let harness_directory = input.harness_directory.trim();
        let archive_sources = validate_archive_settings(
            codex_directory,
            harness_directory,
            &input.archive_sources,
            input.archive_enabled,
        )?;
        let mut next = AppConfig {
            chat,
            embedding,
            codex_directory: codex_directory.into(),
            harness_directory: crate::harness::root_directory(Path::new(harness_directory))
                .display()
                .to_string(),
            archive_sources,
            archive_enabled: input.archive_enabled,
            archive_started_at: self.config.archive_started_at.clone(),
            run_in_background: input.run_in_background,
        };
        let mut changed = Vec::new();
        let result = (|| {
            for (kind, supplied, endpoint) in [
                (ServiceKind::Chat, input.chat.api_key, &mut next.chat),
                (
                    ServiceKind::Embedding,
                    input.embedding.api_key,
                    &mut next.embedding,
                ),
            ] {
                if let Some(key) = supplied {
                    let previous = self.keys.stored(kind.account())?;
                    changed.push((kind, previous));
                    self.write_key(kind, Some(key.trim()).filter(|key| !key.is_empty()))?;
                    endpoint.has_api_key = !key.trim().is_empty();
                }
            }
            write_atomic(
                &self.path,
                &serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?,
            )
        })();
        if let Err(error) = result {
            let mut restore_errors = Vec::new();
            for (kind, previous) in changed.into_iter().rev() {
                if let Err(restore) = self.keys.restore(kind.account(), previous.as_ref()) {
                    restore_errors.push(restore);
                }
            }
            return if restore_errors.is_empty() {
                Err(error)
            } else {
                Err(format!("{error}；恢复原密钥失败，请重新保存密钥"))
            };
        }
        self.config = next;
        self.view()
    }

    pub fn set_archive_started_at(&mut self, started_at: Option<String>) -> Result<(), String> {
        let mut next = self.config.clone();
        next.archive_started_at = started_at;
        write_atomic(
            &self.path,
            &serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?,
        )?;
        self.config = next;
        Ok(())
    }

    pub(crate) fn apply_import(&mut self, mut next: AppConfig) -> Result<SettingsView, String> {
        next.archive_sources = validate_archive_settings(
            &next.codex_directory,
            &next.harness_directory,
            &next.archive_sources,
            next.archive_enabled,
        )?;
        write_atomic(
            &self.path,
            &serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?,
        )?;
        self.config = next;
        self.view()
    }

    pub fn credentials(&self, kind: ServiceKind) -> Result<ServiceCredentials, String> {
        let service = match kind {
            ServiceKind::Chat => &self.config.chat,
            ServiceKind::Embedding => &self.config.embedding,
        };
        let api_key = if service.has_api_key {
            self.read_key(kind)?
                .ok_or("内部存储中没有 API 密钥，请重新填写")?
        } else {
            String::new()
        };
        Ok(ServiceCredentials {
            base_url: service.base_url.clone(),
            model: service.model.clone(),
            api_key,
        })
    }

    pub fn test_credentials(
        &self,
        kind: ServiceKind,
        input: ServiceInput,
    ) -> Result<ServiceCredentials, String> {
        let service = validate_service(&input, false)?;
        let mut credentials = self.connection_credentials(kind, service.base_url, input.api_key)?;
        credentials.model = service.model;
        Ok(credentials)
    }

    pub fn models_credentials(
        &self,
        kind: ServiceKind,
        base_url: &str,
        api_key: Option<String>,
    ) -> Result<ServiceCredentials, String> {
        self.connection_credentials(kind, validate_base_url(base_url)?, api_key)
    }

    fn connection_credentials(
        &self,
        kind: ServiceKind,
        base_url: String,
        api_key: Option<String>,
    ) -> Result<ServiceCredentials, String> {
        let current = match kind {
            ServiceKind::Chat => &self.config.chat,
            ServiceKind::Embedding => &self.config.embedding,
        };
        let api_key = match api_key {
            Some(key) => key.trim().to_string(),
            None if base_url == current.base_url => self.credentials(kind)?.api_key,
            None => String::new(),
        };
        Ok(ServiceCredentials {
            base_url,
            model: String::new(),
            api_key,
        })
    }

    fn read_key(&self, kind: ServiceKind) -> Result<Option<String>, String> {
        if let Some(row) = self.keys.stored(kind.account())? {
            return self.keys.decrypt(kind.account(), row);
        }
        let entry = keyring::Entry::new(&self.legacy_keyring_service, kind.account())
            .map_err(|e| format!("无法读取旧系统凭据，请重新填写 API 密钥：{e}"))?;
        let value = match entry.get_password() {
            Ok(key) => Some(key),
            Err(keyring::Error::NoEntry) => None,
            Err(error) => return Err(format!("无法迁移旧 API 密钥，请重新填写：{error}")),
        };
        self.keys.write(kind.account(), value.as_deref())?;
        Ok(value)
    }

    fn write_key(&self, kind: ServiceKind, key: Option<&str>) -> Result<(), String> {
        self.keys.write(kind.account(), key)
    }
}

fn validate_archive_settings(
    codex_directory: &str,
    harness_directory: &str,
    selected: &[String],
    enabled: bool,
) -> Result<Vec<String>, String> {
    if codex_directory.is_empty() {
        return Err("请选择 Codex 会话目录".into());
    }
    if harness_directory.is_empty() {
        return Err("请选择 DeepSeek Harness 数据目录".into());
    }
    if selected
        .iter()
        .any(|source| !matches!(source.as_str(), "codex" | "harness"))
    {
        return Err("自动归档来源只能选择 Codex 或 DeepSeek Harness".into());
    }
    let sources: Vec<String> = ["codex", "harness"]
        .into_iter()
        .filter(|source| selected.iter().any(|selected| selected == source))
        .map(str::to_string)
        .collect();
    if enabled {
        if sources.is_empty() {
            return Err("请至少选择一个自动归档来源".into());
        }
        if sources.iter().any(|source| source == "codex") && !Path::new(codex_directory).is_dir() {
            return Err("Codex 会话目录不存在，请选择 sessions 目录".into());
        }
        if sources.iter().any(|source| source == "harness")
            && !Path::new(harness_directory).is_dir()
        {
            return Err("DeepSeek Harness 数据目录不存在，请选择 .dsh 或 sessions 目录".into());
        }
    }
    Ok(sources)
}

pub(crate) fn validate_service(
    input: &ServiceInput,
    has_api_key: bool,
) -> Result<ServiceConfig, String> {
    let base_url = validate_base_url(&input.base_url)?;
    let model = input.model.trim();
    if model.is_empty() {
        return Err("请填写模型名称".into());
    }
    Ok(ServiceConfig {
        provider: input.provider,
        base_url,
        model: model.into(),
        has_api_key,
    })
}

fn validate_base_url(base_url: &str) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/');
    let url = url::Url::parse(base).map_err(|_| "API 服务地址格式不正确")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("API 服务地址应为 HTTP 或 HTTPS 基础地址".into());
    }
    Ok(base.into())
}

#[cfg(test)]
mod tests;
