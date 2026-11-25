use super::*;
use serde_json::json;
use std::io::Write;

const BASE: i64 = 1_795_230_000_000;
const ENABLED: &str = "2026-11-21T03:00:00.500Z";

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn header(id: &str) -> Value {
    let cwd = std::env::temp_dir()
        .join("knowtrace-test-non-repository")
        .join("project");
    json!({"type":"session","version":4,"id":id,"createdAt":BASE-60_000,
        "cwd":cwd.to_string_lossy(),"isSeeded":false,"delegationDepth":0,
        "agentPreset":"standard"})
}

fn event(kind: &str, time: i64, data: Value) -> Value {
    json!({"type":kind,"seq":0,"time":BASE+time,"data":data})
}

fn user(text: &str, time: i64) -> Value {
    let mut value = event(
        "user/message",
        time,
        json!({"id":format!("user-{time}"),"role":"user","source":{"kind":"user"},
            "content":[{"type":"text","text":text}]}),
    );
    value["surfaceOp"] = json!("append");
    value
}

fn assistant(turn: u64, step: u64, text: &str, time: i64) -> Value {
    let usage = json!({"inputTokens":3,"outputTokens":2,"cacheReadTokens":5,"cacheWriteTokens":1,"totalTokens":11});
    let mut value = event(
        "assistant/message",
        time + 10,
        json!({
            "turn":turn,"step":step,
            "message":{"id":format!("assistant-{turn}-{step}-{time}"),"role":"assistant",
                "source":{"kind":"model","provider":"fixture-provider","model":"fixture-model","replayState":{"model":"fixture-model"}},
                "content":[{"type":"text","text":text}]},
            "usage":usage,
            "stream":[
                {"type":"chunk","time":BASE+time,"chunk":{"type":"block-start","index":0,"blockType":"text"}},
                {"type":"text-chunks","time0":BASE+time,"index":0,"dt":[],"texts":[text]},
                {"type":"chunk","time":BASE+time+10,"chunk":{"type":"block-end","index":0,"block":{"type":"text","text":text}}},
                {"type":"chunk","time":BASE+time+10,"chunk":{"type":"usage","usage":usage}},
                {"type":"chunk","time":BASE+time+10,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
            ]
        }),
    );
    value["surfaceOp"] = json!("append");
    value
}

fn turn(id: u64, start: i64) -> Vec<Value> {
    vec![
        event("turn/start", start, json!({"turn":id})),
        event("step/start", start + 10, json!({"turn":id,"step":1})),
        user(&format!("问题 {id}"), start + 20),
        event(
            "request/header",
            start + 30,
            json!({"header":{"config":{"provider":"fixture-provider","model":"fixture-model","reasoningEffort":"high"}},"reason":"initial"}),
        ),
        assistant(id, 1, &format!("答复 {id}"), start + 100),
        event("step/end", start + 200, json!({"turn":id,"step":1})),
        event(
            "turn/end",
            start + 300,
            json!({"turn":id,"reason":{"kind":"completed"}}),
        ),
    ]
}

fn numbered(events: &[Value], start: u64) -> Vec<Value> {
    events
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let mut value = value.clone();
            value["seq"] = json!(start + index as u64);
            value
        })
        .collect()
}

fn jsonl(rows: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).unwrap();
        bytes.push(b'\n');
    }
    bytes
}

fn plaintext(head: Value, events: &[Value]) -> Vec<u8> {
    let mut rows = vec![head];
    rows.extend(numbered(events, 0));
    jsonl(&rows)
}

fn frame(rows: &[Value]) -> Vec<u8> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(&jsonl(rows)).unwrap();
    encoder.finish().unwrap()
}

fn write_plain(path: &Path, head: Value, events: &[Value]) {
    fs::write(path, plaintext(head, events)).unwrap();
}

fn append_plain(path: &Path, events: &[Value], start: u64) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&jsonl(&numbered(events, start)))
        .unwrap();
}

#[test]
fn discovers_only_the_highest_canonical_generation_and_normalizes_roots() {
    let fixture = fixture();
    let root = fixture.path().join(".dsh");
    let first = root.join("sessions/--lossy-project-key--/session-one");
    let second = root.join("sessions/--other--/session-two");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    for name in [
        "session.jsonl",
        "session.v2.jsonl",
        "session.v4.jsonl",
        "session.v10.jsonl",
        "session.v04.jsonl",
        "session.v0.jsonl",
        "session.v11.jsonl.tmp",
        "session.v12.JSONL",
        "session.lock",
    ] {
        fs::write(first.join(name), b"").unwrap();
    }
    for name in [
        "session.v3.jsonl.zstd",
        "session.v4.jsonl.zstd",
        "session.v5.jsonl.zstd.123.tmp",
    ] {
        fs::write(second.join(name), b"").unwrap();
    }
    let mut expected = vec![
        first.join("session.v10.jsonl").canonicalize().unwrap(),
        second.join("session.v4.jsonl.zstd").canonicalize().unwrap(),
    ];
    expected.sort();
    assert_eq!(list_sessions(&root).unwrap(), expected);
    assert_eq!(list_sessions(&root.join("sessions")).unwrap(), expected);
    assert_eq!(
        root_directory(&root.join("sessions")),
        root.canonicalize().unwrap()
    );
    assert!(list_sessions(&fixture.path().join("absent"))
        .unwrap()
        .is_empty());
}

#[test]
fn mixed_encodings_in_a_session_directory_are_not_silently_selected() {
    let fixture = fixture();
    let session = fixture.path().join("sessions/project/id");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v3.jsonl"), b"").unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"").unwrap();
    assert!(list_sessions(fixture.path())
        .unwrap_err()
        .contains("同时存在"));
}

#[test]
fn independent_frames_keep_raw_stream_usage_source_ranges_and_physical_lines() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl.zstd");
    let mut events = turn(1, 1000);
    let mut context = user("synthetic context", 1400);
    context["data"]["source"] = json!({"kind":"runtime-context","form":"snapshot","sections":[]});
    context["sourceEventSeqs"] = json!([[0, 2], 4]);
    events.push(context);
    events.push(event(
        "plugin:fixture/opaque",
        1500,
        json!(["preserve",{"value":3}]),
    ));
    let rows = numbered(&events, 0);
    let mut bytes = frame(&[header("raw-session")]);
    bytes.extend(frame(&rows[..4]));
    bytes.extend(frame(&rows[4..]));
    fs::write(&path, &bytes).unwrap();
    let log = read_session(&path).unwrap();
    assert_eq!(log.header["id"], "raw-session");
    assert_eq!(log.events.len(), rows.len());
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(log.events[index].line, index as u64 + 2);
        assert_eq!(&log.events[index].value, row);
    }
    assert_eq!(log.events[4].value["data"]["usage"]["inputTokens"], 3);
    assert_eq!(log.events[7].value["sourceEventSeqs"], json!([[0, 2], 4]));
    assert_eq!(log.offset, bytes.len() as u64);
    assert!(log.seed_cutoff.is_none());
    assert!(log.warnings.is_empty());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn torn_final_frames_commit_no_rows_until_the_entire_frame_arrives() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl.zstd");
    let events = numbered(&turn(1, 1000), 0);
    let mut prefix = frame(&[header("partial")]);
    prefix.extend(frame(&events[..4]));
    let tail = frame(&events[4..]);
    for cut in [1, 2, 3, 4, 5, tail.len() / 2, tail.len() - 1] {
        let mut bytes = prefix.clone();
        bytes.extend_from_slice(&tail[..cut]);
        fs::write(&path, &bytes).unwrap();
        let log = read_session(&path).unwrap();
        assert_eq!(log.events.len(), 4, "cut={cut}");
        assert_eq!(log.offset, prefix.len() as u64, "cut={cut}");
        assert_eq!(log.warnings.len(), 1);
    }
    let cut = tail.len() / 2;
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(&tail[..cut]);
    fs::write(&path, bytes).unwrap();
    let pending = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert!(pending.turns.is_empty());
    assert_eq!(pending.cursor.offset, prefix.len() as u64);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&tail[cut..])
        .unwrap();
    let complete = read_turns(&path, &pending.cursor, ENABLED).unwrap();
    assert_eq!(complete.turns.len(), 1);
    assert_eq!(complete.turns[0].source_id, "harness:partial:1");
    assert_eq!(complete.cursor.offset, (prefix.len() + tail.len()) as u64);
    assert!(read_turns(&path, &complete.cursor, ENABLED)
        .unwrap()
        .turns
        .is_empty());
}

#[test]
fn checksum_errors_are_located_and_not_advanced_past() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl.zstd");
    let prefix = frame(&[header("checksum")]);
    let tail = frame(&numbered(&turn(1, 1000), 0));
    let mut corrupt = tail.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    let mut bytes = prefix.clone();
    bytes.extend(corrupt);
    fs::write(&path, &bytes).unwrap();
    let error = read_session(&path).unwrap_err();
    assert!(error.contains("校验失败"), "{error}");
    assert!(error.contains(&format!("字节 {}", prefix.len())), "{error}");
    assert!(snapshot_session(&path).is_err());
    assert!(read_turns(&path, &Cursor::default(), ENABLED).is_err());
    bytes.truncate(prefix.len());
    bytes.extend(tail);
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        read_turns(&path, &Cursor::default(), ENABLED)
            .unwrap()
            .turns
            .len(),
        1
    );
}

#[test]
fn a_bad_magic_or_complete_frame_with_an_unterminated_record_is_corruption() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl.zstd");
    let prefix = frame(&[header("corrupt")]);
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(b"invalid-frame");
    fs::write(&path, bytes).unwrap();
    assert!(read_session(&path).unwrap_err().contains("magic"));
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(br#"{"type":"turn/start"}"#).unwrap();
    let mut bytes = prefix;
    bytes.extend(encoder.finish().unwrap());
    fs::write(&path, bytes).unwrap();
    assert!(read_session(&path).unwrap_err().contains("完整 JSONL"));
}

#[test]
fn plain_jsonl_requires_a_complete_newline_and_preserves_pending_turns_by_replay() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let events = turn(1, 1000);
    write_plain(&path, header("plain"), &events[..6]);
    let offset = fs::metadata(&path).unwrap().len();
    let tail = serde_json::to_vec(&numbered(&events[6..], 6)[0]).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&tail)
        .unwrap();
    let pending = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert!(pending.turns.is_empty());
    assert_eq!(pending.cursor.offset, offset);
    assert_eq!(pending.warnings.len(), 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let batch = read_turns(&path, &pending.cursor, ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert!(batch.warnings.is_empty());
    assert_eq!(batch.turns[0].session_path, path.display().to_string());
}

#[test]
fn only_completed_root_turns_with_human_input_and_final_text_are_archived() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut events = turn(1, 1000);
    for (id, kind) in [
        (2, "aborted"),
        (3, "error"),
        (4, "max-tokens"),
        (5, "interrupted"),
        (6, "forked"),
        (7, "blocked"),
    ] {
        let mut rejected = turn(id, id as i64 * 1000);
        rejected.last_mut().unwrap()["data"]["reason"] = match kind {
            "aborted" => json!({"kind":kind,"reason":{"kind":"user"}}),
            "error" => json!({"kind":kind,"error":{"message":"fixture failure","code":"UNKNOWN"}}),
            _ => json!({"kind":kind}),
        };
        events.extend(rejected);
    }
    let mut synthetic = turn(8, 8000);
    synthetic[2]["data"]["source"] =
        json!({"kind":"runtime-context","form":"snapshot","sections":[]});
    events.extend(synthetic);
    let mut no_text = turn(9, 9000);
    no_text[4]["data"]["message"]["content"] =
        json!([{"type":"reasoning","text":"not a final reply"}]);
    events.extend(no_text);
    let mut interrupted = turn(10, 10000);
    interrupted[4]["data"]["interrupted"] = json!(true);
    events.extend(interrupted);
    let open = turn(11, 11000);
    events.extend_from_slice(&open[..open.len() - 1]);
    write_plain(&path, header("root"), &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    let turn = &batch.turns[0];
    assert_eq!(turn.session_id, "harness:root");
    assert_eq!(turn.turn_id, "1");
    assert_eq!(turn.source_id, "harness:root:1");
    assert_eq!(turn.user_text, "问题 1");
    assert_eq!(turn.assistant_text, "答复 1");
    assert!(turn.repository_remote.is_none());
    let mut child = header("child");
    child["parentSession"] = json!("root");
    child["origin"] = json!("subagent");
    child["delegationDepth"] = json!(1);
    write_plain(&path, child, &events);
    assert!(read_turns(&path, &Cursor::default(), ENABLED)
        .unwrap()
        .turns
        .is_empty());
    let mut historical_fork = header("ordinary-fork");
    historical_fork["parentSession"] = json!("root");
    write_plain(&path, historical_fork, &events);
    assert_eq!(
        read_turns(&path, &Cursor::default(), ENABLED)
            .unwrap()
            .turns
            .len(),
        1
    );
}

#[test]
fn last_noninterrupted_text_message_wins_without_reasoning_tools_or_synthetic_inputs() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut context = user("injected runtime context", 1030);
    context["data"]["source"] = json!({"kind":"runtime-context","form":"snapshot","sections":[]});
    let mut progress = assistant(1, 1, "中间进度", 1100);
    progress["data"]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool-call","id":"call-1","name":"read","arguments":"{}"}));
    let mut final_message = assistant(1, 2, "最终正文", 1300);
    final_message["data"]["message"]["content"] = json!([
        {"type":"reasoning","text":"private reasoning"},
        {"type":"text","text":"最终正文"},{"type":"text","text":"第二段"}]);
    let mut reasoning = assistant(1, 2, "unused", 1350);
    reasoning["data"]["message"]["content"] = json!([{"type":"reasoning","text":"not final"}]);
    let mut partial = assistant(1, 2, "cancelled prefix", 1370);
    partial["data"]["interrupted"] = json!(true);
    let events = vec![
        event("turn/start", 1000, json!({"turn":1})),
        event("step/start", 1010, json!({"turn":1,"step":1})),
        user("人工问题", 1020),
        context,
        progress,
        event(
            "tool/call",
            1120,
            json!({"turn":1,"step":1,"callId":"call-1","name":"read","arguments":"{}"}),
        ),
        event(
            "tool/result",
            1130,
            json!({"turn":1,"step":1,"message":{"id":"tool-result","role":"tool","source":{"kind":"tool","callId":"call-1"},"toolCallId":"call-1","content":[{"type":"text","text":"tool evidence"}]}}),
        ),
        event("step/end", 1150, json!({"turn":1,"step":1})),
        event("step/start", 1200, json!({"turn":1,"step":2})),
        user("追加人工约束", 1210),
        final_message,
        reasoning,
        partial,
        event("step/end", 1380, json!({"turn":1,"step":2})),
        event(
            "turn/end",
            1400,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    write_plain(&path, header("text-selection"), &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].user_text, "人工问题\n\n追加人工约束");
    assert_eq!(batch.turns[0].assistant_text, "最终正文\n第二段");
}

#[test]
fn the_last_tagged_seed_marker_owns_the_cut_and_inherited_turns_are_not_rearchived() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("fork");
    head["isSeeded"] = json!(true);
    head["parentSession"] = json!("parent");
    let mut events = turn(1, 1000);
    events.push(event("session/end-seed", 1400, json!({"inherited":true})));
    let open = turn(2, 2000);
    events.extend_from_slice(&open[..5]);
    let cut = events.len() as u64;
    events.push(event("session/end-seed", 2150, json!({"inherited":true})));
    events.push(event("step/end", 2150, json!({"turn":2,"step":1})));
    events.push(event(
        "turn/end",
        2150,
        json!({"turn":2,"reason":{"kind":"forked"}}),
    ));
    events.extend(turn(3, 3000));
    write_plain(&path, head, &events);
    let log = read_session(&path).unwrap();
    assert_eq!(log.seed_cutoff, Some(cut));
    assert_eq!(log.events.len(), events.len());
    assert_eq!(log.events[0].value["seq"], 0);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].source_id, "harness:fork:3");
}

#[test]
fn inconsistent_seed_headers_are_rejected_instead_of_becoming_owned_usage() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut seeded = header("seeded");
    seeded["isSeeded"] = json!(true);
    write_plain(&path, seeded.clone(), &turn(1, 1000));
    assert!(read_session(&path).unwrap_err().contains("继承标记"));
    write_plain(
        &path,
        seeded.clone(),
        &[event("session/end-seed", 1000, json!({}))],
    );
    assert!(read_session(&path).is_err());
    let mut events = vec![event("session/end-seed", 1000, json!({"inherited":true}))];
    events.extend(turn(1, 1100));
    write_plain(&path, header("unseeded"), &events);
    assert!(read_session(&path).is_err());
    write_plain(&path, seeded, &events);
    assert_eq!(read_session(&path).unwrap().seed_cutoff, Some(0));
    assert_eq!(
        read_turns(&path, &Cursor::default(), ENABLED)
            .unwrap()
            .turns
            .len(),
        1
    );
}

#[test]
fn enabling_mid_turn_skips_that_turn_but_replay_retains_new_open_turns() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut old = turn(1, 100);
    old.last_mut().unwrap()["time"] = json!(BASE + 1000);
    write_plain(&path, header("baseline"), &old[..6]);
    let baseline = snapshot_session(&path).unwrap();
    assert_eq!(baseline.offset, fs::metadata(&path).unwrap().len());
    append_plain(&path, &old[6..], 6);
    let skipped = read_turns(&path, &baseline, ENABLED).unwrap();
    assert!(skipped.turns.is_empty());
    let next = turn(2, 1100);
    append_plain(&path, &next[..4], 7);
    let open = read_turns(&path, &skipped.cursor, ENABLED).unwrap();
    assert!(open.turns.is_empty());
    append_plain(&path, &next[4..], 11);
    let finished = read_turns(&path, &open.cursor, ENABLED).unwrap();
    assert_eq!(finished.turns.len(), 1);
    assert_eq!(finished.turns[0].source_id, "harness:baseline:2");
    let serialized = serde_json::to_string(&finished.cursor).unwrap();
    let restored: Cursor = serde_json::from_str(&serialized).unwrap();
    assert!(read_turns(&path, &restored, ENABLED)
        .unwrap()
        .turns
        .is_empty());
    append_plain(&path, &[event("session/end-seed", 1500, json!({}))], 14);
    let replay = read_turns(&path, &restored, ENABLED).unwrap();
    assert_eq!(replay.turns.len(), 1);
    assert_eq!(replay.turns[0].source_id, finished.turns[0].source_id);
}

#[test]
fn truncation_rebuilds_the_session_and_business_identity_survives_file_rewrites() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut events = turn(1, 1000);
    events.extend(turn(2, 2000));
    write_plain(&path, header("rewritten"), &events);
    let first = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(first.turns.len(), 2);
    write_plain(&path, header("rewritten"), &turn(1, 1000));
    let shorter = read_turns(&path, &first.cursor, ENABLED).unwrap();
    assert_eq!(shorter.turns.len(), 1);
    assert_eq!(shorter.turns[0].source_id, first.turns[0].source_id);
    assert_eq!(shorter.warnings.len(), 1);
    assert!(shorter.cursor.offset < first.cursor.offset);
    let moved = fixture.path().join("same-header-different-name.jsonl");
    fs::copy(&path, &moved).unwrap();
    let replay = read_turns(&moved, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(replay.turns[0].source_id, first.turns[0].source_id);
    assert_eq!(replay.turns[0].session_path, moved.display().to_string());
}

#[cfg(unix)]
#[test]
fn replacing_a_file_with_the_same_length_still_changes_its_revision() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let replacement = fixture.path().join("replacement.jsonl");
    let bytes = plaintext(header("same-length"), &turn(1, 1000));
    fs::write(&path, &bytes).unwrap();
    let before = snapshot_session(&path).unwrap();
    fs::write(&replacement, bytes).unwrap();
    fs::rename(&replacement, &path).unwrap();
    let after = read_turns(&path, &before, ENABLED).unwrap();
    assert_eq!(after.cursor.offset, before.offset);
    assert_ne!(after.cursor.revision, before.revision);
    assert_eq!(after.turns.len(), 1);
}

fn repository(root: &Path, name: &str, file: &str) -> PathBuf {
    let path = root.join(format!("{name} 代码"));
    fs::create_dir_all(path.join(file).parent().unwrap()).unwrap();
    fs::write(path.join(file), "fixture").unwrap();
    assert!(crate::git::command()
        .args(["init", "--quiet"])
        .arg(&path)
        .output()
        .unwrap()
        .status
        .success());
    assert!(crate::git::command()
        .arg("-C")
        .arg(&path)
        .args([
            "remote",
            "add",
            "origin",
            &format!("https://github.com/test/{name}.git")
        ])
        .output()
        .unwrap()
        .status
        .success());
    path
}

#[test]
fn cwd_changes_and_tool_arguments_keep_existing_git_and_cross_repository_context() {
    let fixture = fixture();
    let front = repository(fixture.path(), "frontend", "src/App.vue");
    let back = repository(fixture.path(), "backend", "src/status.rs");
    fs::write(back.join("Dockerfile"), "FROM scratch\n").unwrap();
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("directories");
    head["cwd"] = json!(front);
    let mut events = turn(1, 1000);
    events[2] = user("前端修改 \u{0060}src/App.vue\u{0060}", 1020);
    events.insert(4, event("tool/call", 1040, json!({"turn":1,"step":1,"callId":"code","name":"read", "arguments":json!({"workdir":back,"file_path":"src/status.rs"}).to_string()})));
    events.insert(
        5,
        event("working-directory/change", 1050, json!({"cwd":back})),
    );
    events[6] = assistant(1, 1, "后端也修改 \u{0060}Dockerfile\u{0060}", 1100);
    events.extend(turn(2, 2000));
    write_plain(&path, head, &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 2);
    for turn in &batch.turns {
        assert_eq!(turn.cwd, back.display().to_string());
        assert_eq!(
            turn.repository_remote.as_deref(),
            Some("https://github.com/test/backend.git")
        );
        assert_eq!(turn.repository_name, "backend");
    }
    let repositories = batch.turns[0].repositories();
    let frontend = repositories
        .iter()
        .find(|repository| repository.name == "frontend")
        .unwrap();
    let backend = repositories
        .iter()
        .find(|repository| repository.name == "backend")
        .unwrap();
    assert!(frontend.files.contains(&"src/App.vue".into()));
    assert!(backend.files.contains(&"src/status.rs".into()));
    assert!(backend.files.contains(&"Dockerfile".into()));
}

#[test]
fn directory_only_cross_repository_turns_keep_every_visited_repository() {
    let fixture = fixture();
    let front = repository(fixture.path(), "frontend", "src/App.vue");
    let back = repository(fixture.path(), "backend", "src/status.rs");
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("directory-only");
    head["cwd"] = json!(front);
    let mut events = turn(1, 1000);
    events[2] = user("处理这项变更", 1020);
    events[4] = assistant(1, 1, "处理完成", 1100);
    events.insert(
        4,
        event("working-directory/change", 1050, json!({"cwd":back})),
    );
    write_plain(&path, head, &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    let archived = &batch.turns[0];
    assert_eq!(archived.cwd, back.display().to_string());
    assert_eq!(archived.repository_name, "backend");
    assert_eq!(archived.code_repositories.len(), 2);
    for (name, root) in [("frontend", front), ("backend", back)] {
        let repository = archived
            .code_repositories
            .iter()
            .find(|repository| repository.name == name)
            .unwrap();
        assert_eq!(repository.root, root.display().to_string());
        assert!(repository.files.is_empty());
    }
}

#[test]
fn missing_cwd_does_not_attribute_relative_paths_to_the_process_repository() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("missing-directory");
    head.as_object_mut().unwrap().remove("cwd");
    let mut events = turn(1, 1000);
    events[2] = user(
        "检查 \u{0060}README.md\u{0060} 与 \u{0060}src/lib.rs\u{0060}",
        1020,
    );
    events[4] = assistant(1, 1, "整理完成", 1100);
    write_plain(&path, head, &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    let archived = &batch.turns[0];
    assert!(archived.cwd.is_empty());
    assert!(archived.repository_remote.is_none());
    assert!(archived.code_repositories.is_empty());
    assert!(archived.repositories().is_empty());
}

#[test]
fn missing_cwd_still_resolves_explicit_absolute_directories_and_files() {
    let fixture = fixture();
    let explicit = repository(fixture.path(), "explicit", "src/main.rs");
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("absolute-directory");
    head.as_object_mut().unwrap().remove("cwd");
    let mut directory = turn(1, 1000);
    directory[2] = user(
        &format!("检查 \u{0060}{}\u{0060}", explicit.display()),
        1020,
    );
    directory[4] = assistant(1, 1, "目录检查完成", 1100);
    let mut file = turn(2, 2000);
    file[2] = user(
        &format!(
            "检查 \u{0060}{}\u{0060}",
            explicit.join("src/main.rs").display()
        ),
        2020,
    );
    file[4] = assistant(2, 1, "文件检查完成", 2100);
    directory.extend(file);
    write_plain(&path, head, &directory);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 2);
    for archived in &batch.turns {
        assert!(archived.cwd.is_empty());
        assert!(archived.repository_remote.is_none());
        assert_eq!(archived.code_repositories.len(), 1);
        assert_eq!(archived.code_repositories[0].name, "explicit");
    }
    assert!(batch.turns[0].code_repositories[0].files.is_empty());
    assert_eq!(
        batch.turns[1].code_repositories[0].files,
        vec!["src/main.rs"]
    );
}

#[test]
fn a_later_recorded_cwd_enables_normal_relative_paths_without_a_header_cwd() {
    let fixture = fixture();
    let explicit = repository(fixture.path(), "later-directory", "src/main.rs");
    let path = fixture.path().join("session.v4.jsonl");
    let mut head = header("later-directory");
    head.as_object_mut().unwrap().remove("cwd");
    let mut events = turn(1, 1000);
    events[2] = user("检查 \u{0060}src/main.rs\u{0060}", 1020);
    events.insert(
        2,
        event("working-directory/change", 1015, json!({"cwd":explicit})),
    );
    write_plain(&path, head, &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].cwd, explicit.display().to_string());
    assert_eq!(batch.turns[0].repository_name, "later-directory");
    assert_eq!(
        batch.turns[0].code_repositories[0].files,
        vec!["src/main.rs"]
    );
}

#[test]
fn the_first_compressed_frame_contains_only_the_header() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl.zstd");
    let mut rows = vec![header("header-frame")];
    rows.extend(numbered(&turn(1, 1000), 0));
    fs::write(&path, frame(&rows)).unwrap();
    assert!(read_session(&path).unwrap_err().contains("完整 JSONL"));
    fs::write(&path, frame(&[header("header-only")])).unwrap();
    let log = read_session(&path).unwrap();
    assert!(log.events.is_empty());
    assert!(read_turns(&path, &Cursor::default(), ENABLED)
        .unwrap()
        .turns
        .is_empty());
}

#[test]
fn inherited_working_directory_changes_still_supply_the_owned_turn_context() {
    let fixture = fixture();
    let path = fixture.path().join("session.v4.jsonl");
    let changed = fixture.path().join("effective-directory");
    let mut head = header("fork-directory");
    head["isSeeded"] = json!(true);
    let mut events = vec![
        event("working-directory/change", 1000, json!({"cwd":changed})),
        event("session/end-seed", 1001, json!({"inherited":true})),
    ];
    events.extend(turn(1, 1100));
    write_plain(&path, head, &events);
    let batch = read_turns(&path, &Cursor::default(), ENABLED).unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].cwd, changed.display().to_string());
}

#[test]
fn unsupported_versions_seq_gaps_and_invalid_complete_json_are_errors_with_locations() {
    let fixture = fixture();
    let path = fixture.path().join("session.v5.jsonl");
    let mut future = header("future");
    future["version"] = json!(5);
    write_plain(&path, future, &turn(1, 1000));
    assert!(read_session(&path).unwrap_err().contains("尚不支持"));
    let mut rows = vec![header("gap")];
    let mut gap = event("turn/start", 1000, json!({"turn":1}));
    gap["seq"] = json!(2);
    rows.push(gap);
    fs::write(&path, jsonl(&rows)).unwrap();
    assert!(read_session(&path).unwrap_err().contains("第 2 行"));
    let mut bytes = jsonl(&[header("bad-json")]);
    bytes.extend_from_slice(b"{invalid}\n");
    fs::write(&path, bytes).unwrap();
    assert!(read_session(&path).unwrap_err().contains("第 2 行"));
}
