use super::*;

#[test]
fn old_filters_default_to_all_clients_and_ignore_internal_sources() {
    let old = json!({
        "start": 0, "end": 4102444800000i64,
        "project": null, "model": null, "role": null,
        "source": "untrusted", "harnessSource": "untrusted"
    });
    let f: Filters = serde_json::from_value(old.clone()).unwrap();
    assert!(f.client.is_none());
    assert!(f.source.is_empty());
    assert!(f.harness_source.is_empty());
    f.validate().unwrap();
    for client in [Value::Null, json!("codex"), json!("harness")] {
        let mut value = old.clone();
        value["client"] = client;
        serde_json::from_value::<Filters>(value)
            .unwrap()
            .validate()
            .unwrap();
    }
    let mut invalid = old;
    invalid["client"] = json!("other");
    assert!(serde_json::from_value::<Filters>(invalid)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn legacy_database_adds_codex_client_without_resetting_usage_or_settings() {
    let (temp, mut store, home) = setup();
    let path = home.join("sessions/legacy.jsonl");
    let u = tokens(1000, 800, 100);
    write(
        &path,
        &[meta("legacy", 0), context("m", 1), count(u.clone(), u, 2)],
    );
    scan(&mut store, &home);
    store.save_prices(&[price("m")]).unwrap();
    store
        .save_preferences(&Preferences {
            auto_scan: false,
            interval_seconds: 30,
        })
        .unwrap();
    let before = store
        .records(&filters(&home), None, None)
        .unwrap()
        .remove(0);
    let before_cost = before.cost.unwrap();
    let last_synced = store.last_synced().unwrap();
    drop(store);
    let database = temp.path().join("usage.sqlite");
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("ALTER TABLE usage_records DROP COLUMN client")
        .unwrap();
    let cursor: String = db
        .query_row("SELECT cursor FROM usage_files", [], |r| r.get(0))
        .unwrap();
    drop(db);

    let mut store = Store::open(&database).unwrap();
    assert_eq!(store.last_synced().unwrap(), last_synced);
    assert!(!store.preferences().unwrap().auto_scan);
    assert_eq!(store.preferences().unwrap().interval_seconds, 30);
    let after = store
        .records(&filters(&home), None, None)
        .unwrap()
        .remove(0);
    assert_eq!(after.id, before.id);
    assert_eq!(after.client, "codex");
    assert_eq!(after.cost, Some(before_cost));
    assert_eq!(
        store
            .detail(&filters(&home), "legacy", 0)
            .unwrap()
            .session
            .client,
        "codex"
    );
    let db = rusqlite::Connection::open(&database).unwrap();
    assert_eq!(
        db.query_row("SELECT cursor FROM usage_files", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        cursor
    );
    drop(db);
    scan(&mut store, &home);
    assert_eq!(overview(&store, &home).summary.records, 1);
    assert_eq!(
        store.source_path(&before.id).unwrap(),
        path.to_string_lossy()
    );
    let views = temp.path().join("unused-codex-views");
    assert_eq!(
        store.source_view(&before.id, &views).unwrap(),
        path.to_string_lossy()
    );
    assert!(!views.exists());
}

#[test]
fn codex_partition_and_ids_stay_unchanged() {
    let (_temp, mut store, home) = setup();
    fs::create_dir_all(home.join("archived_sessions")).unwrap();
    let source = store::source_id(&home);
    assert_eq!(
        source,
        blake3::hash(home.canonicalize().unwrap().to_string_lossy().as_bytes())
            .to_hex()
            .to_string()
    );
    assert_eq!(store::source_id(&home.join("sessions")), source);
    assert_eq!(store::source_id(&home.join("archived_sessions")), source);
    let response = event(
        "token_usage_record",
        json!({"response_id":"r","usage":tokens(1000,800,100)}),
        2,
    );
    write(
        &home.join("sessions/a.jsonl"),
        &[meta("a", 0), context("m", 1), response],
    );
    scan(&mut store, &home);
    let record = store
        .records(&filters(&home), None, None)
        .unwrap()
        .remove(0);
    assert_eq!(record.id, format!("{source}:a:response:r"));
    assert_eq!(record.thread_id, "a");
    assert_eq!(record.client, "codex");
}

fn harness_event(seq: u64, kind: &str, data: Value) -> Value {
    let mut event =
        json!({"type":kind,"seq":seq,"time":1795392000000i64 + seq as i64 * 1000,"data":data});
    if matches!(kind, "assistant/message" | "user/message") {
        event["surfaceOp"] = json!("append");
    }
    event
}

fn harness_usage(input: i64, cached: i64, write: i64, output: i64) -> Value {
    json!({"inputTokens":input,"cacheReadTokens":cached,"cacheWriteTokens":write,"outputTokens":output,"totalTokens":input+cached+write+output})
}

fn harness_header(id: &str) -> Value {
    let cwd = std::env::temp_dir()
        .join("knowtrace-test-non-repository")
        .join("harness-project");
    json!({"type":"session","version":4,"id":id,"createdAt":1795392000000i64,"cwd":cwd.to_string_lossy(),"isSeeded":false,"delegationDepth":0})
}

fn harness_request(seq: u64, provider: &str, model: &str) -> Value {
    harness_event(
        seq,
        "request/header",
        json!({"header":{"config":{"provider":provider,"model":model}},"reason":"initial"}),
    )
}

fn harness_message(seq: u64, turn: u64, step: u64, usage: Value) -> Value {
    harness_event(
        seq,
        "assistant/message",
        json!({
            "turn":turn,"step":step,"message":{
                "id":format!("message-{seq}"),"role":"assistant",
                "source":{"kind":"model","provider":"deepseek","model":"deepseek-v4"},"content":[{"type":"text","text":"done"}]
            },"usage":usage,"stream":[]
        }),
    )
}

fn harness_attempt(seq: u64, turn: u64, step: u64, usage: Value) -> Value {
    harness_event(
        seq,
        "assistant/attempt",
        json!({"turn":turn,"step":step,"stream":[
            {"type":"chunk","time":1795392000000i64 + seq as i64 * 1000,"chunk":{"type":"usage","usage":usage}}
        ]}),
    )
}

fn harness_frame(value: &Value) -> Vec<u8> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    writeln!(encoder, "{value}").unwrap();
    encoder.finish().unwrap()
}

fn write_harness(root: &Path, header: Value, events: &[Value]) -> PathBuf {
    let path = root
        .join("sessions/project")
        .join(header["id"].as_str().unwrap())
        .join("session.v4.jsonl.zstd");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut content = harness_frame(&header);
    for event in events {
        content.extend(harness_frame(event));
    }
    fs::write(&path, content).unwrap();
    path.canonicalize().unwrap()
}

fn append_harness(path: &Path, events: &[Value]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    for event in events {
        file.write_all(&harness_frame(event)).unwrap();
    }
}

fn harness_filters(directories: &Directories, client: Option<&str>) -> Filters {
    let mut f = filters(Path::new(&directories.codex));
    f.harness_source = store::harness_source_id(Path::new(&directories.harness));
    f.client = client.map(str::to_string);
    f
}

fn scan_sources(store: &mut Store, directories: &Directories) {
    let errors = store.scan_sources(directories, false, |_, _| {}).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn harness_disjoint_cache_buckets_match_the_real_native_sample() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let root = Path::new(&directories.harness);
    let sample =
        json!({"inputTokens":3,"outputTokens":522,"cacheWriteTokens":19105,"totalTokens":19630});
    let path = write_harness(
        root,
        harness_header("sample"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, sample),
        ],
    );
    let before = fs::read(&path).unwrap();
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let result = store
        .overview(&f, &directories, ScanStatus::default())
        .unwrap();
    assert_eq!(
        (
            result.summary.input,
            result.summary.output,
            result.summary.cache_write
        ),
        (19108, 522, 19105)
    );
    assert_eq!(result.summary.input + result.summary.output, 19630);
    assert_eq!(result.summary.priced_records, 0);
    assert_eq!(result.summary.cache_known_input, 0);
    assert!(result.quota.is_none());
    assert_eq!(result.directory, directories.codex);
    assert_eq!(result.directories.harness, directories.harness);
    let record = store.records(&f, None, None).unwrap().remove(0);
    assert_eq!(record.thread_id, "harness:sample");
    assert_eq!(record.client, "harness");
    assert_eq!(record.tokens.cached, None);
    assert_eq!(record.tokens.reasoning, None);
    assert_eq!(record.cost, None);
    assert_eq!(record.line, 3);
    assert_eq!(
        store.source_path(&record.id).unwrap(),
        path.to_string_lossy()
    );
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(
        store::harness_source_id(root),
        format!("harness:{}", root.canonicalize().unwrap().display())
    );
    assert_eq!(
        store::harness_source_id(&root.join("sessions")),
        f.harness_source
    );
}

#[test]
fn harness_settlements_replace_the_slot_and_retries_add_even_after_restart() {
    let (temp, mut store, home) = setup();
    let directories = roots(&home);
    let first = harness_usage(20, 5, 0, 10);
    let mut attempt = harness_attempt(1, 1, 1, first.clone());
    attempt["data"]["stream"].as_array_mut().unwrap().insert(0, json!({"type":"chunk","time":1795392000000i64,"chunk":{"type":"usage","usage":harness_usage(10,1,0,1)}}));
    let mut message = harness_message(2, 1, 1, harness_usage(30, 6, 0, 10));
    message["data"]["stream"] =
        harness_attempt(2, 1, 1, harness_usage(9999, 0, 0, 1))["data"]["stream"].clone();
    let path = write_harness(
        Path::new(&directories.harness),
        harness_header("retry"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            attempt,
            message,
            harness_message(3, 1, 1, harness_usage(30, 6, 0, 10)),
        ],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let before = store.records(&f, None, None).unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].tokens.input, 36);
    append_harness(
        &path,
        &[
            harness_event(
                4,
                "llm/retry",
                json!({"retryId":"retry-1","turn":1,"step":1,"provider":"deepseek","mode":"normal","policyKey":"test","retry":1,"maxRetries":2,"delayMs":1,"failure":{"message":"test","code":"TEST"}}),
            ),
            harness_message(5, 1, 1, harness_usage(40, 7, 0, 10)),
            harness_event(
                6,
                "llm/retry-started",
                json!({"retryId":"retry-1","turn":1,"step":1,"retry":1}),
            ),
            harness_request(7, "other-provider", "other-model"),
            harness_attempt(8, 1, 1, harness_usage(50, 8, 0, 10)),
            harness_event(
                9,
                "llm/retry-started",
                json!({"retryId":"unrelated","turn":9,"step":9,"retry":1}),
            ),
            harness_message(10, 1, 1, harness_usage(60, 9, 0, 10)),
            harness_event(
                11,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"aborted","reason":{"kind":"user"}}}),
            ),
            harness_event(
                12,
                "compaction/summary",
                json!({"compactionId":"compact-1","summary":[],"shadowedRange":{"start":2,"end":3},"shadowedSeqs":[2,3],"shadowedTokenCount":1,"provider":"deepseek","model":"deepseek-v4","usage":harness_usage(9999,0,0,10)}),
            ),
        ],
    );
    scan_sources(&mut store, &directories);
    let after = store.records(&f, None, None).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after.iter().map(|r| r.tokens.input).sum::<i64>(), 116);
    assert!(after.iter().any(|r| r.id == before[0].id && r.line == 7));
    assert!(after
        .iter()
        .all(|r| r.provider == "deepseek" && r.model == "deepseek-v4"));
    let ids = after.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    drop(store);
    let mut store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    scan_sources(&mut store, &directories);
    store.scan_sources(&directories, true, |_, _| {}).unwrap();
    assert_eq!(
        store
            .records(&f, None, None)
            .unwrap()
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        store
            .overview(&f, &directories, ScanStatus::default())
            .unwrap()
            .summary
            .records,
        2
    );
}

#[test]
fn harness_seeded_usage_only_counts_owned_requests_and_keeps_role_and_parent() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let mut child = harness_header("child");
    child["isSeeded"] = json!(true);
    child["origin"] = json!("subagent");
    child["delegationDepth"] = json!(1);
    child["parentSession"] = json!("parent");
    write_harness(
        Path::new(&directories.harness),
        child,
        &[
            harness_request(0, "inherited-provider", "inherited-model"),
            harness_message(1, 1, 1, harness_usage(1000, 800, 0, 100)),
            harness_event(2, "session/end-seed", json!({"inherited":true})),
            harness_message(3, 1, 1, harness_usage(2000, 1600, 0, 100)),
            harness_event(4, "session/end-seed", json!({"inherited":true})),
            harness_attempt(5, 2, 1, harness_usage(10, 8, 2, 3)),
        ],
    );
    let mut fork = harness_header("fork");
    fork["parentSession"] = json!("parent");
    write_harness(
        Path::new(&directories.harness),
        fork,
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(20, 5, 0, 5)),
        ],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let detail = store.detail(&f, "harness:child", 0).unwrap();
    assert_eq!(
        (detail.session.summary.records, detail.session.summary.input),
        (1, 20)
    );
    assert_eq!(detail.session.parent_id.as_deref(), Some("harness:parent"));
    assert_eq!(detail.session.role, "agent");
    assert_eq!(detail.session.client, "harness");
    assert_eq!(detail.records[0].provider, "inherited-provider");
    assert_eq!(detail.records[0].model, "inherited-model");
    assert_eq!(detail.records[0].line, 7);
    let fork = store.detail(&f, "harness:fork", 0).unwrap();
    assert_eq!(fork.session.role, "main");
    assert_eq!(fork.session.parent_id.as_deref(), Some("harness:parent"));
}

#[test]
fn both_clients_share_queries_but_never_share_ids_roots_quota_or_issues() {
    let (temp, mut store, home) = setup();
    let mut directories = roots(&home);
    let u = tokens(1000, 800, 100);
    write(
        &home.join("sessions/same.jsonl"),
        &[
            meta("same", 0),
            context("codex-only", 1),
            count(u.clone(), u, 2),
            count(json!({"input_tokens":-1,"output_tokens":2}), Value::Null, 3),
        ],
    );
    write_harness(
        Path::new(&directories.harness),
        harness_header("same"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(100, 20, 0, 10)),
            harness_message(2, 1, 2, json!({"inputTokens":-1,"outputTokens":2})),
            harness_event(
                3,
                "session/title",
                json!({"title":"Old title","messageSeqs":[],"source":{"kind":"fallback"}}),
            ),
            harness_event(
                4,
                "session/title",
                json!({"title":"Harness title","messageSeqs":[],"source":{"kind":"user"}}),
            ),
        ],
    );
    scan_sources(&mut store, &directories);
    for (client, count) in [(None, 2), (Some("codex"), 1), (Some("harness"), 1)] {
        let mut f = harness_filters(&directories, client);
        f.buckets.push(Bucket {
            start: f.start,
            end: f.end,
            label: "all".into(),
        });
        let result = store
            .overview(&f, &directories, ScanStatus::default())
            .unwrap();
        let records = store.records(&f, None, None).unwrap();
        let sessions = store.sessions(&f, 0, "tokens").unwrap();
        assert_eq!(result.summary.records, count);
        assert_eq!(result.summary.records, result.trend[0].summary.records);
        assert_eq!(result.issue_count, count);
        assert_eq!(records.len() as i64, count);
        assert_eq!(sessions.total, count);
        assert_eq!(sessions.items.len() as i64, count);
        assert_eq!(result.quota.is_none(), client == Some("harness"));
        assert_eq!(
            store
                .issues(&f.source, &f.harness_source, client)
                .unwrap()
                .len() as i64,
            count
        );
        for session in sessions.items {
            assert!(client.is_none_or(|client| session.client == client));
            let detail = store.detail(&f, &session.id, 0).unwrap();
            assert_eq!(detail.total_records, session.summary.records);
            assert_eq!(detail.session.client, session.client);
            assert!(detail.records.iter().all(|r| r.client == session.client));
            if session.client == "harness" {
                assert_eq!(session.id, "harness:same");
                assert_eq!(session.title, "Harness title");
            } else {
                assert_eq!(session.id, "same");
            }
        }
        let export = temp.path().join(format!("{}.csv", client.unwrap_or("all")));
        write_csv(records, &export).unwrap();
        let csv = fs::read_to_string(export).unwrap();
        assert!(csv.lines().next().unwrap().ends_with("行号,客户端"));
        assert_eq!(csv.lines().count() as i64, count + 1);
        if client == Some("harness") {
            assert!(csv.contains("\"harness:same\""));
            assert!(csv.contains("session.v4.jsonl.zstd"));
            assert!(csv
                .lines()
                .skip(1)
                .all(|line| line.ends_with("\"harness\"")));
            assert_eq!(result.available_models, ["deepseek-v4"]);
            assert!(store.detail(&f, "same", 0).is_err());
        }
        if client == Some("codex") {
            assert_eq!(result.available_models, ["codex-only"]);
            assert!(store.detail(&f, "harness:same", 0).is_err());
        }
    }
    let original = harness_filters(&directories, Some("harness"));
    directories.harness = temp
        .path()
        .join("other-harness")
        .to_string_lossy()
        .into_owned();
    write_harness(
        Path::new(&directories.harness),
        harness_header("same"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(500, 50, 0, 5)),
        ],
    );
    scan_sources(&mut store, &directories);
    let changed = harness_filters(&directories, Some("harness"));
    assert_eq!(
        store.records(&original, None, None).unwrap()[0]
            .tokens
            .input,
        120
    );
    assert_eq!(
        store.records(&changed, None, None).unwrap()[0].tokens.input,
        550
    );
    assert_ne!(
        store.records(&original, None, None).unwrap()[0].id,
        store.records(&changed, None, None).unwrap()[0].id
    );
    let all = harness_filters(&directories, None);
    assert_eq!(
        store
            .overview(&all, &directories, ScanStatus::default())
            .unwrap()
            .summary
            .records,
        2
    );
}

#[test]
fn harness_custom_prices_use_disjoint_buckets_without_inventing_official_rates() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let mut message = harness_message(1, 1, 1, harness_usage(100, 800, 100, 200));
    message["data"]["message"]["source"]["model"] = json!("gpt-6.1-sol");
    message["data"]["message"]["source"]["provider"] = json!("relay");
    write_harness(
        Path::new(&directories.harness),
        harness_header("priced"),
        &[harness_request(0, "relay", "gpt-6.1-sol"), message],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    assert_eq!(store.records(&f, None, None).unwrap()[0].cost, None);
    let mut p = price("gpt-6.1-sol");
    p.provider = "relay".into();
    p.cache_write = Some(3.0);
    store.save_prices(&[p]).unwrap();
    let detail = store.detail(&f, "harness:priced", 0).unwrap();
    assert!((detail.session.summary.cost - 0.00275).abs() < 1e-10);
    assert!((detail.cost_parts.iter().sum::<f64>() - detail.session.summary.cost).abs() < 1e-10);
    assert_eq!(detail.cost_groups[0].input, 1000);
    assert_eq!(detail.cost_groups[0].cached, 800);
    assert_eq!(detail.cost_groups[0].cache_write, 100);
    assert!(detail.cost_groups[0].price.custom);
}

#[test]
fn harness_total_and_reasoning_warnings_do_not_change_the_four_buckets() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let mut usage = harness_usage(100, 50, 25, 10);
    usage["totalTokens"] = json!(999);
    usage["reasoningTokens"] = json!(20);
    let mut with_reasoning = harness_usage(20, 0, 0, 10);
    with_reasoning["reasoningTokens"] = json!(6);
    write_harness(
        Path::new(&directories.harness),
        harness_header("warnings"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, usage),
            harness_message(2, 1, 2, with_reasoning),
            harness_message(3, 1, 3, json!({"inputTokens":2.5,"outputTokens":1})),
        ],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let detail = store.detail(&f, "harness:warnings", 0).unwrap();
    assert_eq!(
        (
            detail.session.summary.records,
            detail.session.summary.input,
            detail.session.summary.output
        ),
        (2, 195, 20)
    );
    assert_eq!(detail.session.summary.reasoning, 6);
    assert_eq!(detail.session.summary.reasoning_known_output, 10);
    let issues = store
        .issues(&f.source, &f.harness_source, Some("harness"))
        .unwrap();
    assert_eq!(issues.len(), 2);
    assert!(issues.iter().all(|i| i.client == "harness"));
    assert!(issues.iter().any(|i| i.line == 3
        && i.reason.contains("totalTokens=999")
        && i.reason.contains("reasoningTokens=20")));
    assert!(issues
        .iter()
        .any(|i| i.line == 5 && i.reason.contains("inputTokens")));
}

#[test]
fn harness_rewritten_sessions_replace_records_and_clear_stale_issues() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let root = Path::new(&directories.harness);
    let path = write_harness(
        root,
        harness_header("rewritten"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(100, 0, 0, 10)),
            harness_message(2, 1, 2, harness_usage(200, 0, 0, 20)),
            harness_message(3, 1, 3, json!({"inputTokens":-1,"outputTokens":1})),
        ],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    assert_eq!(store.records(&f, None, None).unwrap().len(), 2);
    assert_eq!(
        store
            .issues(&f.source, &f.harness_source, Some("harness"))
            .unwrap()
            .len(),
        1
    );
    write_harness(
        root,
        harness_header("rewritten"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(1, 0, 0, 1)),
        ],
    );
    let before = fs::read(&path).unwrap();
    scan_sources(&mut store, &directories);
    assert_eq!(store.records(&f, None, None).unwrap().len(), 1);
    assert_eq!(store.records(&f, None, None).unwrap()[0].tokens.input, 1);
    assert!(store
        .issues(&f.source, &f.harness_source, Some("harness"))
        .unwrap()
        .is_empty());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn harness_partial_frame_is_counted_once_after_it_is_complete() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let path = write_harness(
        Path::new(&directories.harness),
        harness_header("partial"),
        &[harness_request(0, "deepseek", "deepseek-v4")],
    );
    let frame = harness_frame(&harness_message(1, 1, 1, harness_usage(100, 50, 0, 10)));
    let split = frame.len() / 2;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&frame[..split])
        .unwrap();
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    assert!(store.records(&f, None, None).unwrap().is_empty());
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&frame[split..])
        .unwrap();
    scan_sources(&mut store, &directories);
    scan_sources(&mut store, &directories);
    assert_eq!(store.records(&f, None, None).unwrap().len(), 1);
    assert_eq!(store.records(&f, None, None).unwrap()[0].tokens.input, 150);
}

#[test]
fn missing_default_roots_are_empty_and_one_invalid_root_does_not_block_the_other() {
    let (temp, mut store, home) = setup();
    let directories = Directories {
        codex: home.join("missing").to_string_lossy().into_owned(),
        harness: temp
            .path()
            .join("missing-harness")
            .to_string_lossy()
            .into_owned(),
    };
    let mut progress = Vec::new();
    assert!(store
        .scan_sources(&directories, false, |done, total| progress
            .push((done, total)))
        .unwrap()
        .is_empty());
    assert_eq!(progress.last(), Some(&(0, 0)));
    assert_eq!(
        store
            .overview(
                &harness_filters(&directories, None),
                &directories,
                ScanStatus::default()
            )
            .unwrap()
            .summary
            .records,
        0
    );
    let mut directories = roots(&home);
    let invalid = temp.path().join("not-a-directory");
    fs::write(&invalid, "not a source directory").unwrap();
    directories.codex = invalid.to_string_lossy().into_owned();
    write_harness(
        Path::new(&directories.harness),
        harness_header("valid"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(100, 0, 0, 10)),
        ],
    );
    let errors = store.scan_sources(&directories, false, |_, _| {}).unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("Codex"));
    assert_eq!(
        store
            .records(&harness_filters(&directories, Some("harness")), None, None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn harness_message_stream_fallback_and_zero_replacement_use_the_same_slot() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let mut message = harness_message(1, 1, 1, Value::Null);
    message["data"].as_object_mut().unwrap().remove("usage");
    message["data"]["stream"] =
        harness_attempt(1, 1, 1, harness_usage(100, 50, 0, 10))["data"]["stream"].clone();
    let path = write_harness(
        Path::new(&directories.harness),
        harness_header("fallback"),
        &[harness_request(0, "deepseek", "deepseek-v4"), message],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let before = store.records(&f, None, None).unwrap().remove(0);
    assert_eq!(before.tokens.input, 150);
    let mut interrupted = harness_message(2, 1, 1, harness_usage(0, 0, 0, 0));
    interrupted["data"]["interrupted"] = json!(true);
    append_harness(&path, &[interrupted]);
    scan_sources(&mut store, &directories);
    let after = store.records(&f, None, None).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before.id);
    assert_eq!(after[0].tokens.input + after[0].tokens.output, 0);
}

#[test]
fn harness_source_view_is_readable_readonly_and_never_changes_source_or_records() {
    let (temp, mut store, home) = setup();
    let directories = roots(&home);
    let source = write_harness(
        Path::new(&directories.harness),
        harness_header("view"),
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(100, 50, 0, 10)),
        ],
    );
    scan_sources(&mut store, &directories);
    let f = harness_filters(&directories, Some("harness"));
    let records = store.records(&f, None, None).unwrap();
    let record = &records[0];
    let before_records = serde_json::to_value(&records).unwrap();
    let before_hash = blake3::hash(&fs::read(&source).unwrap());
    let directory = temp.path().join("usage-sources");
    let view = store.source_view(&record.id, &directory).unwrap();
    assert_ne!(Path::new(&view), source);
    assert!(Path::new(&view).starts_with(&directory));
    assert!(fs::metadata(&view).unwrap().permissions().readonly());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&view).unwrap().permissions().mode() & 0o077, 0);
    }
    let content = fs::read_to_string(&view).unwrap();
    let lines = content.lines().collect::<Vec<_>>();
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap()["id"],
        "view"
    );
    let event: Value = serde_json::from_str(lines[record.line as usize - 1]).unwrap();
    assert_eq!(event["seq"], 1);
    assert_eq!(event["data"]["message"]["id"], "message-1");
    assert_eq!(record.file, source.to_string_lossy());
    assert_eq!(
        store.source_path(&record.id).unwrap(),
        source.to_string_lossy()
    );
    let export = temp.path().join("view.csv");
    write_csv(records.clone(), &export).unwrap();
    let csv = fs::read_to_string(export).unwrap();
    assert!(csv.contains(source.to_str().unwrap()));
    assert!(!csv.contains(&view));
    let modified = fs::metadata(&view).unwrap().modified().unwrap();
    let id = record.id.clone();
    drop(store);
    let store = Store::open(&temp.path().join("usage.sqlite")).unwrap();
    assert_eq!(store.source_view(&id, &directory).unwrap(), view);
    assert_eq!(fs::metadata(&view).unwrap().modified().unwrap(), modified);
    assert_eq!(
        serde_json::to_value(store.records(&f, None, None).unwrap()).unwrap(),
        before_records
    );
    assert_eq!(blake3::hash(&fs::read(source).unwrap()), before_hash);
}

#[test]
fn source_view_serializer_preserves_logical_line_gaps_and_raw_fields() {
    let mut event = harness_message(1, 1, 1, harness_usage(1, 2, 3, 4));
    event["ignorable"] = json!(true);
    let log = crate::harness::SessionLog {
        header: harness_header("gaps"),
        events: vec![
            crate::harness::Event {
                line: 2,
                value: harness_request(0, "deepseek", "deepseek-v4"),
            },
            crate::harness::Event {
                line: 5,
                value: event.clone(),
            },
        ],
        offset: 0,
        seed_cutoff: None,
        warnings: vec![],
    };
    let content = store::source_jsonl(&log).unwrap();
    let lines = content.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[2], "");
    assert_eq!(lines[3], "");
    assert_eq!(serde_json::from_str::<Value>(lines[4]).unwrap(), event);
}

#[test]
fn harness_missing_cwd_stays_unassigned_instead_of_using_the_application_project() {
    let (_temp, mut store, home) = setup();
    let directories = roots(&home);
    let mut header = harness_header("missing-cwd");
    header.as_object_mut().unwrap().remove("cwd");
    write_harness(
        Path::new(&directories.harness),
        header,
        &[
            harness_request(0, "deepseek", "deepseek-v4"),
            harness_message(1, 1, 1, harness_usage(100, 50, 0, 10)),
        ],
    );
    scan_sources(&mut store, &directories);
    let mut f = harness_filters(&directories, Some("harness"));
    let records = store.records(&f, None, None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project, "");
    assert_eq!(
        store
            .detail(&f, "harness:missing-cwd", 0)
            .unwrap()
            .session
            .project,
        ""
    );
    f.project = Some(String::new());
    assert_eq!(
        store
            .overview(&f, &directories, ScanStatus::default())
            .unwrap()
            .summary
            .records,
        1
    );
    let output = crate::git::command()
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to locate the application repository for the test"
    );
    f.project = Some(String::from_utf8(output.stdout).unwrap().trim().to_string());
    assert!(store.records(&f, None, None).unwrap().is_empty());
    assert_eq!(
        store
            .overview(&f, &directories, ScanStatus::default())
            .unwrap()
            .summary
            .records,
        0
    );
}

#[test]
fn harness_project_follows_live_and_inherited_working_directory_changes() {
    let (temp, mut store, home) = setup();
    let mut project_a = temp.path().join("repository-a");
    let mut project_b = temp.path().join("repository-b");
    for project in [&project_a, &project_b] {
        let output = crate::git::command()
            .args(["init", "--quiet"])
            .arg(project)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Failed to initialize the test repository"
        );
    }
    project_a = project_a.canonicalize().unwrap();
    project_b = project_b.canonicalize().unwrap();
    let project_a = project_a.to_string_lossy().into_owned();
    let project_b = project_b.to_string_lossy().into_owned();
    let directories = roots(&home);
    let prefix = vec![
        harness_request(0, "deepseek", "deepseek-v4"),
        harness_message(1, 1, 1, harness_usage(10, 1, 0, 1)),
        harness_event(2, "working-directory/change", json!({"cwd":project_b})),
        harness_message(3, 1, 2, harness_usage(20, 2, 0, 2)),
    ];
    let mut header = harness_header("moving");
    header["cwd"] = json!(project_a);
    write_harness(Path::new(&directories.harness), header.clone(), &prefix);
    header["id"] = json!("moving-child");
    header["parentSession"] = json!("moving");
    header["isSeeded"] = json!(true);
    let mut inherited = prefix;
    inherited.push(harness_event(
        4,
        "session/end-seed",
        json!({"inherited":true}),
    ));
    inherited.push(harness_attempt(5, 2, 1, harness_usage(30, 3, 0, 3)));
    write_harness(Path::new(&directories.harness), header, &inherited);
    scan_sources(&mut store, &directories);
    let mut f = harness_filters(&directories, Some("harness"));
    let all = store
        .overview(&f, &directories, ScanStatus::default())
        .unwrap();
    assert_eq!(
        (all.summary.records, all.summary.input, all.summary.output),
        (3, 66, 6)
    );
    let child = store.detail(&f, "harness:moving-child", 0).unwrap();
    assert_eq!(
        (child.session.summary.records, child.session.summary.input),
        (1, 33)
    );
    assert_eq!(child.session.parent_id.as_deref(), Some("harness:moving"));
    assert_eq!(child.session.role, "main");
    assert_eq!(child.records[0].project, project_b);
    assert_eq!(child.records[0].provider, "deepseek");
    f.project = Some(project_b.clone());
    let b = store
        .overview(&f, &directories, ScanStatus::default())
        .unwrap();
    assert_eq!(
        (
            b.summary.records,
            b.summary.sessions,
            b.summary.input,
            b.summary.output
        ),
        (2, 2, 55, 5)
    );
    assert!(store
        .records(&f, None, None)
        .unwrap()
        .iter()
        .all(|r| r.project == project_b));
    f.project = Some(project_a.clone());
    let a = store
        .overview(&f, &directories, ScanStatus::default())
        .unwrap();
    assert_eq!(
        (a.summary.records, a.summary.input, a.summary.output),
        (1, 11, 1)
    );
    assert_eq!(store.records(&f, None, None).unwrap()[0].project, project_a);
}

#[test]
#[ignore = "reads local Harness session logs into a temporary statistics database without model calls"]
fn local_harness_log_replay() {
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default, PartialEq)]
    struct FileSnapshot {
        records: Vec<Value>,
        tokens: [i64; 5],
        cursors: Vec<(String, String, String)>,
        issues: Vec<(i64, String)>,
    }

    let temp = tempfile::tempdir().unwrap();
    let home = std::env::var_os("AIMEMORY_HARNESS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(crate::harness::default_harness_directory);
    let directories = Directories {
        codex: temp
            .path()
            .join("missing-codex")
            .to_string_lossy()
            .into_owned(),
        harness: home.to_string_lossy().into_owned(),
    };
    let revisions = || {
        crate::harness::list_sessions(&home)
            .unwrap()
            .into_iter()
            .map(|path| {
                let revision = crate::harness::revision(&path).unwrap();
                (path, revision)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let database = temp.path().join("usage.sqlite");
    let mut store = Store::open(&database).unwrap();
    let f = harness_filters(&directories, Some("harness"));
    let snapshot = |store: &Store| {
        let mut files = BTreeMap::<PathBuf, FileSnapshot>::new();
        for record in store.records(&f, None, None).unwrap() {
            assert!(record.client == "harness" && record.thread_id.starts_with("harness:"));
            let file = files.entry(PathBuf::from(&record.file)).or_default();
            for (sum, count) in file.tokens.iter_mut().zip([
                record.tokens.input,
                record.tokens.cached.unwrap_or(0),
                record.tokens.cache_write,
                record.tokens.output,
                record.tokens.reasoning.unwrap_or(0),
            ]) {
                *sum += count;
            }
            file.records.push(serde_json::to_value(record).unwrap());
        }
        let db = rusqlite::Connection::open(&database).unwrap();
        let mut statement = db
            .prepare("SELECT source,name,path,cursor FROM usage_files ORDER BY source,name")
            .unwrap();
        let cursors = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        for cursor in cursors {
            let (source, name, path, revision) = cursor.unwrap();
            assert!(source == f.harness_source);
            files
                .entry(PathBuf::from(path))
                .or_default()
                .cursors
                .push((source, name, revision));
        }
        let mut statement = db
            .prepare("SELECT file,line,reason FROM usage_issues WHERE source=?1 ORDER BY file,line")
            .unwrap();
        let issues = statement
            .query_map([&f.harness_source], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        for issue in issues {
            let (path, line, reason) = issue.unwrap();
            files
                .entry(PathBuf::from(path))
                .or_default()
                .issues
                .push((line, reason));
        }
        files
    };
    let mut snapshots = Vec::new();
    let mut all_files = BTreeSet::new();
    let mut scan_ms = Vec::new();
    let mut issue_counts = Vec::new();
    for _ in 0..3 {
        let before = revisions();
        let started = std::time::Instant::now();
        let errors = store.scan_sources(&directories, false, |_, _| {}).unwrap();
        scan_ms.push(started.elapsed().as_millis());
        let after = revisions();
        assert!(
            errors.is_empty(),
            "Harness replay returned {} file errors",
            errors.len()
        );
        let files = snapshot(&store);
        issue_counts.push(files.values().map(|file| file.issues.len()).sum::<usize>());
        all_files.extend(before.keys().cloned());
        all_files.extend(after.keys().cloned());
        all_files.extend(files.keys().cloned());
        snapshots.push((before, after, files));
    }
    let started = std::time::Instant::now();
    let stable_files = all_files
        .iter()
        .filter(|path| {
            let Some(expected) = snapshots[0].0.get(*path) else {
                return false;
            };
            snapshots.iter().all(|(before, after, _)| {
                before.get(*path) == Some(expected) && after.get(*path) == Some(expected)
            })
        })
        .collect::<Vec<_>>();
    assert!(
        !stable_files.is_empty(),
        "Harness replay found no files stable across all three scans"
    );
    let mut stable_records = 0;
    let mut stable_tokens = 0;
    let mut stable_issues = 0;
    for path in stable_files.iter().copied() {
        let baseline = snapshots[0]
            .2
            .get(path)
            .expect("A stable source was not imported");
        assert!(
            baseline.cursors.len() == 1,
            "A stable source did not have exactly one persisted cursor"
        );
        assert!(
            baseline.cursors[0].2 == snapshots[0].0[path],
            "A stable source cursor did not match its revision"
        );
        for (_, _, files) in snapshots.iter().skip(1) {
            assert!(
                files.get(path) == Some(baseline),
                "Stable Harness records, tokens, source, issues, or cursor changed on replay"
            );
        }
        stable_records += baseline.records.len();
        stable_tokens += baseline.tokens[0] + baseline.tokens[3];
        stable_issues += baseline.issues.len();
    }
    assert!(
        stable_records > 0,
        "Harness replay found no usage records in stable files"
    );
    let comparison_ms = started.elapsed().as_millis();
    println!(
        "{}",
        json!({
            "stableFiles":stable_files.len(),"changingFiles":all_files.len()-stable_files.len(),
            "stableRecords":stable_records,"stableTokens":stable_tokens,
            "scanMs":scan_ms,"comparisonMs":comparison_ms,
            "issues":issue_counts,"stableIssues":stable_issues,"errors":0
        })
    );
}
