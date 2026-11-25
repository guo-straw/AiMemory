use super::*;
use serde_json::json;

#[path = "harness_tests.rs"]
mod harness;
use std::{fs, io::Write};

fn at(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(1795392000 + seconds, 0)
        .unwrap()
        .to_rfc3339()
}
fn event(kind: &str, payload: Value, seconds: i64) -> Value {
    json!({"timestamp":at(seconds),"ordinal":seconds+1,"type":kind,"payload":payload})
}
fn meta(id: &str, seconds: i64) -> Value {
    event(
        "session_meta",
        json!({"id":id,"session_id":id,"thread_source":"user","source":"vscode","timestamp":at(seconds),"cwd":"/not-a-repository/usage-project","model_provider":"openai"}),
        seconds,
    )
}
fn context(model: &str, seconds: i64) -> Value {
    event(
        "turn_context",
        json!({"turn_id":format!("turn-{seconds}"),"model":model,"cwd":"/not-a-repository/usage-project"}),
        seconds,
    )
}
fn tokens(input: i64, cached: i64, output: i64) -> Value {
    json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":output/2,"total_tokens":input+output})
}
fn count(last: Value, total: Value, seconds: i64) -> Value {
    event(
        "event_msg",
        json!({"type":"token_count","info":{"last_token_usage":last,"total_token_usage":total},"rate_limits":{"limit_id":"codex","primary":{"used_percent":62,"window_minutes":10080,"resets_at":1795576480},"secondary":null}}),
        seconds,
    )
}
fn write(path: &Path, events: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = events.iter().map(|v| format!("{v}\n")).collect::<String>();
    fs::write(path, content).unwrap();
}
fn setup() -> (tempfile::TempDir, Store, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    fs::create_dir_all(home.join("sessions")).unwrap();
    let store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    (temp, store, home)
}
fn filters(home: &Path) -> Filters {
    Filters {
        source: store::source_id(home),
        harness_source: String::new(),
        client: None,
        start: 0,
        end: 4102444800000,
        project: None,
        model: None,
        role: None,
        buckets: vec![],
    }
}
fn roots(home: &Path) -> Directories {
    Directories {
        codex: home.to_string_lossy().into_owned(),
        harness: home
            .parent()
            .unwrap()
            .join("harness")
            .to_string_lossy()
            .into_owned(),
    }
}
fn overview(store: &Store, home: &Path) -> Overview {
    store
        .overview(&filters(home), &roots(home), ScanStatus::default())
        .unwrap()
}
fn scan(store: &mut Store, home: &Path) {
    assert!(store.scan(home, false, |_, _| {}).unwrap().is_empty());
}
fn price(model: &str) -> Price {
    Price {
        model: model.into(),
        provider: "openai".into(),
        tier: "".into(),
        input: Some(2.5),
        cached: Some(0.25),
        cache_write: None,
        output: Some(10.),
        source: "测试参考价".into(),
        custom: true,
    }
}

#[test]
fn tokens_prices_and_detail_use_the_same_accounting() {
    let (_temp, mut store, home) = setup();
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("model-a", 1),
            count(
                tokens(1_000_000, 800_000, 100_000),
                tokens(1_000_000, 800_000, 100_000),
                2,
            ),
        ],
    );
    scan(&mut store, &home);
    store.save_prices(&[price("model-a")]).unwrap();
    let result = overview(&store, &home);
    let s = result.summary;
    assert_eq!(
        (
            s.records,
            s.sessions,
            s.input + s.output,
            s.cached,
            s.cache_known_input
        ),
        (1, 1, 1_100_000, 800_000, 1_000_000)
    );
    assert!((s.cost - 1.70).abs() < 1e-9);
    let d = store.detail(&filters(&home), "a", 0).unwrap();
    assert!((d.cost_parts.iter().sum::<f64>() - s.cost).abs() < 1e-9);
    assert_eq!(d.records[0].cost, Some(s.cost));
    assert_eq!(d.cost_groups.len(), 1);
    assert_eq!(d.cost_groups[0].input, 1_000_000);
    assert_eq!(d.cost_groups[0].price.input, Some(2.5));
}

#[test]
fn official_prices_follow_tier_and_the_full_input_context_threshold() {
    let (_temp, mut store, home) = setup();
    write(
        &home.join("sessions/standard.jsonl"),
        &[
            meta("standard", 0),
            context("gpt-6.1-sol", 1),
            count(
                tokens(272_000, 270_000, 1_000),
                tokens(272_000, 270_000, 1_000),
                2,
            ),
            context("gpt-6.1-sol", 3),
            count(
                tokens(272_001, 270_000, 1_000),
                tokens(544_001, 540_000, 2_000),
                4,
            ),
        ],
    );
    let mut fast = context("gpt-6.1-sol", 1);
    fast["payload"]["service_tier"] = json!("priority");
    write(
        &home.join("sessions/fast.jsonl"),
        &[
            meta("fast", 0),
            fast,
            count(
                tokens(100_000, 90_000, 1_000),
                tokens(100_000, 90_000, 1_000),
                2,
            ),
        ],
    );
    write(
        &home.join("sessions/internal.jsonl"),
        &[
            meta("internal", 0),
            context("codex-auto-review", 1),
            count(tokens(1000, 800, 100), tokens(1000, 800, 100), 2),
        ],
    );
    let mut other = meta("other", 0);
    other["payload"]["model_provider"] = json!("custom");
    write(
        &home.join("sessions/other.jsonl"),
        &[
            other,
            context("gpt-6.1-sol", 1),
            count(tokens(1000, 800, 100), tokens(1000, 800, 100), 2),
        ],
    );
    scan(&mut store, &home);
    let result = overview(&store, &home);
    assert_eq!(
        (result.summary.records, result.summary.priced_records),
        (5, 4)
    );
    assert!((result.summary.cost - (0.041 + 0.077004 + 0.078 + 0.00148)).abs() < 1e-10);
    let detail = store.detail(&filters(&home), "standard", 0).unwrap();
    assert_eq!(detail.cost_groups.len(), 2);
    assert!((detail.cost_parts.iter().sum::<f64>() - detail.session.summary.cost).abs() < 1e-10);
    assert!(detail
        .cost_groups
        .iter()
        .all(|g| !g.price.custom && g.price.source.starts_with("OpenAI 官方")));
    assert_eq!(
        detail
            .cost_groups
            .iter()
            .map(|g| g.price.input.unwrap() as i64)
            .sum::<i64>(),
        6
    );
    let prices = store.prices().unwrap();
    assert_eq!(
        prices
            .iter()
            .find(|p| p.model == "gpt-6.1-sol" && p.tier == "priority")
            .unwrap()
            .input,
        Some(4.0)
    );
    assert_eq!(
        prices
            .iter()
            .find(|p| p.model == "codex-auto-review")
            .unwrap()
            .input,
        None
    );
}

#[test]
fn manual_prices_survive_reopening_and_can_return_to_official_defaults() {
    let (temp, mut store, home) = setup();
    let u = tokens(100_000, 90_000, 1_000);
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("gpt-6.1-sol", 1),
            count(u.clone(), u, 2),
        ],
    );
    scan(&mut store, &home);
    let official_cost = overview(&store, &home).summary.cost;
    assert!((official_cost - 0.039).abs() < 1e-10);
    let mut manual = price("gpt-6.1-sol");
    store.save_prices(&[manual.clone()]).unwrap();
    drop(store);
    let store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    assert!((overview(&store, &home).summary.cost - 0.0575).abs() < 1e-10);
    assert!(store.prices().unwrap()[0].custom);
    manual.custom = false;
    store.save_prices(&[manual]).unwrap();
    assert!((overview(&store, &home).summary.cost - official_cost).abs() < 1e-10);
    assert!(!store.prices().unwrap()[0].custom);
    // Older development builds saved an all-empty row for unconfigured models.
    let db = rusqlite::Connection::open(temp.path().join("usage.sqlite")).unwrap();
    db.execute(
        "INSERT INTO usage_prices VALUES('gpt-6.1-sol','openai','',NULL,NULL,NULL,NULL,'手动配置')",
        [],
    )
    .unwrap();
    assert!((overview(&store, &home).summary.cost - official_cost).abs() < 1e-10);
    assert!(!store.prices().unwrap()[0].custom);
}

#[test]
fn repeated_scans_restart_full_scan_and_archive_move_are_idempotent() {
    let (temp, mut store, home) = setup();
    let path = home.join("sessions/a.jsonl");
    let usage = tokens(1000, 800, 100);
    write(
        &path,
        &[
            meta("a", 0),
            context("m", 1),
            count(usage.clone(), usage.clone(), 2),
            count(usage.clone(), usage, 3),
        ],
    );
    scan(&mut store, &home);
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 1);
    drop(store);
    let mut store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    fs::create_dir_all(home.join("archived_sessions")).unwrap();
    fs::rename(path, home.join("archived_sessions/a.jsonl")).unwrap();
    scan(&mut store, &home);
    store.scan(&home, true, |_, _| {}).unwrap();
    assert_eq!(overview(&store, &home).summary.input, 1000);
    assert!(store.records(&filters(&home), None, None).unwrap()[0]
        .file
        .contains("archived_sessions"));
}

#[test]
fn identical_real_responses_with_advancing_totals_are_both_counted() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("m", 1),
            count(u.clone(), u.clone(), 2),
            count(u, tokens(2000, 1600, 200), 3),
        ],
    );
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 2);
}

#[test]
fn response_identity_covers_its_token_snapshot_once() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    let response = event(
        "token_usage_record",
        json!({"response_id":"resp-a","turn_id":"t","usage":u}),
        2,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("m", 1),
            response.clone(),
            count(u.clone(), u.clone(), 3),
            response,
            count(u.clone(), u, 4),
        ],
    );
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 1);
}

#[test]
fn forked_history_and_shared_session_ids_do_not_merge_threads() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("parent", 0),
            context("m", 1),
            count(u.clone(), u.clone(), 2),
        ],
    );
    let mut child = meta("child", 10);
    child["payload"]["session_id"] = json!("parent");
    child["payload"]["thread_source"] = json!("agent");
    child["payload"]["source"] = json!({"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}});
    write(
        &home.join("sessions/b.jsonl"),
        &[
            child,
            meta("parent", 0),
            context("m", 1),
            count(u.clone(), u.clone(), 2),
            context("m", 11),
            count(u, tokens(2000, 1600, 200), 12),
        ],
    );
    scan(&mut store, &home);
    let result = overview(&store, &home);
    assert_eq!(
        (
            result.summary.records,
            result.summary.sessions,
            result.summary.input
        ),
        (2, 2, 2000)
    );
    let child = store.detail(&filters(&home), "child", 0).unwrap().session;
    assert_eq!(child.parent_id.as_deref(), Some("parent"));
    assert_eq!(child.role, "agent");
}

#[test]
fn estimates_quota_updates_and_aborted_turns_keep_usage_separate() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    let estimate = count(tokens(0, 0, 0), tokens(4000, 800, 100), 3);
    let quota = event(
        "event_msg",
        json!({"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":75,"window_minutes":300,"resets_at":1795576480},"secondary":null}}),
        4,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("m", 1),
            count(u.clone(), u, 2),
            estimate,
            quota,
            event("event_msg", json!({"type":"turn_aborted"}), 5),
        ],
    );
    scan(&mut store, &home);
    let result = overview(&store, &home);
    assert_eq!((result.summary.records, result.summary.input), (1, 1000));
    assert_eq!(result.quota.unwrap().data["primary"]["window_minutes"], 300);
}

#[test]
fn cumulative_rollback_prefers_last_and_reports_unresolvable_records() {
    let (_temp, mut store, home) = setup();
    let u = tokens(100, 80, 10);
    let cumulative = event(
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":tokens(20,10,2)}}),
        4,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("m", 1),
            count(u.clone(), tokens(1000, 800, 100), 2),
            count(u, tokens(500, 400, 50), 3),
            cumulative,
        ],
    );
    scan(&mut store, &home);
    let result = overview(&store, &home);
    assert_eq!(
        (
            result.summary.records,
            result.summary.input,
            result.issue_count
        ),
        (2, 200, 1)
    );
}

#[test]
fn unfinished_tail_is_imported_once_after_newline_arrives() {
    let (_temp, mut store, home) = setup();
    let path = home.join("sessions/a.jsonl");
    write(&path, &[meta("a", 0), context("m", 1)]);
    let line = count(tokens(1000, 800, 100), tokens(1000, 800, 100), 2).to_string();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 0);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    scan(&mut store, &home);
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 1);
}

#[test]
fn missing_cache_and_missing_prices_are_not_reported_as_free_usage() {
    let (_temp, mut store, home) = setup();
    let u = json!({"input_tokens":1000,"output_tokens":100});
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("m", 1),
            count(u.clone(), u, 2),
            context("unpriced", 3),
            count(tokens(1000, 800, 100), tokens(2000, 800, 200), 4),
        ],
    );
    scan(&mut store, &home);
    store.save_prices(&[price("m")]).unwrap();
    let result = overview(&store, &home);
    assert_eq!(
        (
            result.summary.records,
            result.summary.priced_records,
            result.summary.cache_known_input
        ),
        (2, 0, 1000)
    );
    assert!(store
        .records(&filters(&home), None, None)
        .unwrap()
        .iter()
        .all(|r| r.cost.is_none()));
}

#[test]
fn cache_write_and_reasoning_remain_subsets_of_totals() {
    let (_temp, mut store, home) = setup();
    let mut u = tokens(1000, 700, 100);
    u["cache_write_input_tokens"] = json!(100);
    write(
        &home.join("sessions/a.jsonl"),
        &[meta("a", 0), context("m", 1), count(u.clone(), u, 2)],
    );
    scan(&mut store, &home);
    let mut p = price("m");
    p.cache_write = Some(3.0);
    store.save_prices(&[p]).unwrap();
    let s = overview(&store, &home).summary;
    assert_eq!(
        (s.input + s.output, s.cache_write, s.reasoning),
        (1100, 100, 50)
    );
    assert!((s.cost - 0.001975).abs() < 1e-10);
}

#[test]
fn model_project_time_and_source_filters_are_consistent() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    let mut moved = context("b", 3);
    moved["payload"]["cwd"] = json!("/not-a-repository/second-project");
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("a", 1),
            count(u.clone(), u.clone(), 2),
            moved,
            count(u.clone(), tokens(2000, 1600, 200), 4),
        ],
    );
    scan(&mut store, &home);
    store.save_prices(&[price("a")]).unwrap();
    let result = overview(&store, &home);
    assert_eq!(result.models.len(), 2);
    assert_eq!(result.projects.len(), 2);
    assert_eq!(result.summary.priced_records, 1);
    let mut f = filters(&home);
    f.model = Some("a".into());
    f.buckets = vec![Bucket {
        start: 0,
        end: 4102444800000,
        label: "all".into(),
    }];
    let a = store
        .overview(&f, &roots(&home), ScanStatus::default())
        .unwrap();
    let page = store.sessions(&f, 0, "recent").unwrap();
    let detail = store.detail(&f, "a", 0).unwrap();
    assert_eq!(a.summary.input, 1000);
    assert_eq!(a.summary.input, a.trend[0].summary.input);
    assert_eq!(a.summary.input, page.items[0].summary.input);
    assert_eq!(detail.records.len(), 1);
    let other = home.parent().unwrap().join("other-codex");
    write(
        &other.join("sessions/a.jsonl"),
        &[meta("a", 0), context("a", 1), count(u.clone(), u, 2)],
    );
    scan(&mut store, &other);
    assert_eq!(overview(&store, &home).summary.records, 2);
    assert_eq!(overview(&store, &other).summary.records, 1);
}

#[test]
fn session_pagination_does_not_limit_record_export() {
    let (_temp, mut store, home) = setup();
    for n in 0..23 {
        let id = format!("session-{n}");
        let u = tokens(1000, 800, 100);
        write(
            &home.join(format!("sessions/{id}.jsonl")),
            &[meta(&id, 0), context("m", 1), count(u.clone(), u, 2)],
        );
    }
    scan(&mut store, &home);
    let f = filters(&home);
    assert_eq!(store.sessions(&f, 0, "tokens").unwrap().items.len(), 20);
    assert_eq!(store.sessions(&f, 1, "recent").unwrap().items.len(), 3);
    assert_eq!(store.records(&f, None, None).unwrap().len(), 23);
}

#[test]
#[ignore = "reads the user's local Codex logs into a temporary database"]
fn local_log_replay() {
    let temp = tempfile::tempdir().unwrap();
    let home = crate::codex::default_codex_directory();
    let mut store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    let started = std::time::Instant::now();
    let errors = store.scan(&home, false, |_, _| {}).unwrap();
    let initial_ms = started.elapsed().as_millis();
    let before = overview(&store, &home);
    let started = std::time::Instant::now();
    store.scan(&home, false, |_, _| {}).unwrap();
    let repeat_ms = started.elapsed().as_millis();
    let after = overview(&store, &home);
    assert_eq!(before.summary.input, after.summary.input);
    assert_eq!(before.summary.records, after.summary.records);
    println!(
        "{}",
        json!({"initialMs":initial_ms,"repeatMs":repeat_ms,"records":before.summary.records,"sessions":before.summary.sessions,"tokens":before.summary.input+before.summary.output,"pending":before.issue_count,"errors":errors})
    );
    assert!(errors.is_empty());
}

#[test]
fn continuation_restores_its_cumulative_baseline() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    write(
        &home.join("sessions/a.jsonl"),
        &[meta("a", 0), context("m", 1), count(u.clone(), u, 2)],
    );
    let mut continuation = meta("a", 10);
    continuation["payload"]["history_base_id"] = json!("a");
    let next = event(
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":tokens(2000,1600,200)}}),
        12,
    );
    write(
        &home.join("sessions/b.jsonl"),
        &[continuation, context("m", 11), next],
    );
    scan(&mut store, &home);
    let s = overview(&store, &home).summary;
    assert_eq!((s.records, s.input, s.sessions), (2, 2000, 1));
}

#[test]
fn applied_thread_settings_keep_provider_and_tier_through_turn_context() {
    let (_temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    let settings = event(
        "event_msg",
        json!({"type":"thread_settings_applied","thread_settings":{"model":"m","model_provider_id":"custom","service_tier":"default"}}),
        1,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            settings,
            context("m", 2),
            count(u.clone(), u, 3),
        ],
    );
    scan(&mut store, &home);
    let row = store
        .records(&filters(&home), None, None)
        .unwrap()
        .remove(0);
    assert_eq!(row.provider, "custom");
    assert_eq!(row.tier, "default");
}

#[test]
fn inconsistent_cumulative_cache_changes_are_pending_instead_of_negative() {
    let (_temp, mut store, home) = setup();
    let a = event(
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":tokens(100,80,10)}}),
        2,
    );
    let b = event(
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":tokens(110,100,11)}}),
        3,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[meta("a", 0), context("m", 1), a, b],
    );
    scan(&mut store, &home);
    let result = overview(&store, &home);
    assert_eq!(result.summary.records, 1);
    assert_eq!(result.issue_count, 1);
}

#[test]
fn export_contains_all_filtered_records_and_escapes_csv() {
    let (temp, mut store, home) = setup();
    let u = tokens(1000, 800, 100);
    write(
        &home.join("sessions/a.jsonl"),
        &[
            meta("a", 0),
            context("model,\"quoted\"", 1),
            count(u.clone(), u, 2),
        ],
    );
    scan(&mut store, &home);
    let path = temp.path().join("export.csv");
    write_csv(store.records(&filters(&home), None, None).unwrap(), &path).unwrap();
    let csv = fs::read_to_string(path).unwrap();
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("\"model,\"\"quoted\"\"\""));
    assert_eq!(csv.lines().count(), 2);
}

#[tokio::test]
#[ignore = "local browser acceptance bridge backed by the production usage store"]
async fn browser_bridge() {
    use axum::{
        extract::{Path as RoutePath, State},
        routing::post,
        Router,
    };
    use std::sync::Arc;
    struct Bridge {
        store: Store,
        directories: Directories,
        source_views: PathBuf,
        stop: Arc<tokio::sync::Notify>,
    }
    async fn invoke(
        RoutePath(command): RoutePath<String>,
        State(state): State<Arc<Mutex<Bridge>>>,
        body: String,
    ) -> ([(axum::http::header::HeaderName, &'static str); 1], String) {
        let result = (|| -> Result<Value, String> {
            let args: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            let mut bridge = state.lock().map_err(|e| e.to_string())?;
            let mut f: Filters = serde_json::from_value(args.get("filters").cloned().unwrap_or(
                json!({"start":0,"end":4102444800000i64,"project":null,"model":null,"role":null}),
            ))
            .map_err(|e| e.to_string())?;
            f.source = store::source_id(Path::new(&bridge.directories.codex));
            f.harness_source = store::harness_source_id(Path::new(&bridge.directories.harness));
            f.validate()?;
            let page = args["page"].as_u64().unwrap_or(0) as u32;
            let value = match command.as_str() {
                "usage_overview" => serde_json::to_value(bridge.store.overview(
                    &f,
                    &bridge.directories,
                    ScanStatus {
                        last_synced_at: bridge.store.last_synced()?,
                        ..Default::default()
                    },
                )?)
                .unwrap(),
                "usage_sessions" => serde_json::to_value(bridge.store.sessions(
                    &f,
                    page,
                    args["sort"].as_str().unwrap_or("recent"),
                )?)
                .unwrap(),
                "usage_detail" => serde_json::to_value(bridge.store.detail(
                    &f,
                    args["threadId"].as_str().ok_or("缺少线程")?,
                    page,
                )?)
                .unwrap(),
                "usage_scan" => {
                    let directories = bridge.directories.clone();
                    let errors = bridge.store.scan_sources(
                        &directories,
                        args["full"].as_bool().unwrap_or(false),
                        |_, _| {},
                    )?;
                    serde_json::to_value(ScanStatus {
                        last_synced_at: bridge.store.last_synced()?,
                        errors,
                        ..Default::default()
                    })
                    .unwrap()
                }
                "usage_preferences_save" => {
                    let p: Preferences = serde_json::from_value(args["preferences"].clone())
                        .map_err(|e| e.to_string())?;
                    bridge.store.save_preferences(&p)?;
                    serde_json::to_value(p).unwrap()
                }
                "usage_prices_get" => serde_json::to_value(bridge.store.prices()?).unwrap(),
                "usage_prices_save" => {
                    let prices: Vec<Price> = serde_json::from_value(args["prices"].clone())
                        .map_err(|e| e.to_string())?;
                    for p in &prices {
                        p.validate()?;
                    }
                    bridge.store.save_prices(&prices)?;
                    Value::Null
                }
                "usage_issues" => {
                    let client: Option<String> =
                        serde_json::from_value(args.get("client").cloned().unwrap_or(Value::Null))
                            .map_err(|e| e.to_string())?;
                    validate_client(client.as_deref())?;
                    serde_json::to_value(bridge.store.issues(
                        &f.source,
                        &f.harness_source,
                        client.as_deref(),
                    )?)
                    .unwrap()
                }
                "usage_open_source" => {
                    bridge.store.source_view(
                        args["recordId"].as_str().ok_or("缺少来源")?,
                        &bridge.source_views,
                    )?;
                    Value::Null
                }
                "usage_export" => {
                    let records = bridge.store.records(&f, None, None)?;
                    super::write_csv(
                        records,
                        Path::new(args["path"].as_str().ok_or("缺少导出路径")?),
                    )?;
                    Value::Null
                }
                "stop" => {
                    bridge.stop.notify_one();
                    Value::Null
                }
                _ => return Err(format!("未知验收命令 {command}")),
            };
            Ok(value)
        })();
        let body = match result {
            Ok(data) => json!({"data":data}),
            Err(error) => json!({"error":error}),
        };
        (
            [(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
            body.to_string(),
        )
    }
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("dev-data/usage-acceptance");
    fs::create_dir_all(&directory).unwrap();
    let directories = Directories {
        codex: crate::codex::default_codex_directory()
            .to_string_lossy()
            .into_owned(),
        harness: std::env::var_os("AIMEMORY_HARNESS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(crate::harness::default_harness_directory)
            .to_string_lossy()
            .into_owned(),
    };
    let mut store = Store::open(&directory.join("usage.sqlite")).unwrap();
    let errors = store.scan_sources(&directories, true, |_, _| {}).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    store
        .save_preferences(&Preferences {
            auto_scan: false,
            interval_seconds: 60,
        })
        .unwrap();
    let stop = Arc::new(tokio::sync::Notify::new());
    let state = Arc::new(Mutex::new(Bridge {
        store,
        directories,
        source_views: directory.join("usage-sources"),
        stop: stop.clone(),
    }));
    let app = Router::new()
        .route("/invoke/{command}", post(invoke))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:5199")
        .await
        .unwrap();
    println!("USAGE_BROWSER_BRIDGE_READY http://127.0.0.1:5199");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { stop.notified().await })
        .await
        .unwrap();
}
