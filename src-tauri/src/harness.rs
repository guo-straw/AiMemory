use crate::codex::{self, CodexTurn};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::Cursor as IoCursor,
    path::{Path, PathBuf},
};

const SESSION_VERSION: u64 = 4;
const ZSTD_MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

#[derive(Clone, Debug)]
pub struct Event {
    pub line: u64,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct SessionLog {
    pub header: Value,
    pub events: Vec<Event>,
    pub offset: u64,
    pub seed_cutoff: Option<u64>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub offset: u64,
    pub revision: String,
}

pub struct Batch {
    pub cursor: Cursor,
    pub turns: Vec<CodexTurn>,
    pub warnings: Vec<String>,
}

pub fn default_harness_directory() -> PathBuf {
    if let Some(home) = std::env::var_os("DSH_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".dsh")
}

pub fn root_directory(directory: &Path) -> PathBuf {
    let directory = fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    let root = if directory.file_name().is_some_and(|name| name == "sessions") {
        directory.parent().unwrap_or(&directory).to_path_buf()
    } else {
        directory
    };
    fs::canonicalize(&root).unwrap_or(root)
}

pub fn list_sessions(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let sessions = root_directory(directory).join("sessions");
    let mut paths = Vec::new();
    for project in directories(&sessions)? {
        for session in directories(&project)? {
            if let Some(path) = select_generation(&session)? {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn directories(path: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取 Harness 目录 {}：{error}", path.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn select_generation(directory: &Path) -> Result<Option<PathBuf>, String> {
    let mut selected: Option<(u64, PathBuf)> = None;
    let mut encoding = None;
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some((version, compressed)) = generation(&name.to_string_lossy()) else {
            continue;
        };
        if encoding.is_some_and(|previous| previous != compressed) {
            return Err(format!(
                "Harness 会话目录同时存在 JSONL 与 Zstandard 代际：{}",
                directory.display()
            ));
        }
        encoding = Some(compressed);
        if selected
            .as_ref()
            .is_none_or(|(previous, _)| version > *previous)
        {
            selected = Some((version, entry.path()));
        }
    }
    Ok(selected.map(|(_, path)| path))
}

fn generation(name: &str) -> Option<(u64, bool)> {
    let (name, compressed) = name
        .strip_suffix(".zstd")
        .map(|name| (name, true))
        .unwrap_or((name, false));
    if name == "session.jsonl" {
        return Some((0, compressed));
    }
    let number = name.strip_prefix("session.v")?.strip_suffix(".jsonl")?;
    if number.starts_with('0') || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((number.parse().ok()?, compressed))
}

pub fn read_session(path: &Path) -> Result<SessionLog, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("无法读取 Harness 会话 {}：{error}", path.display()))?;
    let mut warnings = Vec::new();
    let (plain, offset) = if path
        .extension()
        .is_some_and(|extension| extension == "zstd")
    {
        decode_frames(path, &bytes, &mut warnings)?
    } else {
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |end| end + 1);
        if end < bytes.len() {
            warnings.push(format!(
                "{} 的末尾 JSONL 行尚未完成，等待下次读取",
                path.display()
            ));
        }
        (bytes[..end].to_vec(), end)
    };
    let mut lines = plain.split_inclusive(|byte| *byte == b'\n');
    let first = lines
        .next()
        .ok_or_else(|| format!("Harness 会话头尚未完整：{}", path.display()))?;
    let header = parse_line(path, 1, first)?;
    validate_header(path, &header)?;
    let mut events = Vec::new();
    let mut seed_cutoff = None;
    for (index, line) in lines.enumerate() {
        let number = index as u64 + 2;
        let value = parse_line(path, number, line)?;
        let expected = events.len() as u64;
        if !value.is_object()
            || value["type"].as_str().is_none()
            || value["seq"].as_u64() != Some(expected)
            || value["time"].as_i64().is_none()
            || value.get("data").is_none()
        {
            return Err(format!(
                "Harness 事件结构或 seq 不连续：{} 第 {number} 行（预期 seq {expected}）",
                path.display()
            ));
        }
        if value["type"] == "session/end-seed" && value["data"]["inherited"] == true {
            seed_cutoff = Some(expected);
        }
        events.push(Event {
            line: number,
            value,
        });
    }
    if header["isSeeded"].as_bool() != Some(seed_cutoff.is_some()) {
        return Err(format!(
            "Harness 继承标记与 isSeeded 不一致：{}",
            path.display()
        ));
    }
    Ok(SessionLog {
        header,
        events,
        offset: offset as u64,
        seed_cutoff,
        warnings,
    })
}

fn decode_frames(
    path: &Path,
    bytes: &[u8],
    warnings: &mut Vec<String>,
) -> Result<(Vec<u8>, usize), String> {
    let mut plain = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let pending = || {
            format!(
                "{} 字节 {offset} 的末尾 Zstandard 帧尚未完成，等待下次读取",
                path.display()
            )
        };
        if remaining.len() < ZSTD_MAGIC.len() {
            warnings.push(pending());
            break;
        }
        if !remaining.starts_with(ZSTD_MAGIC) {
            return Err(format!(
                "Harness Zstandard 帧 magic 错误：{} 字节 {offset}",
                path.display()
            ));
        }
        let length = match zstd::zstd_safe::find_frame_compressed_size(remaining) {
            Ok(length) => length,
            Err(code) if zstd::zstd_safe::get_error_name(code) == "Src size is incorrect" => {
                warnings.push(pending());
                break;
            }
            Err(code) => {
                return Err(format!(
                    "Harness Zstandard 帧结构损坏：{} 字节 {offset}：{}",
                    path.display(),
                    zstd::zstd_safe::get_error_name(code)
                ))
            }
        };
        let frame =
            zstd::stream::decode_all(IoCursor::new(&remaining[..length])).map_err(|error| {
                format!(
                    "Harness Zstandard 帧解码或校验失败：{} 字节 {offset}：{error}",
                    path.display()
                )
            })?;
        if frame.last() != Some(&b'\n')
            || (offset == 0 && frame.iter().filter(|byte| **byte == b'\n').count() != 1)
        {
            return Err(format!(
                "Harness Zstandard 完整帧未包含完整 JSONL 记录：{} 字节 {offset}",
                path.display()
            ));
        }
        plain.extend_from_slice(&frame);
        offset += length;
    }
    Ok((plain, offset))
}

fn parse_line(path: &Path, line: u64, bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|error| {
        format!(
            "Harness JSONL 无效：{} 第 {line} 行：{error}",
            path.display()
        )
    })
}

fn validate_header(path: &Path, header: &Value) -> Result<(), String> {
    if header["version"].as_u64() != Some(SESSION_VERSION) {
        return Err(format!(
            "尚不支持 Harness 会话格式 {}：{}",
            header["version"],
            path.display()
        ));
    }
    if header["type"] != "session"
        || header["id"].as_str().is_none_or(str::is_empty)
        || header["createdAt"].as_u64().is_none()
        || header["isSeeded"].as_bool().is_none()
        || header["delegationDepth"].as_u64().is_none()
        || header
            .get("cwd")
            .is_some_and(|cwd| cwd.as_str().is_none_or(|cwd| !Path::new(cwd).is_absolute()))
        || header
            .get("parentSession")
            .is_some_and(|parent| !parent.is_string())
        || header
            .get("origin")
            .is_some_and(|origin| origin != "subagent")
    {
        return Err(format!("Harness V4 会话头字段无效：{}", path.display()));
    }
    Ok(())
}

pub fn revision(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("无法读取 Harness 会话状态 {}：{error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata.modified().map_err(|error| error.to_string())?;
        Ok(format!("{}:{modified:?}", metadata.len()))
    }
}

pub fn snapshot_session(path: &Path) -> Result<Cursor, String> {
    // Capture before reading: a concurrent append must trigger another scan, not be marked consumed.
    let revision = revision(path)?;
    let log = read_session(path)?;
    Ok(Cursor {
        offset: log.offset,
        revision,
    })
}

pub fn read_turns(path: &Path, cursor: &Cursor, enabled_at: &str) -> Result<Batch, String> {
    let enabled_at = DateTime::parse_from_rfc3339(enabled_at)
        .map_err(|error| format!("归档起点格式错误：{error}"))?
        .with_timezone(&Utc);
    let revision = revision(path)?;
    if revision == cursor.revision {
        return Ok(Batch {
            cursor: cursor.clone(),
            turns: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let log = read_session(path)?;
    let mut warnings = log.warnings.clone();
    if log.offset < cursor.offset {
        warnings.push(format!(
            "Harness 记录已截短重写，已完整重放 {}",
            path.display()
        ));
    }
    // Rebuild changed files, including open turns; stable source IDs let the pipeline deduplicate replays.
    let turns = archive_turns(path, &log, enabled_at);
    Ok(Batch {
        cursor: Cursor {
            offset: log.offset,
            revision,
        },
        turns,
        warnings,
    })
}

struct PendingTurn {
    id: u64,
    cwd: String,
    user_messages: Vec<String>,
    final_text: Option<String>,
    code_paths: BTreeSet<PathBuf>,
}

fn archive_turns(path: &Path, log: &SessionLog, enabled_at: DateTime<Utc>) -> Vec<CodexTurn> {
    if log.header["origin"] == "subagent" {
        return Vec::new();
    }
    let id = log.header["id"].as_str().unwrap_or_default();
    let mut cwd = log.header["cwd"].as_str().unwrap_or_default().to_string();
    let mut active: Option<PendingTurn> = None;
    let mut turns = Vec::new();
    for event in &log.events {
        let value = &event.value;
        let data = &value["data"];
        let kind = value["type"].as_str().unwrap_or_default();
        if kind == "working-directory/change" {
            if let Some(directory) = data["cwd"].as_str() {
                cwd = directory.to_string();
                if let Some(active) = active.as_mut() {
                    active.cwd = cwd.clone();
                    active.code_paths.insert(PathBuf::from(directory));
                }
            }
        }
        if log
            .seed_cutoff
            .is_some_and(|cutoff| value["seq"].as_u64().is_some_and(|seq| seq <= cutoff))
        {
            continue;
        }
        match kind {
            "turn/start" => {
                active = data["turn"]
                    .as_u64()
                    .filter(|_| event_time(value).is_some_and(|time| time >= enabled_at))
                    .map(|id| PendingTurn {
                        id,
                        cwd: cwd.clone(),
                        user_messages: Vec::new(),
                        final_text: None,
                        code_paths: if cwd.is_empty() {
                            BTreeSet::new()
                        } else {
                            BTreeSet::from([PathBuf::from(&cwd)])
                        },
                    });
            }
            "user/message" if data["source"]["kind"] == "user" => {
                if let Some(active) = active.as_mut() {
                    let text = message_text(data);
                    if !text.trim().is_empty() {
                        crate::code_context::collect_paths(
                            &text,
                            &active.cwd,
                            &mut active.code_paths,
                        );
                        active.user_messages.push(text);
                    }
                }
            }
            "assistant/message" => {
                if let Some(active) = active
                    .as_mut()
                    .filter(|active| data["turn"].as_u64() == Some(active.id))
                {
                    if data["interrupted"] != true {
                        let text = message_text(&data["message"]);
                        if !text.trim().is_empty() {
                            crate::code_context::collect_paths(
                                &text,
                                &active.cwd,
                                &mut active.code_paths,
                            );
                            active.final_text = Some(text);
                        }
                    }
                }
            }
            "tool/call" => {
                if let Some(active) = active
                    .as_mut()
                    .filter(|active| data["turn"].as_u64() == Some(active.id))
                {
                    if let Some(arguments) = data["arguments"].as_str() {
                        crate::code_context::collect_tool_paths(
                            arguments,
                            &active.cwd,
                            &mut active.code_paths,
                        );
                    }
                }
            }
            "turn/end" => {
                if active
                    .as_ref()
                    .is_none_or(|active| data["turn"].as_u64() != Some(active.id))
                {
                    continue;
                }
                let pending = active.take().unwrap();
                if data["reason"]["kind"] != "completed" || pending.user_messages.is_empty() {
                    continue;
                }
                if let (Some(completed), Some(assistant)) =
                    (event_time(value), pending.final_text.as_ref())
                {
                    turns.push(finish_turn(path, id, &pending, assistant, completed));
                }
            }
            _ => {}
        }
    }
    turns
}

fn message_text(message: &Value) -> String {
    message["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn event_time(event: &Value) -> Option<DateTime<Utc>> {
    event["time"]
        .as_i64()
        .and_then(DateTime::from_timestamp_millis)
}

fn finish_turn(
    path: &Path,
    session: &str,
    pending: &PendingTurn,
    assistant: &str,
    completed: DateTime<Utc>,
) -> CodexTurn {
    let remote = if pending.cwd.is_empty() {
        None
    } else {
        codex::git_remote(&pending.cwd)
    };
    let code_repositories = if pending.cwd.is_empty() {
        let absolute_paths = pending
            .code_paths
            .iter()
            .filter(|path| path.is_absolute())
            .cloned()
            .collect();
        crate::code_context::resolve_repositories(&absolute_paths)
    } else {
        crate::code_context::resolve_repositories(&pending.code_paths)
    };
    let repository_name = remote
        .as_deref()
        .and_then(|remote| remote.trim_end_matches('/').rsplit(['/', ':']).next())
        .map(|name| name.trim_end_matches(".git").to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            Path::new(&pending.cwd)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
    CodexTurn {
        source_id: format!("harness:{session}:{}", pending.id),
        session_id: format!("harness:{session}"),
        turn_id: pending.id.to_string(),
        completed_at: completed.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        cwd: pending.cwd.clone(),
        repository_remote: remote,
        repository_name,
        user_text: pending.user_messages.join("\n\n"),
        assistant_text: assistant.to_string(),
        session_path: path.display().to_string(),
        code_repositories,
    }
}

#[cfg(test)]
mod tests;
