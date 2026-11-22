use reqwest::{header::ACCEPT, Client, Response, StatusCode};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    env::consts::{ARCH, OS},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;

const LATEST_RELEASE: &str = "https://api.github.com/repos/guo-straw/AiMemory/releases/latest";
const RELEASE_ASSETS: &str = "https://api.github.com/repos/guo-straw/AiMemory/releases/assets";
const RELEASES: &str = "https://github.com/guo-straw/AiMemory/releases/tag";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Installer>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Installer {
    #[serde(rename(serialize = "assetId", deserialize = "id"))]
    asset_id: u64,
    name: String,
    size: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum UpdateStatus {
    Available,
    Current,
    Ahead,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateInfo {
    latest_version: String,
    status: UpdateStatus,
    release_url: String,
    installer: Option<Installer>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "lowercase")]
pub(super) enum UpdateError {
    Access(String),
    Check(String),
}

#[tauri::command]
pub(super) async fn app_update_check(
    token: Option<String>,
    app: AppHandle,
) -> Result<UpdateInfo, UpdateError> {
    check_release(
        LATEST_RELEASE,
        &app.package_info().version,
        token.as_deref(),
    )
    .await
}

async fn check_release(
    endpoint: &str,
    current: &Version,
    token: Option<&str>,
) -> Result<UpdateInfo, UpdateError> {
    let response = github_get(endpoint, "application/vnd.github+json", token).await?;
    let release: Release = response
        .json()
        .await
        .map_err(|e| UpdateError::Check(format!("无法读取 GitHub 发布信息：{e}")))?;
    let tag = release.tag_name;
    let latest = Version::parse(tag.strip_prefix('v').unwrap_or(&tag))
        .map_err(|_| UpdateError::Check(format!("发布标签 {tag} 不是有效版本号")))?;
    let status = match latest.cmp_precedence(current) {
        Ordering::Greater => UpdateStatus::Available,
        Ordering::Equal => UpdateStatus::Current,
        Ordering::Less => UpdateStatus::Ahead,
    };
    let installer = if matches!(status, UpdateStatus::Available) {
        select_installer(release.assets, &latest, OS, ARCH)
    } else {
        None
    };
    Ok(UpdateInfo {
        latest_version: latest.to_string(),
        status,
        release_url: format!("{RELEASES}/{tag}"),
        installer,
    })
}

fn select_installer(
    assets: Vec<Installer>,
    version: &Version,
    os: &str,
    arch: &str,
) -> Option<Installer> {
    let suffix = match (os, arch) {
        ("macos", "aarch64") => "aarch64.dmg",
        ("macos", "x86_64") => "x64.dmg",
        ("windows", "x86_64") => "x64-setup.exe",
        _ => return None,
    };
    let filename = format!("KnowTrace_{version}_{suffix}");
    let legacy_filename = format!("AiMemory_{version}_{suffix}");
    assets
        .into_iter()
        .find(|asset| (asset.name == filename || asset.name == legacy_filename) && asset.size > 0)
}

#[tauri::command]
pub(super) async fn app_update_download(
    asset_id: u64,
    expected_size: u64,
    path: PathBuf,
    token: Option<String>,
) -> Result<(), UpdateError> {
    let endpoint = format!("{RELEASE_ASSETS}/{asset_id}");
    download_asset(&endpoint, expected_size, &path, token.as_deref()).await
}

async fn download_asset(
    endpoint: &str,
    expected_size: u64,
    path: &Path,
    token: Option<&str>,
) -> Result<(), UpdateError> {
    let mut response = github_get(endpoint, "application/octet-stream", token).await?;
    let parent = path
        .parent()
        .ok_or_else(|| UpdateError::Check("请选择安装包的保存位置".into()))?;
    let (file, temporary) = tempfile::NamedTempFile::new_in(parent)
        .map_err(save_error)?
        .into_parts();
    let mut file = tokio::fs::File::from_std(file);
    let mut received = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| UpdateError::Check(format!("安装包下载中断，请重试：{}", e.without_url())))?
    {
        file.write_all(&chunk).await.map_err(save_error)?;
        received += chunk.len() as u64;
    }
    if received != expected_size {
        return Err(UpdateError::Check(format!(
            "安装包大小与发布记录不符（收到 {received} 字节，预期 {expected_size} 字节），请重试。"
        )));
    }
    file.flush().await.map_err(save_error)?;
    drop(file);
    temporary.persist(path).map_err(|e| save_error(e.error))?;
    Ok(())
}

fn save_error(error: std::io::Error) -> UpdateError {
    UpdateError::Check(format!("无法保存安装包：{error}"))
}

async fn github_get(
    endpoint: &str,
    accept: &str,
    token: Option<&str>,
) -> Result<Response, UpdateError> {
    let client = Client::builder()
        .user_agent(concat!("KnowTrace/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| UpdateError::Check(format!("无法初始化 GitHub 连接：{e}")))?;
    let token = token.map(str::trim).filter(|value| !value.is_empty());
    let mut request = client.get(endpoint).header(ACCEPT, accept);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| {
        UpdateError::Check(format!(
            "连接 GitHub 失败，请检查网络后重试：{}",
            e.without_url()
        ))
    })?;
    check_response_status(&response, token.is_some())?;
    Ok(response)
}

fn check_response_status(response: &Response, authenticated: bool) -> Result<(), UpdateError> {
    let status = response.status();
    let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|v| v == "0"));
    let error = match status {
        _ if status.is_success() => return Ok(()),
        _ if rate_limited && !authenticated => {
            UpdateError::Access("GitHub 匿名请求已达限额，请稍后重试或使用只读令牌检查。".into())
        }
        _ if rate_limited => UpdateError::Check("GitHub 请求已达限额，请稍后重试。".into()),
        StatusCode::UNAUTHORIZED => {
            UpdateError::Access("GitHub 身份验证失败，请检查令牌是否有效。".into())
        }
        StatusCode::FORBIDDEN => {
            UpdateError::Access("GitHub 拒绝读取发布信息，请检查令牌的仓库读取权限。".into())
        }
        StatusCode::NOT_FOUND => {
            UpdateError::Access("未找到可访问的发布资源，请确认令牌对发布仓库有读取权限。".into())
        }
        _ => UpdateError::Check(format!("GitHub 请求失败（HTTP {status}），请稍后重试。")),
    };
    Err(error)
}

#[cfg(test)]
mod tests;
