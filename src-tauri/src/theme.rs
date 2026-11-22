use crate::storage::write_atomic;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeConfig {
    pub preset: String,
    pub accent: String,
    pub background: String,
    pub foreground: String,
    pub font: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            preset: "ChatGPT".into(),
            accent: "#3566F0".into(),
            background: "#FFFFFF".into(),
            foreground: "#1A1C1F".into(),
            font: "system".into(),
        }
    }
}

impl ThemeConfig {
    pub fn validate(mut self) -> Result<Self, String> {
        self.preset = self.preset.trim().to_string();
        if self.preset.is_empty() {
            return Err("请填写主题预设名称".into());
        }
        for color in [&mut self.accent, &mut self.background, &mut self.foreground] {
            if color.len() != 7
                || !color.starts_with('#')
                || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err("主题颜色须为六位 HEX，例如 #3566F0".into());
            }
            color.make_ascii_uppercase();
        }
        if !matches!(self.font.as_str(), "system" | "serif" | "mono") {
            return Err("请选择系统、衬线或等宽字体".into());
        }
        Ok(self)
    }
}

pub fn load(data_dir: &Path) -> Result<ThemeConfig, String> {
    match fs::read_to_string(data_dir.join("appearance.json")) {
        Ok(json) => serde_json::from_str::<ThemeConfig>(&json)
            .map_err(|error| format!("主题配置格式错误：{error}"))?
            .validate(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ThemeConfig::default()),
        Err(error) => Err(format!("无法读取主题配置：{error}")),
    }
}

pub fn save(data_dir: &Path, theme: ThemeConfig) -> Result<ThemeConfig, String> {
    let theme = theme.validate()?;
    save_validated(data_dir, &theme)?;
    Ok(theme)
}

pub(crate) fn save_validated(data_dir: &Path, theme: &ThemeConfig) -> Result<(), String> {
    write_atomic(
        &data_dir.join("appearance.json"),
        &serde_json::to_vec_pretty(theme).map_err(|error| error.to_string())?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_roundtrip_and_invalid_input_preserve_saved_theme() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(load(directory.path()).unwrap(), ThemeConfig::default());
        let dark = ThemeConfig {
            preset: " 石墨夜色 ".into(),
            accent: "#90afff".into(),
            background: "#202227".into(),
            foreground: "#f2f3f5".into(),
            font: "system".into(),
        };
        let saved = save(directory.path(), dark).unwrap();
        assert_eq!(saved.preset, "石墨夜色");
        assert_eq!(saved.accent, "#90AFFF");
        assert_eq!(load(directory.path()).unwrap(), saved);
        let mut invalid = saved.clone();
        invalid.background = "url(secret)".into();
        assert!(save(directory.path(), invalid).is_err());
        assert_eq!(load(directory.path()).unwrap(), saved);
        let mut invalid_font = saved.clone();
        invalid_font.font = "external-font".into();
        assert!(invalid_font.validate().is_err());
        assert!(!directory.path().join("services.json").exists());
    }

    #[test]
    fn malformed_appearance_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("appearance.json"), "not-json").unwrap();
        assert!(load(directory.path()).unwrap_err().contains("格式错误"));
    }
}
