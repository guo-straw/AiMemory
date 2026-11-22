use super::*;
use serde_json::json;
use std::io::Write;

const ENABLED: &str = "2026-11-21T03:00:00.500Z";

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn meta(id: &str, source: &str) -> Value {
    json!({"timestamp":"2026-11-21T02:00:00Z","type":"session_meta","payload":{
        "id":id,"session_id":"parent-session","thread_source":source,"cwd":"/not-a-repository/project",
        "timestamp":"2026-11-21T02:00:00Z","git":{"repository_url":"git@github.com:example/project.git"}
    }})
}

fn started(id: &str) -> Value {
    json!({"timestamp":"2026-11-21T03:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":id,"root_turn_id":id}})
}

fn message(role: &str, phase: Option<&str>, text: &str) -> Value {
    json!({"timestamp":"2026-11-21T03:00:00.100Z","type":"response_item","payload":{"type":"message","role":role,"phase":phase,"content":[{"type":if role=="user" {"input_text"} else {"output_text"},"text":text}]}})
}

fn completed(id: &str, at: &str) -> Value {
    json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete","turn_id":id,"root_turn_id":id,"last_agent_message":"最终答复中已确定停止条件。","completed_at":1795230000}})
}

fn append(path: &Path, events: &[Value]) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    for event in events {
        writeln!(file, "{event}").unwrap();
    }
}

#[test]
fn completed_turn_uses_only_user_and_final_and_resumes_without_duplicates() {
    let directory = fixture();
    let path = directory.path().join("rollout-source.jsonl");
    append(
        &path,
        &[
            meta("actual-file-session", "user"),
            started("turn-1"),
            message("user", None, "请把任务状态改为每两秒查询。"),
            message("developer", None, "系统信息不参与提取。"),
            message("assistant", Some("commentary"), "正在排查。"),
            json!({"type":"response_item","payload":{"type":"reasoning","content":[{"type":"text","text":"推理不参与提取"}]}}),
            message(
                "assistant",
                Some("final_answer"),
                "最终答复中已确定停止条件。",
            ),
            completed("turn-1", "2026-11-21T03:00:00.900Z"),
        ],
    );
    let batch = read_rollout(&path, &RolloutCursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert!(batch.warnings.is_empty());
    let turn = &batch.turns[0];
    assert_eq!(turn.session_id, "actual-file-session");
    assert_eq!(turn.source_id, "codex:actual-file-session:turn-1");
    assert_eq!(turn.user_text, "请把任务状态改为每两秒查询。");
    assert_eq!(turn.assistant_text, "最终答复中已确定停止条件。");
    assert_eq!(turn.completed_at, "2026-11-21T03:00:00.900Z");
    assert_eq!(turn.repository_name, "project");
    let serialized = serde_json::to_string(&batch.cursor).unwrap();
    let restored = serde_json::from_str(&serialized).unwrap();
    let again = read_rollout(&path, &restored, ENABLED).unwrap();
    assert!(again.turns.is_empty());
    assert_eq!(again.cursor.offset, path.metadata().unwrap().len());
}

#[test]
fn snapshot_skips_history_and_retains_active_turn_before_enable() {
    let directory = fixture();
    let path = directory.path().join("rollout-active.jsonl");
    append(
        &path,
        &[
            meta("session", "user"),
            started("old-turn"),
            message("user", None, "旧问题"),
            completed("old-turn", "2026-11-21T02:59:00Z"),
            started("active-turn"),
            message("user", None, "启用之前已经发出的当前问题"),
            json!({"type":"response_item","payload":{"type":"custom_tool_call_output","output":"x".repeat(180000)}}),
        ],
    );
    let cursor = snapshot_rollout(&path).unwrap();
    assert_eq!(cursor.offset, path.metadata().unwrap().len());
    assert_eq!(cursor.active.as_ref().unwrap().id, "active-turn");
    append(&path, &[completed("active-turn", "2026-11-21T03:00:01Z")]);
    let batch = read_rollout(&path, &cursor, ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].user_text, "启用之前已经发出的当前问题");
    let complete = snapshot_rollout(&path).unwrap();
    assert!(complete.active.is_none());
    assert!(read_rollout(&path, &complete, ENABLED)
        .unwrap()
        .turns
        .is_empty());
}

#[test]
fn partial_json_line_is_read_once_after_its_newline_arrives() {
    let directory = fixture();
    let path = directory.path().join("rollout-partial.jsonl");
    append(
        &path,
        &[
            meta("session", "user"),
            started("turn"),
            message("user", None, "保留半行"),
        ],
    );
    let cursor = snapshot_rollout(&path).unwrap();
    let event = serde_json::to_string(&completed("turn", "2026-11-21T03:01:00Z")).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(event.as_bytes())
        .unwrap();
    let pending = read_rollout(&path, &cursor, ENABLED).unwrap();
    assert!(pending.turns.is_empty());
    assert_eq!(pending.cursor.offset, cursor.offset);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let batch = read_rollout(&path, &pending.cursor, ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.cursor.offset, path.metadata().unwrap().len());
}

#[test]
fn child_identity_stays_excluded_when_parent_metadata_is_replayed() {
    let directory = fixture();
    let path = directory.path().join("rollout-child.jsonl");
    append(
        &path,
        &[
            meta("child", "subagent"),
            meta("parent", "user"),
            started("replayed-turn"),
            message("user", None, "父会话复制过来的内容"),
            completed("replayed-turn", "2026-11-21T03:01:00Z"),
        ],
    );
    let batch = read_rollout(&path, &RolloutCursor::default(), ENABLED).unwrap();
    assert!(batch.turns.is_empty());
    assert_eq!(batch.cursor.session.unwrap().id, "child");
    assert!(snapshot_rollout(&path).unwrap().active.is_none());
}

#[test]
fn aborted_turns_and_old_completed_turns_do_not_archive() {
    let directory = fixture();
    let path = directory.path().join("rollout-aborted.jsonl");
    append(
        &path,
        &[
            meta("session", "user"),
            started("old"),
            message("user", None, "历史完成轮次"),
            completed("old", "2026-11-21T02:59:59Z"),
            started("aborted"),
            message("user", None, "被取消的轮次"),
            json!({"timestamp":"2026-11-21T03:01:00Z","type":"event_msg","payload":{"type":"turn_aborted","turn_id":"aborted"}}),
            completed("aborted", "2026-11-21T03:01:01Z"),
        ],
    );
    let batch = read_rollout(&path, &RolloutCursor::default(), ENABLED).unwrap();
    assert!(batch.turns.is_empty());
}

#[test]
fn metadata_fork_replay_before_creation_is_ignored() {
    let directory = fixture();
    let path = directory.path().join("rollout-fork.jsonl");
    let mut identity = meta("new-fork", "user");
    identity["payload"]["timestamp"] = json!("2026-11-21T03:02:00Z");
    let mut start = started("replay");
    start["timestamp"] = json!("2026-11-21T03:01:00Z");
    append(
        &path,
        &[
            identity,
            start,
            message("user", None, "旧历史"),
            completed("replay", "2026-11-21T03:01:01Z"),
        ],
    );
    assert!(read_rollout(&path, &RolloutCursor::default(), ENABLED)
        .unwrap()
        .turns
        .is_empty());
}

#[test]
fn directory_scan_uses_sessions_and_does_not_scan_archived_sessions() {
    let directory = fixture();
    let sessions = directory.path().join("sessions/2026/11/21");
    let archived = directory.path().join("archived_sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(&archived).unwrap();
    fs::write(sessions.join("rollout-current.jsonl"), b"").unwrap();
    fs::write(archived.join("rollout-archived.jsonl"), b"").unwrap();
    assert_eq!(
        list_rollouts(directory.path()).unwrap(),
        vec![sessions.join("rollout-current.jsonl")]
    );
    assert_eq!(
        list_rollouts(&directory.path().join("sessions"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn truncated_rollout_repositions_without_replaying_completed_history() {
    let directory = fixture();
    let path = directory.path().join("rollout-rewritten.jsonl");
    append(
        &path,
        &[
            meta("session", "user"),
            started("old"),
            message("user", None, &"历史内容".repeat(500)),
            completed("old", "2026-11-21T03:01:00Z"),
        ],
    );
    let cursor = snapshot_rollout(&path).unwrap();
    fs::write(&path, b"").unwrap();
    append(
        &path,
        &[
            meta("session", "user"),
            started("new"),
            message("user", None, "重写之后的活动轮次"),
        ],
    );
    let repositioned = read_rollout(&path, &cursor, ENABLED).unwrap();
    assert!(repositioned.turns.is_empty());
    assert_eq!(repositioned.warnings.len(), 1);
    append(&path, &[completed("new", "2026-11-21T03:02:00Z")]);
    let batch = read_rollout(&path, &repositioned.cursor, ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].turn_id, "new");
}
