use super::{Tokens, UsageRecord};
use serde_json::Value;

pub(super) struct Cursor {
    thread: String,
    cwd: String,
    role: String,
    provider: String,
    model: String,
    last: Option<(u64, u64, u64)>,
}

pub(super) struct Parsed {
    pub record: UsageRecord,
    pub warning: Option<String>,
}

fn tokens(value: &Value) -> Result<(Tokens, Option<String>), String> {
    let counter = |key: &str| -> Result<Option<i64>, String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_i64()
                .filter(|n| *n >= 0)
                .map(Some)
                .ok_or_else(|| format!("{key} 必须为非负整数")),
        }
    };
    let uncached = counter("inputTokens")?.ok_or("缺少 inputTokens")?;
    let cached = counter("cacheReadTokens")?;
    let cache_write = counter("cacheWriteTokens")?.unwrap_or(0);
    let output = counter("outputTokens")?.ok_or("缺少 outputTokens")?;
    // Harness reports disjoint input buckets; the existing usage API includes cache in input.
    let input = uncached
        .checked_add(cached.unwrap_or(0))
        .and_then(|n| n.checked_add(cache_write))
        .ok_or("输入 Token 合计超出整数范围")?;
    let total = input.checked_add(output).ok_or("Token 合计超出整数范围")?;
    let mut warnings = Vec::new();
    if let Some(reported) = counter("totalTokens")? {
        if reported != total {
            warnings.push(format!(
                "DSH totalTokens={reported} 与分项合计={total} 不一致，按原生四项口径统计"
            ));
        }
    }
    let mut reasoning = counter("reasoningTokens")?;
    if reasoning.is_some_and(|n| n > output) {
        warnings.push(format!(
            "DSH reasoningTokens={} 超出 outputTokens={output}，未作为其中推理展示",
            reasoning.unwrap()
        ));
        reasoning = None;
    }
    Ok((
        Tokens {
            input,
            cached,
            cache_write,
            output,
            reasoning,
        },
        (!warnings.is_empty()).then(|| warnings.join("；")),
    ))
}

impl Cursor {
    pub fn new(session: &str, cwd: &str, role: &str) -> Self {
        Self {
            thread: format!("harness:{session}"),
            cwd: cwd.into(),
            role: role.into(),
            provider: String::new(),
            model: String::new(),
            last: None,
        }
    }

    pub fn parse(
        &mut self,
        event: &Value,
        source: &str,
        file: &str,
        line: i64,
    ) -> Result<Option<Parsed>, String> {
        let kind = event["type"].as_str().unwrap_or_default();
        let data = &event["data"];
        if kind == "working-directory/change" {
            self.cwd = data["cwd"].as_str().ok_or("工作目录变更缺少 cwd")?.into();
            return Ok(None);
        }
        if kind == "request/header" {
            let config = &data["header"]["config"];
            if let Some(provider) = config["provider"].as_str() {
                self.provider = provider.into();
            }
            if let Some(model) = config["model"].as_str() {
                self.model = model.into();
            }
            return Ok(None);
        }
        if kind == "llm/retry-started" {
            if self.last.is_some_and(|(turn, step, _)| {
                data["turn"].as_u64() == Some(turn) && data["step"].as_u64() == Some(step)
            }) {
                self.last = None;
            }
            return Ok(None);
        }
        if kind != "assistant/message" && kind != "assistant/attempt" {
            return Ok(None);
        }
        let usage = (kind == "assistant/message")
            .then(|| data.get("usage"))
            .flatten()
            .or_else(|| {
                data["stream"].as_array()?.iter().rev().find_map(|record| {
                    (record["type"] == "chunk" && record["chunk"]["type"] == "usage")
                        .then(|| record["chunk"].get("usage"))
                        .flatten()
                })
            });
        let Some(usage) = usage else { return Ok(None) };
        let (tokens, warning) = tokens(usage)?;
        let turn = data["turn"].as_u64().ok_or("用量记录缺少有效 turn")?;
        let step = data["step"].as_u64().ok_or("用量记录缺少有效 step")?;
        let seq = event["seq"].as_u64().ok_or("用量记录缺少有效 seq")?;
        let at = event["time"].as_i64().ok_or("用量记录缺少有效时间")?;
        let slot = self
            .last
            .filter(|(t, s, _)| *t == turn && *s == step)
            .map(|(_, _, seq)| seq)
            .unwrap_or(seq);
        // A retry starts a new billable slot; repeated settlements of one attempt replace it.
        self.last = Some((turn, step, slot));
        let route = &data["message"]["source"];
        let model = route["model"].as_str().unwrap_or(&self.model);
        let provider = route["provider"].as_str().unwrap_or(&self.provider);
        Ok(Some(Parsed {
            record: UsageRecord {
                id: format!("{source}:{}:settlement:{slot}", self.thread),
                client: "harness".into(),
                thread_id: self.thread.clone(),
                turn_id: turn.to_string(),
                at,
                model: model.into(),
                provider: provider.into(),
                tier: String::new(),
                project: self.cwd.clone(),
                role: self.role.clone(),
                tokens,
                cost: None,
                file: file.into(),
                line,
                basis: "harness-buckets".into(),
            },
            warning,
        }))
    }
}
