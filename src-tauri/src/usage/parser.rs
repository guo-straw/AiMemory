use super::{QuotaSnapshot, Tokens, UsageRecord};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub offset: u64,
    pub line: i64,
    pub thread: String,
    pub parent: Option<String>,
    pub role: String,
    pub created: i64,
    pub fork_ordinal: Option<u64>,
    pub cwd: String,
    pub model: String,
    pub provider: String,
    pub tier: String,
    pub turn: String,
    #[serde(default)]
    continuation: bool,
    #[serde(default)]
    history_base: Option<String>,
    high_water: Option<Tokens>,
    signatures: BTreeMap<String, String>,
    previous_signature: String,
    response_usage: Option<Tokens>,
}

pub(super) struct Parsed {
    pub record: Option<UsageRecord>,
    pub quota: Option<QuotaSnapshot>,
}

fn text(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap_or_default().to_string()
}

fn timestamp(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}

pub(super) fn relevant(line: &[u8]) -> bool {
    let head = String::from_utf8_lossy(line);
    [
        "session_meta",
        "turn_context",
        "token_count",
        "token_usage_record",
        "thread_settings_applied",
        "task_started",
    ]
    .iter()
    .any(|kind| head.contains(kind))
}

impl Cursor {
    pub fn needs_baseline(&self) -> bool {
        self.continuation && self.high_water.is_none()
    }

    pub fn inherit_baseline(&mut self, previous: &Self) -> bool {
        if previous.thread == self.thread || self.history_base.as_ref() == Some(&previous.thread) {
            self.high_water = previous.high_water.clone();
            return self.high_water.is_some();
        }
        false
    }

    pub fn parse(&mut self, event: &Value, source: &str, file: &str) -> Result<Parsed, String> {
        let mut parsed = Parsed {
            record: None,
            quota: None,
        };
        let payload = &event["payload"];
        let kind = event["type"].as_str().unwrap_or_default();
        if kind == "session_meta" {
            if self.thread.is_empty() {
                self.thread = text(payload, "id");
                if self.thread.is_empty() {
                    self.thread = text(payload, "session_id");
                }
                self.created = timestamp(&payload["timestamp"])
                    .or_else(|| timestamp(&event["timestamp"]))
                    .unwrap_or(0);
                self.history_base = payload["history_base_id"].as_str().map(str::to_string);
                self.continuation = payload["is_continuation"].as_bool().unwrap_or(false)
                    || self.history_base.is_some();
                self.fork_ordinal =
                    payload["forked_from_ordinal_exclusive"]
                        .as_u64()
                        .or_else(|| {
                            payload["subagent_history_start_ordinal"]
                                .as_u64()
                                .and_then(|n| n.checked_sub(1))
                        });
                self.parent = payload["forked_from_id"]
                    .as_str()
                    .or_else(|| payload["parent_thread_id"].as_str())
                    .or_else(|| {
                        payload
                            .pointer("/source/subagent/thread_spawn/parent_thread_id")
                            .and_then(Value::as_str)
                    })
                    .or_else(|| payload["session_id"].as_str().filter(|s| *s != self.thread))
                    .map(str::to_string);
                self.role = match payload["thread_source"].as_str() {
                    Some("user") => "main",
                    Some("guardian_review" | "memory" | "memory_consolidation") => "system",
                    _ if payload.pointer("/source/subagent/thread_spawn").is_some() => "agent",
                    _ if payload["source"].get("subagent").is_some() => "system",
                    _ => "main",
                }
                .into();
                self.cwd = text(payload, "cwd");
                self.provider = text(payload, "model_provider");
            }
            return Ok(parsed);
        }
        if self.thread.is_empty() {
            return Err("缺少根会话身份".into());
        }
        if kind == "turn_context" {
            if let Some(cwd) = payload["cwd"].as_str() {
                self.cwd = cwd.into();
            }
            if let Some(model) = payload["model"].as_str() {
                self.model = model.into();
            }
            self.turn = text(payload, "turn_id");
            if let Some(tier) = payload.get("service_tier") {
                self.tier = tier.as_str().unwrap_or_default().into();
            }
            return Ok(parsed);
        }
        if kind == "event_msg" && payload["type"] == "task_started" {
            self.turn = text(payload, "turn_id");
            return Ok(parsed);
        }
        if kind == "event_msg" && payload["type"] == "thread_settings_applied" {
            let settings = payload.get("thread_settings").unwrap_or(payload);
            if let Some(model) = settings["model"].as_str() {
                self.model = model.into();
            }
            if let Some(tier) = settings.get("service_tier") {
                self.tier = tier.as_str().unwrap_or_default().into();
            }
            if let Some(provider) = settings["model_provider_id"].as_str() {
                self.provider = provider.into();
            }
            if let Some(cwd) = settings["cwd"].as_str() {
                self.cwd = cwd.into();
            }
            return Ok(parsed);
        }
        let at = timestamp(&event["timestamp"]).ok_or("用量记录缺少有效时间")?;
        let inherited = at < self.created
            || self
                .fork_ordinal
                .is_some_and(|cutoff| event["ordinal"].as_u64().is_some_and(|n| n <= cutoff));
        if kind == "token_usage_record" {
            let tokens = Tokens::parse(&payload["usage"])?;
            if inherited {
                return Ok(parsed);
            }
            let response = payload["response_id"]
                .as_str()
                .ok_or("响应级用量缺少 response_id")?;
            self.response_usage = Some(tokens.clone());
            parsed.record = Some(self.record(
                event,
                source,
                file,
                at,
                tokens,
                format!("response:{response}"),
                "response",
            ));
            return Ok(parsed);
        }
        if kind != "event_msg" || payload["type"] != "token_count" {
            return Ok(parsed);
        }
        if !inherited && payload["rate_limits"].is_object() {
            parsed.quota = Some(QuotaSnapshot {
                at,
                thread_id: self.thread.clone(),
                data: payload["rate_limits"].clone(),
            });
        }
        let info = &payload["info"];
        if !info.is_object() {
            return Ok(parsed);
        }
        let total = info["total_token_usage"]
            .is_object()
            .then(|| Tokens::parse(&info["total_token_usage"]))
            .transpose()?;
        let last = info["last_token_usage"]
            .is_object()
            .then(|| Tokens::parse(&info["last_token_usage"]))
            .transpose()?;
        if inherited {
            if let Some(total) = total {
                self.advance_high_water(total);
            }
            return Ok(parsed);
        }
        let signature =
            serde_json::to_string(&(info.get("total_token_usage"), info.get("last_token_usage")))
                .map_err(|e| e.to_string())?;
        let bucket = payload
            .pointer("/rate_limits/limit_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let duplicate = total.is_some()
            && (self.signatures.get(bucket) == Some(&signature)
                || self.previous_signature == signature);
        self.signatures.insert(bucket.into(), signature.clone());
        self.previous_signature = signature;
        let covered_response = self
            .response_usage
            .take()
            .is_some_and(|usage| last.as_ref() == Some(&usage));
        let (tokens, basis) = if let Some(last) = last {
            (last, "last")
        } else if let Some(total) = &total {
            match &self.high_water {
                Some(previous) => (total.delta(previous)?, "cumulative"),
                None if self.parent.is_none() && !self.continuation => {
                    (total.clone(), "cumulative")
                }
                None => return Err("继承会话缺少累计基线与 last_token_usage".into()),
            }
        } else {
            return Ok(parsed);
        };
        if let Some(total) = total {
            self.advance_high_water(total);
        }
        // Context estimates carry total_tokens but no input/output; they are not usage.
        if duplicate || covered_response || tokens.input + tokens.output == 0 {
            return Ok(parsed);
        }
        let identity = format!(
            "event:{}",
            blake3::hash(
                serde_json::to_string(event)
                    .map_err(|e| e.to_string())?
                    .as_bytes()
            )
            .to_hex()
        );
        parsed.record = Some(self.record(event, source, file, at, tokens, identity, basis));
        Ok(parsed)
    }

    fn advance_high_water(&mut self, current: Tokens) {
        match &mut self.high_water {
            Some(high) => {
                high.input = high.input.max(current.input);
                high.output = high.output.max(current.output);
                high.cached = high.cached.zip(current.cached).map(|(a, b)| a.max(b));
                high.cache_write = high.cache_write.max(current.cache_write);
                high.reasoning = high.reasoning.zip(current.reasoning).map(|(a, b)| a.max(b));
            }
            None => self.high_water = Some(current),
        }
    }

    fn record(
        &self,
        event: &Value,
        source: &str,
        file: &str,
        at: i64,
        tokens: Tokens,
        identity: String,
        basis: &str,
    ) -> UsageRecord {
        UsageRecord {
            id: format!("{}:{}:{}", source, self.thread, identity),
            client: "codex".into(),
            thread_id: self.thread.clone(),
            turn_id: event
                .pointer("/payload/turn_id")
                .and_then(Value::as_str)
                .unwrap_or(&self.turn)
                .into(),
            at,
            model: self.model.clone(),
            provider: self.provider.clone(),
            tier: self.tier.clone(),
            project: self.cwd.clone(),
            role: self.role.clone(),
            tokens,
            cost: None,
            file: file.into(),
            line: self.line,
            basis: basis.into(),
        }
    }
}
