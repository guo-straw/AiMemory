use crate::{
    config::{validate_service, AppConfig, ServiceInput, ServiceProvider},
    storage::{transfer::PreparedMemoryImport, write_atomic, Vault},
    theme::ThemeConfig,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferKind {
    Memories,
    Config,
    Theme,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableRepository {
    pub id: String,
    pub name: String,
    pub remote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableMemory {
    pub id: String,
    pub repository: String,
    pub markdown: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableLink {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPackage {
    #[serde(rename = "type")]
    pub package_type: String,
    pub version: u32,
    pub memories: Vec<PortableMemory>,
    pub links: Vec<PortableLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<PortableRepository>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableService {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ServiceProvider>,
    pub base_url: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableConfig {
    pub chat: PortableService,
    pub embedding: PortableService,
    pub archive_enabled: bool,
    #[serde(alias = "runInBackground")]
    pub keep_in_tray: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeConfig>,
}

impl PortableConfig {
    pub fn from_local(config: &AppConfig, theme: &ThemeConfig) -> Self {
        Self {
            chat: PortableService {
                provider: config.chat.provider,
                base_url: config.chat.base_url.clone(),
                model: config.chat.model.clone(),
            },
            embedding: PortableService {
                provider: config.embedding.provider,
                base_url: config.embedding.base_url.clone(),
                model: config.embedding.model.clone(),
            },
            archive_enabled: config.archive_enabled,
            keep_in_tray: config.run_in_background,
            theme: Some(theme.clone()),
        }
    }

    fn validate(mut self) -> Result<Self, String> {
        for service in [&mut self.chat, &mut self.embedding] {
            let normalized = validate_service(
                &ServiceInput {
                    provider: service.provider,
                    base_url: service.base_url.clone(),
                    model: service.model.clone(),
                    api_key: None,
                },
                false,
            )?;
            service.base_url = normalized.base_url;
            service.model = normalized.model;
        }
        self.theme = self.theme.map(ThemeConfig::validate).transpose()?;
        Ok(self)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigPackage {
    #[serde(rename = "type")]
    package_type: String,
    version: u32,
    config: PortableConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemePackage {
    #[serde(rename = "type")]
    package_type: String,
    version: u32,
    theme: ThemeConfig,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPreviewRow {
    pub id: String,
    pub title: String,
    pub repository: String,
    pub status: String,
    pub markdown: String,
    pub local_markdown: Option<String>,
    pub local_repository: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LinkPreviewRow {
    pub from: String,
    pub to: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigPreviewRow {
    pub label: String,
    pub before: String,
    pub after: String,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub token: String,
    pub kind: TransferKind,
    pub path: String,
    pub memories: Vec<MemoryPreviewRow>,
    pub links: Vec<LinkPreviewRow>,
    pub rows: Vec<ConfigPreviewRow>,
    pub added_memories: usize,
    pub existing_memories: usize,
    pub new_links: usize,
    pub invalid_links: usize,
    pub changed_count: usize,
    pub has_theme: bool,
    pub theme: Option<ThemeConfig>,
}

pub enum PreparedImport {
    Memories(PreparedMemoryImport),
    Config(PortableConfig),
    Theme(ThemeConfig),
}

#[derive(Default)]
pub struct TransferState {
    pending: BTreeMap<String, (PathBuf, PreparedImport)>,
}

impl TransferState {
    pub fn preview(
        &mut self,
        kind: TransferKind,
        path: &Path,
        vault: &Vault,
        config: &AppConfig,
        theme: &ThemeConfig,
    ) -> Result<ImportPreview, String> {
        let raw = fs::read_to_string(path).map_err(|error| format!("无法读取导入文件：{error}"))?;
        let mut preview = ImportPreview {
            token: uuid::Uuid::new_v4().to_string(),
            kind,
            path: path.display().to_string(),
            memories: Vec::new(),
            links: Vec::new(),
            rows: Vec::new(),
            added_memories: 0,
            existing_memories: 0,
            new_links: 0,
            invalid_links: 0,
            changed_count: 0,
            has_theme: false,
            theme: None,
        };
        let prepared = match kind {
            TransferKind::Memories => {
                let packet: MemoryPackage = parse_package(&raw)?;
                check_header(&packet.package_type, packet.version, "aimemory.memories")?;
                let plan = vault.prepare_memory_import(packet, &mut preview)?;
                PreparedImport::Memories(plan)
            }
            TransferKind::Config => {
                let packet: ConfigPackage = parse_package(&raw)?;
                check_header(&packet.package_type, packet.version, "aimemory.config")?;
                let incoming = packet.config.validate()?;
                preview.rows = config_rows(config, theme, &incoming);
                preview.changed_count = preview.rows.iter().filter(|row| row.changed).count();
                preview.has_theme = incoming.theme.is_some();
                preview.theme = incoming.theme.clone();
                PreparedImport::Config(incoming)
            }
            TransferKind::Theme => {
                let incoming = parse_theme(&raw)?;
                preview.rows = theme_rows(theme, &incoming);
                preview.changed_count = preview.rows.iter().filter(|row| row.changed).count();
                preview.has_theme = true;
                preview.theme = Some(incoming.clone());
                PreparedImport::Theme(incoming)
            }
        };
        self.pending
            .insert(preview.token.clone(), (vault.root.clone(), prepared));
        Ok(preview)
    }

    pub fn take(&mut self, token: &str, current_root: &Path) -> Result<PreparedImport, String> {
        let (root, _) = self
            .pending
            .get(token)
            .ok_or("导入预览已失效，请重新选择文件")?;
        if root != current_root {
            return Err("记忆库已切换，请重新预览后导入".into());
        }
        Ok(self.pending.remove(token).unwrap().1)
    }

    pub fn discard(&mut self, token: &str) {
        self.pending.remove(token);
    }
}

pub fn export_memories(path: &Path, vault: &Vault) -> Result<String, String> {
    write_package(path, &vault.memory_package()?)
}

pub fn export_config(
    path: &Path,
    config: &AppConfig,
    theme: &ThemeConfig,
) -> Result<String, String> {
    write_package(
        path,
        &ConfigPackage {
            package_type: "aimemory.config".into(),
            version: 1,
            config: PortableConfig::from_local(config, theme),
        },
    )
}

pub fn export_theme(path: &Path, theme: &ThemeConfig) -> Result<String, String> {
    write_package(
        path,
        &ThemePackage {
            package_type: "aimemory.theme".into(),
            version: 1,
            theme: theme.clone(),
        },
    )
}

pub fn read_theme(path: &Path) -> Result<ThemeConfig, String> {
    parse_theme(&fs::read_to_string(path).map_err(|error| format!("无法读取主题文件：{error}"))?)
}

fn parse_theme(raw: &str) -> Result<ThemeConfig, String> {
    let packet: ThemePackage = parse_package(raw)?;
    check_header(&packet.package_type, packet.version, "aimemory.theme")?;
    packet.theme.validate()
}

fn parse_package<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("导入文件格式无效：{error}"))
}

fn check_header(kind: &str, version: u32, expected: &str) -> Result<(), String> {
    if kind != expected {
        return Err(format!("文件类型不符，请选择 {expected} 文件"));
    }
    if version != 1 {
        return Err("请使用版本 1 的知迹文件（兼容 AiMemory 文件）".into());
    }
    Ok(())
}

fn write_package(path: &Path, packet: &impl Serialize) -> Result<String, String> {
    let mut bytes = serde_json::to_vec_pretty(packet).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)?;
    Ok(path.display().to_string())
}

fn config_rows(
    local: &AppConfig,
    theme: &ThemeConfig,
    incoming: &PortableConfig,
) -> Vec<ConfigPreviewRow> {
    let mut rows = [
        (
            "聊天服务地址",
            local.chat.base_url.clone(),
            incoming.chat.base_url.clone(),
        ),
        (
            "聊天模型",
            local.chat.model.clone(),
            incoming.chat.model.clone(),
        ),
        (
            "向量服务地址",
            local.embedding.base_url.clone(),
            incoming.embedding.base_url.clone(),
        ),
        (
            "向量模型",
            local.embedding.model.clone(),
            incoming.embedding.model.clone(),
        ),
        (
            "自动归档",
            enabled(local.archive_enabled),
            enabled(incoming.archive_enabled),
        ),
        (
            "关闭窗口驻留托盘",
            enabled(local.run_in_background),
            enabled(incoming.keep_in_tray),
        ),
    ]
    .into_iter()
    .map(|(label, before, after)| diff_row(label, before, after))
    .collect::<Vec<_>>();
    if let Some(incoming_theme) = &incoming.theme {
        rows.extend(theme_rows(theme, incoming_theme));
    } else {
        rows.push(diff_row(
            "主题预设",
            theme.preset.clone(),
            theme.preset.clone(),
        ));
    }
    rows
}

fn theme_rows(local: &ThemeConfig, incoming: &ThemeConfig) -> Vec<ConfigPreviewRow> {
    let mut rows: Vec<_> = [
        ("主题预设", &local.preset, &incoming.preset),
        ("强调色", &local.accent, &incoming.accent),
        ("背景色", &local.background, &incoming.background),
        ("前景色", &local.foreground, &incoming.foreground),
    ]
    .into_iter()
    .map(|(label, before, after)| diff_row(label, before.clone(), after.clone()))
    .collect();
    let font_label = |font: &str| {
        match font {
            "serif" => "衬线",
            "mono" => "等宽",
            _ => "系统",
        }
        .to_string()
    };
    rows.push(diff_row(
        "界面字体",
        font_label(&local.font),
        font_label(&incoming.font),
    ));
    rows
}

fn diff_row(label: &str, before: String, after: String) -> ConfigPreviewRow {
    ConfigPreviewRow {
        label: label.into(),
        changed: before != after,
        before,
        after,
    }
}

fn enabled(value: bool) -> String {
    if value { "开启" } else { "关闭" }.into()
}

#[cfg(test)]
mod tests;
