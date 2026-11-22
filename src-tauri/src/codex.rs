use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurn {
    pub source_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub completed_at: String,
    pub cwd: String,
    pub repository_remote: Option<String>,
    pub repository_name: String,
    pub user_text: String,
    pub assistant_text: String,
    pub session_path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutCursor {
    pub offset: u64,
    session: Option<SessionIdentity>,
    active: Option<PendingTurn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdentity {
    id: String,
    cwd: String,
    remote: Option<String>,
    eligible: bool,
    created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingTurn {
    id: String,
    cwd: String,
    user_messages: Vec<String>,
    user_event_text: Option<String>,
    final_text: Option<String>,
}

pub struct RolloutBatch {
    pub cursor: RolloutCursor,
    pub turns: Vec<CodexTurn>,
    pub warnings: Vec<String>,
}

pub fn default_codex_directory() -> PathBuf {
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".codex")
}

pub fn list_rollouts(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let sessions = directory.join("sessions");
    collect_rollouts(
        if sessions.is_dir() {
            &sessions
        } else {
            directory
        },
        &mut paths,
    )?;
    paths.sort();
    Ok(paths)
}

pub fn snapshot_rollout(path: &Path) -> Result<RolloutCursor, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let end = complete_line_end(&mut file)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut cursor = RolloutCursor::default();
    let mut first = Vec::new();
    BufReader::new(&mut file)
        .read_until(b'\n', &mut first)
        .map_err(|error| error.to_string())?;
    if first.last() == Some(&b'\n') {
        if let Ok(event) = serde_json::from_slice::<Value>(&first) {
            set_session(&mut cursor, &event);
        }
    }
    if cursor
        .session
        .as_ref()
        .is_some_and(|session| !session.eligible)
    {
        cursor.offset = end;
        return Ok(cursor);
    }
    if let Some(start) = active_turn_start(&mut file, end)? {
        cursor.offset = start;
        cursor = read_range(path, &mut file, &cursor, end, None)?.cursor;
    }
    cursor.offset = end;
    Ok(cursor)
}

pub fn read_rollout(
    path: &Path,
    cursor: &RolloutCursor,
    enabled_at: &str,
) -> Result<RolloutBatch, String> {
    let enabled_at = DateTime::parse_from_rfc3339(enabled_at)
        .map_err(|error| format!("归档起点格式错误：{error}"))?
        .with_timezone(&Utc);
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length < cursor.offset {
        return Ok(RolloutBatch {
            cursor: snapshot_rollout(path)?,
            turns: Vec::new(),
            warnings: vec![format!("Codex 记录已重写，已重新定位 {}", path.display())],
        });
    }
    if cursor
        .session
        .as_ref()
        .is_some_and(|session| !session.eligible)
    {
        let mut cursor = cursor.clone();
        cursor.offset = length;
        return Ok(RolloutBatch {
            cursor,
            turns: Vec::new(),
            warnings: Vec::new(),
        });
    }
    read_range(path, &mut file, cursor, length, Some(enabled_at))
}

fn read_range(
    path: &Path,
    file: &mut File,
    previous: &RolloutCursor,
    end: u64,
    enabled_at: Option<DateTime<Utc>>,
) -> Result<RolloutBatch, String> {
    let mut batch = RolloutBatch {
        cursor: previous.clone(),
        turns: Vec::new(),
        warnings: Vec::new(),
    };
    file.seek(SeekFrom::Start(previous.offset))
        .map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(file.take(end - previous.offset));
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if bytes == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let offset = batch.cursor.offset;
        batch.cursor.offset += bytes as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let event = match serde_json::from_slice::<Value>(&line) {
            Ok(event) => event,
            Err(_) => {
                batch.warnings.push(format!(
                    "{} 的字节 {offset} 处不是有效 JSONL",
                    path.display()
                ));
                continue;
            }
        };
        if let Some(turn) = process_event(&mut batch.cursor, &event, path, enabled_at) {
            batch.turns.push(turn);
        }
        if batch
            .cursor
            .session
            .as_ref()
            .is_some_and(|session| !session.eligible)
        {
            batch.cursor.offset = end;
            break;
        }
    }
    Ok(batch)
}

fn process_event(
    cursor: &mut RolloutCursor,
    event: &Value,
    path: &Path,
    enabled_at: Option<DateTime<Utc>>,
) -> Option<CodexTurn> {
    let payload = &event["payload"];
    let event_type = event["type"].as_str()?;
    if event_type == "session_meta" {
        set_session(cursor, event);
        return None;
    }
    let session = cursor.session.as_ref()?;
    if !session.eligible {
        return None;
    }
    if event_type == "event_msg" {
        match payload["type"].as_str()? {
            "task_started" => {
                if session
                    .created_at
                    .is_some_and(|created| event_time(event).is_some_and(|time| time < created))
                {
                    return None;
                }
                cursor.active = Some(PendingTurn {
                    id: payload["turn_id"].as_str()?.to_string(),
                    cwd: session.cwd.clone(),
                    user_messages: Vec::new(),
                    user_event_text: None,
                    final_text: None,
                });
            }
            "turn_aborted" => {
                if cursor.active.as_ref().is_some_and(|active| {
                    payload["turn_id"].as_str().is_none_or(|id| id == active.id)
                }) {
                    cursor.active = None;
                }
            }
            "user_message" => {
                if let Some(active) = cursor.active.as_mut() {
                    active.user_event_text = payload["message"].as_str().map(str::to_string);
                }
            }
            "task_complete" => {
                let active = cursor.active.as_ref()?;
                if payload["turn_id"].as_str()? != active.id {
                    return None;
                }
                let active = cursor.active.take()?;
                let completed = event_time(event)?;
                if enabled_at.is_none_or(|start| completed < start) {
                    return None;
                }
                let user_text = if active.user_messages.is_empty() {
                    active.user_event_text.unwrap_or_default()
                } else {
                    active.user_messages.join("\n\n")
                };
                let assistant_text = payload["last_agent_message"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_string)
                    .or(active.final_text)
                    .unwrap_or_default();
                if user_text.trim().is_empty() || assistant_text.trim().is_empty() {
                    return None;
                }
                let remote = session.remote.clone().or_else(|| git_remote(&active.cwd));
                let repository_name = remote
                    .as_deref()
                    .and_then(|remote| remote.trim_end_matches('/').rsplit(['/', ':']).next())
                    .map(|name| name.trim_end_matches(".git").to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        Path::new(&active.cwd)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    });
                return Some(CodexTurn {
                    source_id: format!("codex:{}:{}", session.id, active.id),
                    session_id: session.id.clone(),
                    turn_id: active.id,
                    completed_at: completed.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    cwd: active.cwd,
                    repository_remote: remote,
                    repository_name,
                    user_text,
                    assistant_text,
                    session_path: path.display().to_string(),
                });
            }
            _ => {}
        }
    } else if event_type == "turn_context" {
        if let Some(active) = cursor.active.as_mut() {
            if payload["turn_id"].as_str() == Some(active.id.as_str()) {
                if let Some(cwd) = payload["cwd"].as_str() {
                    active.cwd = cwd.to_string();
                }
            }
        }
    } else if event_type == "response_item" && payload["type"].as_str() == Some("message") {
        let active = cursor.active.as_mut()?;
        let text = message_text(payload);
        if !text.is_empty() {
            match payload["role"].as_str() {
                Some("user") => active.user_messages.push(text),
                Some("assistant")
                    if matches!(payload["phase"].as_str(), Some("final_answer" | "final")) =>
                {
                    active.final_text = Some(text)
                }
                _ => {}
            }
        }
    }
    None
}

fn set_session(cursor: &mut RolloutCursor, event: &Value) {
    if cursor.session.is_some() || event["type"].as_str() != Some("session_meta") {
        return;
    }
    let meta = &event["payload"];
    let Some(id) = meta["id"].as_str().or_else(|| meta["session_id"].as_str()) else {
        return;
    };
    // Child rollouts replay their parent's metadata after their own first identity record.
    let eligible = match meta["thread_source"].as_str() {
        Some(source) => source == "user",
        None => meta["source"].get("subagent").is_none(),
    };
    cursor.session = Some(SessionIdentity {
        id: id.to_string(),
        cwd: meta["cwd"].as_str().unwrap_or_default().to_string(),
        remote: meta["git"]["repository_url"].as_str().map(str::to_string),
        eligible,
        created_at: meta["timestamp"]
            .as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| time.with_timezone(&Utc)),
    });
}

fn event_time(event: &Value) -> Option<DateTime<Utc>> {
    event["timestamp"]
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
        .or_else(|| {
            event["payload"]["completed_at"]
                .as_i64()
                .and_then(|time| DateTime::from_timestamp(time, 0))
        })
}

fn message_text(payload: &Value) -> String {
    payload["content"]
        .as_array()
        .map(|content| {
            content
                .iter()
                .filter(|part| {
                    matches!(
                        part["type"].as_str(),
                        Some("input_text" | "output_text" | "text")
                    )
                })
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn git_remote(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", cwd, "config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!remote.is_empty()).then_some(remote)
}

fn collect_rollouts(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            collect_rollouts(&entry.path(), paths)?;
        } else if kind.is_file()
            && entry.file_name().to_string_lossy().starts_with("rollout-")
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn complete_line_end(file: &mut File) -> Result<u64, String> {
    let mut position = file.metadata().map_err(|error| error.to_string())?.len();
    let mut buffer = vec![0; 65536];
    while position > 0 {
        let start = position.saturating_sub(buffer.len() as u64);
        let length = (position - start) as usize;
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        file.read_exact(&mut buffer[..length])
            .map_err(|error| error.to_string())?;
        if let Some(index) = buffer[..length].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        position = start;
    }
    Ok(0)
}

fn active_turn_start(file: &mut File, end: u64) -> Result<Option<u64>, String> {
    let mut position = end;
    let mut prefix = Vec::new();
    while position > 0 {
        let start = position.saturating_sub(65536);
        let mut block = vec![0; (position - start) as usize];
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        file.read_exact(&mut block)
            .map_err(|error| error.to_string())?;
        block.extend_from_slice(&prefix);
        let mut line_end = block.len();
        prefix.clear();
        while line_end > 0 {
            let content_end = if block[line_end - 1] == b'\n' {
                line_end - 1
            } else {
                line_end
            };
            let line_start = block[..content_end]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(0);
            if line_start == 0 && start > 0 {
                prefix.extend_from_slice(&block[..line_end]);
                break;
            }
            if let Ok(event) = serde_json::from_slice::<Value>(&block[line_start..content_end]) {
                if event["type"].as_str() == Some("event_msg") {
                    match event["payload"]["type"].as_str() {
                        Some("task_started") => return Ok(Some(start + line_start as u64)),
                        Some("task_complete" | "turn_aborted") => return Ok(None),
                        _ => {}
                    }
                }
            }
            line_end = line_start;
        }
        position = start;
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
