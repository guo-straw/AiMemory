use super::*;
use crate::{
    model_api::{ApiUsage, ChatExtraction, ExtractedMemory},
    storage::Vault,
};
use serde_json::{json, Value};
use std::io::Write;

const BASE: i64 = 1_795_230_000_000;
const ENABLED: &str = "2026-11-21T03:00:00.000Z";
const SESSION: &str = "same-session";
const REMOTE: &str = "https://github.com/fixture/archive.git";
const USER: &str = "归档必须保留来源隔离。";
const ASSISTANT: &str = "已保留两个来源的独立游标。";

struct Fixture {
    directory: tempfile::TempDir,
    config: AppConfig,
    codex: PathBuf,
    harness: PathBuf,
}

impl Fixture {
    fn new(sources: &[&str]) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root.canonicalize().unwrap()).unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        for arguments in [
            &["init", "--quiet"][..],
            &["remote", "add", "origin", REMOTE][..],
        ] {
            let output = crate::git::command()
                .arg("-C")
                .arg(&project)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let service = ServiceConfig {
            provider: None,
            base_url: "https://fixture.invalid/v1".into(),
            model: "fixture".into(),
            has_api_key: false,
        };
        let config = AppConfig {
            chat: service.clone(),
            embedding: service,
            codex_directory: directory.path().join("codex").display().to_string(),
            harness_directory: directory.path().join("harness").display().to_string(),
            archive_sources: sources.iter().map(|source| (*source).into()).collect(),
            archive_enabled: true,
            archive_started_at: None,
            run_in_background: false,
        };
        let codex = Path::new(&config.codex_directory).join("sessions/rollout-fixture.jsonl");
        let harness = Path::new(&config.harness_directory)
            .join("sessions/project/same-session/session.v4.jsonl.zstd");
        fs::create_dir_all(codex.parent().unwrap()).unwrap();
        fs::create_dir_all(harness.parent().unwrap()).unwrap();
        append_rows(
            &codex,
            &[json!({
                "timestamp": time(-60_000), "type": "session_meta",
                "payload": {"id": SESSION, "thread_source": "user", "cwd": project,
                    "timestamp": time(-60_000), "git": {"repository_url": REMOTE}}
            })],
            false,
        );
        append_rows(
            &harness,
            &[json!({
                "type": "session", "version": 4, "id": SESSION, "createdAt": BASE - 60_000,
                "cwd": project, "isSeeded": false, "delegationDepth": 0
            })],
            true,
        );
        Self {
            directory,
            config,
            codex,
            harness,
        }
    }

    fn workspace(&self) -> (Vault, Pipeline) {
        let vault = Vault::open(self.directory.path()).unwrap();
        let pipeline = Pipeline::open(self.directory.path()).unwrap();
        (vault, pipeline)
    }

    fn append_codex_turn(&self, turn: u64) {
        let id = turn.to_string();
        let start = turn as i64 * 1_000;
        append_rows(
            &self.codex,
            &[
                json!({"timestamp": time(start), "type": "event_msg",
                "payload": {"type": "task_started", "turn_id": id}}),
                json!({"timestamp": time(start + 100), "type": "event_msg",
                "payload": {"type": "user_message", "message": USER}}),
                json!({"timestamp": time(start + 300), "type": "event_msg",
                "payload": {"type": "task_complete", "turn_id": id,
                    "last_agent_message": ASSISTANT}}),
            ],
            false,
        );
    }

    fn append_harness_turn(&self, turn: u64) {
        let start = BASE + turn as i64 * 1_000;
        let seq = (turn - 1) * 4;
        append_rows(
            &self.harness,
            &[
                json!({"type": "turn/start", "seq": seq, "time": start, "data": {"turn": turn}}),
                json!({"type": "user/message", "seq": seq + 1, "time": start + 100,
                "data": {"source": {"kind": "user"}, "content": [{"type": "text", "text": USER}]}}),
                json!({"type": "assistant/message", "seq": seq + 2, "time": start + 200,
                "data": {"turn": turn, "message": {"content": [{"type": "text", "text": ASSISTANT}]}}}),
                json!({"type": "turn/end", "seq": seq + 3, "time": start + 300,
                "data": {"turn": turn, "reason": {"kind": "completed"}}}),
            ],
            true,
        );
    }

    fn completed_turns(&self) -> (codex::RolloutBatch, harness::Batch) {
        self.append_codex_turn(1);
        self.append_harness_turn(1);
        let codex = codex::read_rollout(&self.codex, &RolloutCursor::default(), ENABLED).unwrap();
        let harness =
            harness::read_turns(&self.harness, &harness::Cursor::default(), ENABLED).unwrap();
        assert!(codex.warnings.is_empty());
        assert!(harness.warnings.is_empty());
        assert_eq!(codex.turns.len(), 1);
        assert_eq!(harness.turns.len(), 1);
        (codex, harness)
    }
}

fn time(offset: i64) -> String {
    chrono::DateTime::from_timestamp_millis(BASE + offset)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn append_rows(path: &Path, rows: &[Value], compressed: bool) {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).unwrap();
        bytes.push(b'\n');
    }
    if compressed {
        bytes = zstd::stream::encode_all(bytes.as_slice(), 1).unwrap();
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
}

#[test]
fn activating_harness_keeps_codex_cursor_epoch_and_unread_turns() {
    let fixture = Fixture::new(&["codex"]);
    let (_vault, mut pipeline) = fixture.workspace();
    fixture.append_codex_turn(1);
    let mut disabled = fixture.config.clone();
    disabled.archive_enabled = false;
    let baselines = collect_activations(&disabled, &fixture.config).unwrap();
    assert!(matches!(
        baselines.as_slice(),
        [SourceBaseline::Codex(_, _)]
    ));
    baselines[0].apply(&mut pipeline, ENABLED).unwrap();
    let codex_before = serde_json::to_value(pipeline.cursors().unwrap()).unwrap();
    fixture.append_codex_turn(2);
    fixture.append_harness_turn(1);

    let mut both = fixture.config.clone();
    both.archive_sources.push("harness".into());
    let baselines = collect_activations(&fixture.config, &both).unwrap();
    assert!(matches!(
        baselines.as_slice(),
        [SourceBaseline::Harness(_, _)]
    ));
    let harness_epoch = time(2_500);
    baselines[0].apply(&mut pipeline, &harness_epoch).unwrap();
    drop(pipeline);
    let pipeline = Pipeline::open(fixture.directory.path()).unwrap();

    assert_eq!(
        serde_json::to_value(pipeline.cursors().unwrap()).unwrap(),
        codex_before
    );
    assert_eq!(
        pipeline.meta("enabled_at").unwrap().as_deref(),
        Some(ENABLED)
    );
    assert_eq!(
        pipeline.meta("codex_directory").unwrap().as_deref(),
        Some(fixture.config.codex_directory.as_str())
    );
    assert_eq!(
        pipeline.meta("harness_enabled_at").unwrap().as_deref(),
        Some(harness_epoch.as_str())
    );
    let cursors = pipeline.harness_cursors().unwrap();
    assert_eq!(cursors.len(), 1);
    assert_eq!(
        cursors[&fixture.harness].offset,
        fs::metadata(&fixture.harness).unwrap().len()
    );
    assert!(!cursors[&fixture.harness].revision.is_empty());
    let batch = codex::read_rollout(
        &fixture.codex,
        &pipeline.cursors().unwrap()[&fixture.codex],
        ENABLED,
    )
    .unwrap();
    assert_eq!(batch.turns.len(), 1);
    assert_eq!(batch.turns[0].turn_id, "2");
    assert!(collect_activations(&both, &both).unwrap().is_empty());
}

#[test]
fn reenabling_sources_replaces_only_the_newly_enabled_baseline() {
    let fixture = Fixture::new(&["codex", "harness"]);
    let (_vault, mut pipeline) = fixture.workspace();
    fixture.completed_turns();
    let mut disabled = fixture.config.clone();
    disabled.archive_enabled = false;
    let baselines = collect_activations(&disabled, &fixture.config).unwrap();
    assert_eq!(baselines.len(), 2);
    for baseline in baselines {
        baseline.apply(&mut pipeline, ENABLED).unwrap();
    }
    let codex_before = serde_json::to_value(pipeline.cursors().unwrap()).unwrap();
    let harness_before = pipeline.harness_cursors().unwrap()[&fixture.harness].offset;

    let mut codex_only = fixture.config.clone();
    codex_only.archive_sources = vec!["codex".into()];
    assert!(collect_activations(&fixture.config, &codex_only)
        .unwrap()
        .is_empty());
    fixture.append_codex_turn(2);
    fixture.append_harness_turn(2);
    let baselines = collect_activations(&codex_only, &fixture.config).unwrap();
    assert!(matches!(
        baselines.as_slice(),
        [SourceBaseline::Harness(_, _)]
    ));
    let harness_epoch = time(2_500);
    baselines[0].apply(&mut pipeline, &harness_epoch).unwrap();
    assert_eq!(
        serde_json::to_value(pipeline.cursors().unwrap()).unwrap(),
        codex_before
    );
    assert_eq!(
        pipeline.meta("enabled_at").unwrap().as_deref(),
        Some(ENABLED)
    );
    assert!(pipeline.harness_cursors().unwrap()[&fixture.harness].offset > harness_before);
    let harness_before = serde_json::to_value(pipeline.harness_cursors().unwrap()).unwrap();

    assert!(collect_activations(&fixture.config, &disabled)
        .unwrap()
        .is_empty());
    fixture.append_codex_turn(3);
    fixture.append_harness_turn(3);
    let baselines = collect_activations(&disabled, &codex_only).unwrap();
    assert!(matches!(
        baselines.as_slice(),
        [SourceBaseline::Codex(_, _)]
    ));
    let codex_epoch = time(3_500);
    baselines[0].apply(&mut pipeline, &codex_epoch).unwrap();
    assert_eq!(
        pipeline.meta("enabled_at").unwrap().as_deref(),
        Some(codex_epoch.as_str())
    );
    assert_ne!(
        serde_json::to_value(pipeline.cursors().unwrap()).unwrap(),
        codex_before
    );
    assert_eq!(
        serde_json::to_value(pipeline.harness_cursors().unwrap()).unwrap(),
        harness_before
    );
    assert_eq!(
        pipeline.meta("harness_enabled_at").unwrap().as_deref(),
        Some(harness_epoch.as_str())
    );
}

#[test]
fn matching_session_and_turn_ids_stay_namespaced_and_reingestion_is_idempotent() {
    let fixture = Fixture::new(&["codex", "harness"]);
    let (_vault, mut pipeline) = fixture.workspace();
    let (codex, harness) = fixture.completed_turns();
    let codex_turn = &codex.turns[0];
    let harness_turn = &harness.turns[0];
    assert_eq!(codex_turn.source_id, format!("codex:{SESSION}:1"));
    assert_eq!(harness_turn.source_id, format!("harness:{SESSION}:1"));
    assert_eq!(codex_turn.session_id, SESSION);
    assert_eq!(harness_turn.session_id, format!("harness:{SESSION}"));
    assert_eq!(codex_turn.turn_id, harness_turn.turn_id);
    for _ in 0..2 {
        pipeline
            .ingest(&fixture.codex, &codex.cursor, &codex.turns)
            .unwrap();
        pipeline
            .ingest_harness(&fixture.harness, &harness.cursor, &harness.turns)
            .unwrap();
    }
    pipeline.mark_extracting(&codex_turn.source_id).unwrap();
    pipeline
        .turn_failed(&harness_turn.source_id, "fixture failure")
        .unwrap();
    pipeline
        .ingest(&fixture.codex, &codex.cursor, &codex.turns)
        .unwrap();
    pipeline
        .ingest_harness(&fixture.harness, &harness.cursor, &harness.turns)
        .unwrap();
    drop(pipeline);
    let pipeline = Pipeline::open(fixture.directory.path()).unwrap();
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.recent_turns.len(), 2);
    let codex_status = status
        .recent_turns
        .iter()
        .find(|turn| turn.source_id == codex_turn.source_id)
        .unwrap();
    assert_eq!(codex_status.client, "codex");
    assert_eq!(codex_status.session_id, SESSION);
    assert_eq!(codex_status.status, "extracting");
    let harness_status = status
        .recent_turns
        .iter()
        .find(|turn| turn.source_id == harness_turn.source_id)
        .unwrap();
    assert_eq!(harness_status.client, "harness");
    assert_eq!(harness_status.session_id, format!("harness:{SESSION}"));
    assert_eq!(harness_status.status, "failed");
    assert_eq!(harness_status.error.as_deref(), Some("fixture failure"));
    assert_eq!(pipeline.cursors().unwrap().len(), 1);
    assert_eq!(pipeline.harness_cursors().unwrap().len(), 1);
}

#[test]
fn next_turn_for_sources_never_dispatches_a_disabled_source() {
    for enabled in ["codex", "harness"] {
        let fixture = Fixture::new(&[enabled]);
        let (_vault, mut pipeline) = fixture.workspace();
        let (codex, harness) = fixture.completed_turns();
        pipeline
            .ingest(&fixture.codex, &codex.cursor, &codex.turns)
            .unwrap();
        pipeline
            .ingest_harness(&fixture.harness, &harness.cursor, &harness.turns)
            .unwrap();
        assert!(pipeline.next_turn_for_sources(&[]).unwrap().is_none());
        let selected = pipeline
            .next_turn_for_sources(&fixture.config.archive_sources)
            .unwrap()
            .unwrap();
        let expected = if enabled == "codex" {
            &codex.turns[0]
        } else {
            &harness.turns[0]
        };
        assert_eq!(selected.source_id, expected.source_id);
        pipeline.mark_extracting(&selected.source_id).unwrap();
        assert!(pipeline
            .next_turn_for_sources(&fixture.config.archive_sources)
            .unwrap()
            .is_none());
        assert!(pipeline.next_turn().unwrap().is_some());
    }
}

#[test]
fn harness_only_started_at_does_not_depend_on_a_codex_epoch() {
    let fixture = Fixture::new(&["harness"]);
    let (_vault, mut pipeline) = fixture.workspace();
    assert_eq!(started_at(&pipeline, &fixture.config).unwrap(), None);
    let harness_epoch = time(500);
    pipeline
        .baseline_harness(&fixture.config.harness_directory, &harness_epoch, &[])
        .unwrap();
    assert_eq!(pipeline.meta("enabled_at").unwrap(), None);
    assert_eq!(
        started_at(&pipeline, &fixture.config).unwrap().as_deref(),
        Some(harness_epoch.as_str())
    );
    pipeline
        .baseline(&fixture.config.codex_directory, ENABLED, &[])
        .unwrap();
    assert_eq!(
        started_at(&pipeline, &fixture.config).unwrap().as_deref(),
        Some(harness_epoch.as_str())
    );
    let mut both = fixture.config.clone();
    both.archive_sources.push("codex".into());
    assert_eq!(
        started_at(&pipeline, &both).unwrap().as_deref(),
        Some(ENABLED)
    );
}

#[test]
fn materialized_harness_memory_keeps_its_label_evidence_and_session_link() {
    let fixture = Fixture::new(&["harness"]);
    let (mut vault, mut pipeline) = fixture.workspace();
    fixture.append_harness_turn(1);
    let batch =
        harness::read_turns(&fixture.harness, &harness::Cursor::default(), ENABLED).unwrap();
    let turn = &batch.turns[0];
    pipeline
        .ingest_harness(&fixture.harness, &batch.cursor, &batch.turns)
        .unwrap();
    pipeline.mark_extracting(&turn.source_id).unwrap();
    let extraction = ChatExtraction {
        memories: vec![ExtractedMemory {
            title: "归档来源隔离".into(),
            kind: "实现约束".into(),
            summary: USER.into(),
            details: ASSISTANT.into(),
            evidence: USER.into(),
            ..Default::default()
        }],
        usage: ApiUsage {
            input_tokens: 3,
            output_tokens: 2,
            reported: true,
        },
        latency_ms: 1,
        input_truncated: false,
    };
    pipeline
        .extraction_complete(&turn.source_id, &extraction)
        .unwrap();
    assert!(pipeline.materialize(&mut vault).unwrap());
    drop(vault);
    let mut vault = Vault::open(fixture.directory.path()).unwrap();
    let memories = vault
        .memories_for_session(&format!("harness:{SESSION}"))
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert!(vault.memories_for_session(SESSION).unwrap().is_empty());
    let memory = vault.read(&memories[0].id).unwrap();
    assert_eq!(
        memory.source.as_deref(),
        Some(format!("DeepSeek Harness · 会话 {SESSION} · 轮次 1").as_str())
    );
    assert_eq!(memory.evidence_sources.len(), 1);
    let evidence = &memory.evidence_sources[0];
    assert_eq!(evidence.source_id, turn.source_id);
    assert_eq!(evidence.session_id, turn.session_id);
    assert_eq!(evidence.turn_id, "1");
    assert_eq!(evidence.message_role.as_deref(), Some("user"));
    assert_eq!(evidence.status, "located");
    assert_eq!(evidence.quote.as_deref(), Some(USER));
    assert!(evidence.start_line.is_some());
    assert!(evidence.end_line >= evidence.start_line);

    pipeline
        .ingest_harness(&fixture.harness, &batch.cursor, &batch.turns)
        .unwrap();
    assert!(!pipeline.materialize(&mut vault).unwrap());
    let status = pipeline.status(true, None, false, None).unwrap();
    assert_eq!(status.written_memories, 1);
    assert_eq!(status.memory_writes, 1);
    assert_eq!(status.recent_turns[0].client, "harness");
    assert_eq!(status.recent_turns[0].status, "saved");
}
