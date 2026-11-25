use super::*;

fn filters() -> Filters {
    Filters {
        source: "pricing-codex".into(),
        harness_source: "harness:pricing".into(),
        client: None,
        start: 0,
        end: i64::MAX,
        project: None,
        model: None,
        role: None,
        buckets: vec![],
    }
}

fn record(id: &str, provider: &str, model: &str, tier: &str, client: &str) -> UsageRecord {
    UsageRecord {
        id: id.into(),
        client: client.into(),
        thread_id: format!("{client}:pricing"),
        turn_id: id.into(),
        at: Utc::now().timestamp_millis(),
        model: model.into(),
        provider: provider.into(),
        tier: tier.into(),
        project: String::new(),
        role: "main".into(),
        tokens: Tokens {
            input: 1_000,
            cached: Some(800),
            cache_write: 0,
            output: 100,
            reasoning: Some(50),
        },
        cost: None,
        file: "pricing-fixture.jsonl".into(),
        line: 1,
        basis: "test".into(),
    }
}

fn put(store: &Store, record: UsageRecord) {
    let source = if record.client == "harness" {
        "harness:pricing"
    } else {
        "pricing-codex"
    };
    let tx = store.db.unchecked_transaction().unwrap();
    save_record(&tx, source, record, &mut HashMap::new()).unwrap();
    tx.commit().unwrap();
}

fn listed(store: &Store, provider: &str, model: &str, tier: &str) -> Price {
    store
        .prices()
        .unwrap()
        .into_iter()
        .find(|p| p.provider == provider && p.model == model && p.tier == tier)
        .unwrap()
}

fn cost(store: &Store, id: &str) -> Option<f64> {
    store
        .records(&filters(), None, None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap()
        .cost
}

fn near(actual: Option<f64>, expected: f64) {
    assert!(
        (actual.unwrap() - expected).abs() < 1e-10,
        "{actual:?} != {expected}"
    );
}

#[test]
fn official_defaults_keep_custom_routes_and_codex_identity() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    for (id, provider, client) in [
        ("gateway", "my-gateway", "harness"),
        ("codex", "openai", "codex"),
        ("codex-gateway", "codex-proxy", "codex"),
    ] {
        put(&store, record(id, provider, "gpt-6.1-sol", "", client));
        near(cost(&store, id), 0.00148);
        let p = listed(&store, provider, "gpt-6.1-sol", "");
        assert_eq!(
            (p.input, p.cached, p.cache_write, p.output),
            (Some(2.0), Some(0.1), None, Some(10.0))
        );
        assert!(!p.custom);
        assert!(p.source.starts_with("OpenAI 官方参考价"));
        assert!(p.source.contains("缓存写入缺TTL未定价"));
        let detail = store
            .detail(&filters(), &format!("{client}:pricing"), 0)
            .unwrap();
        let group = detail
            .cost_groups
            .iter()
            .find(|g| g.price.provider == provider)
            .unwrap();
        assert_eq!(group.price.source, p.source);
        assert_eq!(
            (
                group.price.input,
                group.price.cached,
                group.price.cache_write,
                group.price.output
            ),
            (p.input, p.cached, p.cache_write, p.output)
        );
        near(
            Some(detail.cost_parts.iter().sum()),
            detail.session.summary.cost,
        );
    }
    let records = store.records(&filters(), None, None).unwrap();
    assert!(records
        .iter()
        .any(|r| r.id == "gateway" && r.client == "harness" && r.provider == "my-gateway"));
    assert!(records
        .iter()
        .any(|r| r.id == "codex-gateway" && r.client == "codex" && r.provider == "codex-proxy"));
    let summary = store.aggregate(&filters()).unwrap();
    assert_eq!((summary.records, summary.priced_records), (3, 3));
    near(Some(summary.cost), 0.00444);
}

#[test]
fn manual_prices_survive_opening_and_restore_only_the_requested_exact_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pricing.sqlite");
    let store = Store::open(&path).unwrap();
    put(
        &store,
        record("short", "gateway", "gpt-6.1-sol", "", "harness"),
    );
    let mut long = record("long", "gateway", "gpt-6.1-sol", "", "harness");
    long.tokens = Tokens {
        input: 300_000,
        cached: Some(280_000),
        cache_write: 1_000,
        output: 100,
        reasoning: Some(50),
    };
    put(&store, long);
    put(
        &store,
        record("priority", "gateway", "gpt-6.1-sol", "priority", "harness"),
    );
    put(
        &store,
        record("other", "other-gateway", "gpt-6.1-sol", "", "harness"),
    );
    let mut manual = Price {
        model: "gpt-6.1-sol".into(),
        provider: "gateway".into(),
        tier: String::new(),
        input: Some(11.0),
        cached: Some(0.5),
        cache_write: Some(7.0),
        output: Some(19.0),
        source: "保留的网关报价".into(),
        custom: true,
    };
    let other = Price {
        provider: "other-gateway".into(),
        ..manual.clone()
    };
    store.save_prices(&[manual.clone(), other]).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let restored = listed(&store, "gateway", "gpt-6.1-sol", "");
    assert!(restored.custom);
    assert_eq!(
        (
            restored.input,
            restored.cached,
            restored.cache_write,
            restored.output
        ),
        (Some(11.0), Some(0.5), Some(7.0), Some(19.0))
    );
    assert_eq!(restored.source, "保留的网关报价");
    near(cost(&store, "short"), 0.0045);
    near(cost(&store, "long"), 0.3579);
    near(cost(&store, "priority"), 0.00296);
    manual.custom = false;
    store.save_prices(&[manual]).unwrap();
    near(cost(&store, "short"), 0.00148);
    assert_eq!(cost(&store, "long"), None);
    assert!(!listed(&store, "gateway", "gpt-6.1-sol", "").custom);
    assert!(listed(&store, "other-gateway", "gpt-6.1-sol", "").custom);
    near(cost(&store, "other"), 0.0045);
}

#[test]
fn partial_or_zero_manual_prices_are_not_filled_from_official_rates() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    put(
        &store,
        record("manual", "gateway", "gpt-6.1-sol", "", "harness"),
    );
    let mut manual = listed(&store, "gateway", "gpt-6.1-sol", "");
    manual.input = Some(9.0);
    manual.cached = None;
    manual.cache_write = None;
    manual.output = None;
    manual.source = "部分手动报价".into();
    manual.custom = true;
    store.save_prices(&[manual.clone()]).unwrap();
    let p = listed(&store, "gateway", "gpt-6.1-sol", "");
    assert_eq!(
        (p.input, p.cached, p.cache_write, p.output),
        (Some(9.0), None, None, None)
    );
    assert!(p.custom);
    assert_eq!(cost(&store, "manual"), None);
    manual.input = Some(0.0);
    manual.cached = Some(0.0);
    manual.cache_write = Some(0.0);
    manual.output = Some(0.0);
    store.save_prices(&[manual.clone()]).unwrap();
    near(cost(&store, "manual"), 0.0);
    manual.input = None;
    manual.cached = None;
    manual.cache_write = None;
    manual.output = None;
    store.save_prices(&[manual]).unwrap();
    near(cost(&store, "manual"), 0.00148);
    assert!(!listed(&store, "gateway", "gpt-6.1-sol", "").custom);
}

#[test]
fn unknown_ids_model_suffixes_and_unlisted_tiers_stay_unpriced() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    for model in [
        "unknown",
        "gpt-6.1-sol-latest",
        "gateway/gpt-6.1-sol",
        "GPT-6.1-sol",
        "gpt-6.1-sol-2026-11-23",
        "codex-auto-review",
        "deepseek-flash",
    ] {
        put(&store, record(model, "gateway", model, "", "harness"));
        assert_eq!(cost(&store, model), None);
        let p = listed(&store, "gateway", model, "");
        assert_eq!(
            (p.input, p.cached, p.cache_write, p.output),
            (None, None, None, None)
        );
        assert!(!p.custom);
        assert_eq!(p.source, "暂无已核实的官方参考价");
    }
    put(
        &store,
        record(
            "unlisted-tier",
            "gateway",
            "gpt-6.1-sol",
            "scale",
            "harness",
        ),
    );
    assert_eq!(cost(&store, "unlisted-tier"), None);
    assert_eq!(
        listed(&store, "gateway", "gpt-6.1-sol", "scale").input,
        None
    );
}

#[test]
fn context_and_tier_rules_use_full_input_and_match_the_displayed_short_rates() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    for (i, (tier, input, cached, price_input, price_cached, price_output, expected)) in [
        ("", 100_000, 90_000, 2.0, 0.1, 10.0, 0.039),
        ("default", 100_000, 90_000, 2.0, 0.1, 10.0, 0.039),
        ("auto", 100_000, 90_000, 2.0, 0.1, 10.0, 0.039),
        ("standard", 272_000, 270_000, 2.0, 0.1, 10.0, 0.041),
        ("standard", 272_001, 270_000, 4.0, 0.2, 15.0, 0.077004),
        ("priority", 100_000, 90_000, 4.0, 0.2, 20.0, 0.078),
        ("fast", 272_001, 270_000, 8.0, 0.4, 30.0, 0.154008),
        ("batch", 100_000, 90_000, 1.0, 0.05, 5.0, 0.0195),
        ("flex", 100_000, 90_000, 1.0, 0.05, 5.0, 0.0195),
        ("ultrafast", 100_000, 90_000, 12.0, 0.6, 60.0, 0.234),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("context-{i}");
        let mut r = record(&id, "gateway", "gpt-6.1-sol", tier, "harness");
        r.tokens = Tokens {
            input,
            cached: Some(cached),
            cache_write: 0,
            output: 1_000,
            reasoning: Some(500),
        };
        put(&store, r);
        near(cost(&store, &id), expected);
        let (pi, pc, po, source): (f64, f64, f64, String) = store.db.query_row(
            &format!("{PRICED} SELECT price_input,price_cached,price_output,price_source FROM priced WHERE id=?1"),
            [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!((pi, pc, po), (price_input, price_cached, price_output));
        let p = listed(&store, "gateway", "gpt-6.1-sol", tier);
        if input <= 272_000 {
            assert_eq!(
                (p.input, p.cached, p.output, p.source),
                (Some(pi), Some(pc), Some(po), source)
            );
        } else {
            assert!(p.source.contains("短上下文 ≤272K"));
            assert!(source.contains("长上下文 >272K"));
        }
    }
}

#[test]
fn cache_writes_are_input_priced_only_for_verified_older_models() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    for (model, expected_write, expected_cost) in [
        ("gpt-5.5", 5.0, 0.0071),
        ("gpt-5.3-codex", 1.75, 0.002835),
        ("gpt-4.1", 2.0, 0.0025),
    ] {
        let mut r = record(model, "gateway", model, "", "harness");
        r.tokens = Tokens {
            input: 1_000,
            cached: Some(200),
            cache_write: 100,
            output: 100,
            reasoning: Some(50),
        };
        put(&store, r);
        near(cost(&store, model), expected_cost);
        let p = listed(&store, "gateway", model, "");
        assert_eq!(p.cache_write, Some(expected_write));
        assert!(p.source.contains("缓存写入按输入价"));
    }
    let mut long = record("legacy-long", "gateway", "gpt-5.5", "", "harness");
    long.tokens = Tokens {
        input: 300_000,
        cached: Some(280_000),
        cache_write: 1_000,
        output: 100,
        reasoning: Some(50),
    };
    put(&store, long);
    near(cost(&store, "legacy-long"), 0.4845);
    let mut ttl_unknown = record("ttl-unknown", "gateway", "gpt-6.1-sol", "", "harness");
    ttl_unknown.tokens.cache_write = 100;
    put(&store, ttl_unknown);
    assert_eq!(cost(&store, "ttl-unknown"), None);
    assert_eq!(
        listed(&store, "gateway", "gpt-6.1-sol", "").cache_write,
        None
    );
    let mut cache_unknown = record("cache-unknown", "gateway", "gpt-6.1-sol", "", "harness");
    cache_unknown.tokens.cached = None;
    put(&store, cache_unknown);
    assert_eq!(cost(&store, "cache-unknown"), None);
    let mut no_cached_rate = record("no-cached-rate", "gateway", "gpt-5.5-pro", "", "codex");
    no_cached_rate.tokens.cached = Some(1);
    put(&store, no_cached_rate);
    assert_eq!(cost(&store, "no-cached-rate"), None);
    let mut zero_cached = record("zero-cached", "gateway", "gpt-5.5-pro", "", "codex");
    zero_cached.tokens.cached = Some(0);
    zero_cached.tokens.cache_write = 100;
    put(&store, zero_cached);
    near(cost(&store, "zero-cached"), 0.048);
}

#[test]
fn promotion_uses_the_record_timestamp_and_pacific_boundary() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let boundary = chrono::DateTime::parse_from_rfc3339("2027-03-01T08:00:00Z")
        .unwrap()
        .timestamp_millis();
    for (id, at, expected) in [
        ("before", boundary - 1, 0.00444),
        ("exact", boundary, 0.00888),
        ("after", boundary + 1, 0.00888),
    ] {
        let mut r = record(id, "gateway", "gpt-6.2-sol", "ultrafast", "harness");
        r.at = at;
        put(&store, r);
        near(cost(&store, id), expected);
    }
}
