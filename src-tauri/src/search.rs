use crate::{
    config::{service_fingerprint, ServiceCredentials},
    memory_markdown, model_api,
    storage::{KeywordQuery, MemorySummary},
};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    #[serde(default)]
    pub query: String,
    pub repository_id: Option<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: SearchMode,
    pub group_of: Option<String>,
    pub offset: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Auto,
    Keyword,
    Id,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedSection {
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    #[serde(flatten)]
    pub memory: MemorySummary,
    pub score: f64,
    pub group_count: usize,
    pub matched_sections: Vec<MatchedSection>,
    pub matches_history_only: bool,
    #[serde(skip)]
    group: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    pub query_vector_cached: bool,
    pub embedding_input_tokens: u64,
    pub embedding_latency_ms: u64,
    pub query_usage_reported: bool,
    pub local_search_ms: f64,
    pub pending_embeddings: usize,
    pub remote_requests: usize,
    pub search_mode: String,
    pub score_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

pub struct SearchEngine {
    state: Mutex<SearchState>,
}

struct SearchState {
    db: Connection,
    snapshot: Option<Arc<IndexSnapshot>>,
}

struct IndexSnapshot {
    service_key: String,
    pipeline_version: i64,
    memory_version: i64,
    entries: Vec<IndexedMemory>,
    totals: BTreeMap<String, usize>,
    local: Vec<LocalMemory>,
    groups: BTreeMap<String, usize>,
}

struct IndexedMemory {
    memory: MemorySummary,
    vector: Vec<f32>,
    group: String,
}

struct LocalMemory {
    memory: MemorySummary,
    group: String,
    title: String,
    text: String,
    current: String,
    sections: Vec<(String, String)>,
}

enum QueryCache {
    Ready(Vec<f32>),
    Claimed,
    Waiting,
}

struct QueryVector {
    vector: Vec<f32>,
    cached: bool,
    input_tokens: u64,
    latency_ms: u64,
    usage_reported: bool,
}

pub fn service_key(service: &ServiceCredentials) -> String {
    service_fingerprint(&service.base_url, &service.model)
}

impl SearchEngine {
    pub fn open(root: &Path) -> Result<Self, String> {
        let db = Connection::open_with_flags(
            root.join(".aimemory/pipeline.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(|error| format!("无法打开向量索引：{error}"))?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        db.execute(
            "ATTACH DATABASE ?1 AS memory_index",
            [root.join(".aimemory/index.sqlite").display().to_string()],
        )
        .map_err(|error| error.to_string())?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS query_vectors (
                cache_key TEXT PRIMARY KEY, owner TEXT NOT NULL,
                expires_at INTEGER NOT NULL, vector BLOB, dimensions INTEGER
            );
            CREATE TABLE IF NOT EXISTS search_events (
                id TEXT PRIMARY KEY, query_hash TEXT NOT NULL, mode TEXT NOT NULL,
                cached INTEGER NOT NULL, remote_requests INTEGER NOT NULL, input_tokens INTEGER NOT NULL,
                usage_reported INTEGER NOT NULL, latency_ms INTEGER NOT NULL, local_ms REAL NOT NULL,
                fallback_reason TEXT
            );",
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            state: Mutex::new(SearchState { db, snapshot: None }),
        })
    }

    pub async fn search(
        &self,
        request: SearchRequest,
        service: &ServiceCredentials,
    ) -> Result<SearchResponse, String> {
        self.search_with(request, Some(service_key(service)), || Ok(service.clone()))
            .await
    }

    pub async fn search_with(
        &self,
        request: SearchRequest,
        key: Option<String>,
        credentials: impl FnOnce() -> Result<ServiceCredentials, String>,
    ) -> Result<SearchResponse, String> {
        let (query, limit) = validate_request(&request)?;
        let started = Instant::now();
        let snapshot = self.snapshot(key.as_deref().unwrap_or(""))?;
        let in_scope = |memory: &MemorySummary| {
            request
                .repository_id
                .as_ref()
                .is_none_or(|id| &memory.repository_id == id)
        };
        let total = request
            .repository_id
            .as_ref()
            .map(|id| snapshot.totals.get(id).copied().unwrap_or(0))
            .unwrap_or_else(|| snapshot.totals.values().sum());
        let ready = snapshot
            .entries
            .iter()
            .filter(|e| in_scope(&e.memory))
            .count();
        let mut response = SearchResponse {
            results: Vec::new(),
            query_vector_cached: false,
            embedding_input_tokens: 0,
            embedding_latency_ms: 0,
            query_usage_reported: true,
            local_search_ms: 0.0,
            pending_embeddings: total - ready,
            remote_requests: 0,
            search_mode: "keyword".into(),
            score_kind: "text".into(),
            fallback_reason: None,
            group_of: None,
            offset: None,
            total: None,
            has_more: None,
        };
        if let Some(id) = &request.group_of {
            let member = snapshot
                .local
                .iter()
                .find(|e| e.memory.id == *id && in_scope(&e.memory))
                .ok_or("未找到当前仓库中的分组成员")?;
            let mut members = snapshot
                .local
                .iter()
                .filter(|e| e.group == member.group)
                .collect::<Vec<_>>();
            members.sort_by(|a, b| a.memory.id.cmp(&b.memory.id));
            let offset = request.offset.unwrap_or(0);
            response.total = Some(members.len());
            response.has_more = Some(offset.saturating_add(limit) < members.len());
            response.results = members
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|e| local_hit(e, 1.0, &snapshot.groups, None))
                .collect();
            response.group_of = Some(id.clone());
            response.offset = Some(offset);
            response.search_mode = "group".into();
            response.score_kind = "id".into();
        } else if request.mode == SearchMode::Id
            || snapshot.local.iter().any(|e| e.memory.id == query)
            || ["MEM-", "ARC-", "MD-"]
                .iter()
                .any(|prefix| query.starts_with(prefix))
        {
            response.results = snapshot
                .local
                .iter()
                .filter(|e| e.memory.id == query && in_scope(&e.memory))
                .map(|e| local_hit(e, 1.0, &snapshot.groups, None))
                .collect();
            response.search_mode = "id".into();
            response.score_kind = "id".into();
        } else {
            let keywords = KeywordQuery::new(&query);
            if request.mode == SearchMode::Keyword || ready == 0 || key.is_none() {
                response.results = keyword_rank(
                    &snapshot,
                    request.repository_id.as_deref(),
                    &keywords,
                    limit,
                    false,
                );
                if request.mode != SearchMode::Keyword {
                    response.fallback_reason = Some(
                        if key.is_none() {
                            "未配置向量服务"
                        } else {
                            "当前仓库尚无可用向量"
                        }
                        .into(),
                    );
                }
            } else {
                let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
                drop(snapshot);
                let vector = self
                    .query_vector(&query, key.as_deref().unwrap(), credentials)
                    .await;
                let local_started = Instant::now();
                let snapshot = self.snapshot(key.as_deref().unwrap())?;
                let total = request
                    .repository_id
                    .as_ref()
                    .map(|id| snapshot.totals.get(id).copied().unwrap_or(0))
                    .unwrap_or_else(|| snapshot.totals.values().sum());
                match vector {
                    Ok(vector) => {
                        response.query_vector_cached = vector.cached;
                        response.embedding_input_tokens = vector.input_tokens;
                        response.embedding_latency_ms = vector.latency_ms;
                        response.query_usage_reported = vector.usage_reported;
                        response.remote_requests = usize::from(!vector.cached);
                        let (semantic, compatible) = rank(
                            &snapshot.entries,
                            request.repository_id.as_deref(),
                            &vector.vector,
                            limit.max(20),
                        );
                        response.pending_embeddings = total - compatible;
                        let literal_match = keywords.has_literals()
                            && snapshot
                                .local
                                .iter()
                                .any(|e| in_scope(&e.memory) && keywords.literal_match(&e.text));
                        if compatible == 0 {
                            response.results = keyword_rank(
                                &snapshot,
                                request.repository_id.as_deref(),
                                &keywords,
                                limit,
                                false,
                            );
                            response.fallback_reason =
                                Some("查询向量维数与当前记忆索引不一致，已使用本地关键词".into());
                        } else {
                            let lexical = if literal_match || response.pending_embeddings > 0 {
                                keyword_rank(
                                    &snapshot,
                                    request.repository_id.as_deref(),
                                    &keywords,
                                    limit.max(20),
                                    !literal_match,
                                )
                            } else {
                                Vec::new()
                            };
                            if lexical.is_empty() {
                                response.results = semantic.into_iter().take(limit).collect();
                                response.search_mode = "semantic".into();
                                response.score_kind = "cosine".into();
                            } else {
                                response.results = fuse(semantic, lexical, limit);
                                response.search_mode = "hybrid".into();
                                response.score_kind = "rrf".into();
                            }
                        }
                    }
                    Err(error) => {
                        response.results = keyword_rank(
                            &snapshot,
                            request.repository_id.as_deref(),
                            &keywords,
                            limit,
                            false,
                        );
                        response.pending_embeddings = total
                            - snapshot
                                .entries
                                .iter()
                                .filter(|e| in_scope(&e.memory))
                                .count();
                        response.fallback_reason = Some(error.message);
                        response.embedding_input_tokens = error.input_tokens;
                        response.embedding_latency_ms = error.latency_ms;
                        response.query_usage_reported = error.usage_reported;
                        response.remote_requests = error.remote_requests;
                    }
                }
                for hit in &mut response.results {
                    if let Some(entry) =
                        snapshot.local.iter().find(|e| e.memory.id == hit.memory.id)
                    {
                        let annotated =
                            local_hit(entry, hit.score, &snapshot.groups, Some(&keywords));
                        hit.group_count = annotated.group_count;
                        hit.matched_sections = annotated.matched_sections;
                        hit.matches_history_only = annotated.matches_history_only;
                    }
                }
                response.local_search_ms =
                    initial_ms + local_started.elapsed().as_secs_f64() * 1000.0;
                self.record_search(&query, &response)?;
                return Ok(response);
            }
        }
        response.local_search_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.record_search(&query, &response)?;
        Ok(response)
    }

    fn record_search(&self, query: &str, result: &SearchResponse) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|e| e.to_string())?
            .db
            .execute(
                "INSERT INTO search_events VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    blake3::hash(query.as_bytes()).to_hex().as_str(),
                    result.search_mode,
                    result.query_vector_cached,
                    result.remote_requests as i64,
                    result.embedding_input_tokens as i64,
                    result.query_usage_reported,
                    result.embedding_latency_ms as i64,
                    result.local_search_ms,
                    result.fallback_reason
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn snapshot(&self, key: &str) -> Result<Arc<IndexSnapshot>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let pipeline_version: i64 = state
            .db
            .query_row("PRAGMA main.data_version", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let memory_version: i64 = state
            .db
            .query_row("PRAGMA memory_index.data_version", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if let Some(snapshot) = &state.snapshot {
            if snapshot.service_key == key
                && snapshot.pipeline_version == pipeline_version
                && snapshot.memory_version == memory_version
            {
                return Ok(snapshot.clone());
            }
        }
        let mut statement = state
            .db
            .prepare(
                "SELECT m.id,m.repository_id,m.title,m.summary,m.kind,m.tags,m.updated_at,m.path,
                e.status,e.service_key,e.input_hash,e.vector,e.dimensions,m.markdown,i.content,e.error
            FROM memory_index.memories m LEFT JOIN embeddings e ON e.memory_id=m.id
            LEFT JOIN memory_index.memory_inputs i ON i.memory_id=m.id",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        let mut entries = Vec::new();
        let mut totals = BTreeMap::new();
        let mut local = Vec::new();
        let mut groups = BTreeMap::new();
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let repository_id: String = row.get(1).map_err(|error| error.to_string())?;
            *totals.entry(repository_id.clone()).or_insert(0) += 1;
            let status: Option<String> = row.get(8).map_err(|error| error.to_string())?;
            let indexed_service: Option<String> = row.get(9).map_err(|error| error.to_string())?;
            let title: String = row.get(2).map_err(|error| error.to_string())?;
            let summary: String = row.get(3).map_err(|error| error.to_string())?;
            let tags_json: String = row.get(5).map_err(|error| error.to_string())?;
            let tags: Vec<String> =
                serde_json::from_str(&tags_json).map_err(|error| error.to_string())?;
            let markdown: String = row.get(13).map_err(|error| error.to_string())?;
            let content: Option<String> = row.get(14).map_err(|error| error.to_string())?;
            let content =
                content.unwrap_or_else(|| format!("{title}\n{summary}\n{}", tags.join(" ")));
            let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
            let indexed_hash: Option<String> = row.get(10).map_err(|error| error.to_string())?;
            let valid = status.as_deref() == Some("ready")
                && indexed_service.as_deref() == Some(key)
                && indexed_hash.as_deref() == Some(hash.as_str());
            let group = group_key(&repository_id, &hash);
            *groups.entry(group.clone()).or_insert(0) += 1;
            let memory = MemorySummary {
                id: row.get(0).map_err(|e| e.to_string())?,
                repository_id,
                title,
                summary,
                kind: row.get(4).map_err(|e| e.to_string())?,
                tags,
                updated_at: row.get(6).map_err(|e| e.to_string())?,
                path: row.get(7).map_err(|e| e.to_string())?,
                embedding_status: if valid {
                    "ready".into()
                } else if status.as_deref() == Some("ready") {
                    "pending".into()
                } else {
                    status.unwrap_or("unconfigured".into())
                },
                embedding_error: row.get(15).map_err(|e| e.to_string())?,
            };
            let sections = memory_markdown::section_ranges(&markdown)
                .into_iter()
                .filter(|s| s.info.level == 2)
                .map(|s| (s.info.title, markdown[s.body_start..s.end].to_lowercase()))
                .collect();
            local.push(LocalMemory {
                title: memory.title.to_lowercase(),
                text: format!("{}\n{content}\n{markdown}", memory.id).to_lowercase(),
                current: content.to_lowercase(),
                memory: memory.clone(),
                group: group.clone(),
                sections,
            });
            if !valid {
                continue;
            }
            let bytes: Option<Vec<u8>> = row.get(11).map_err(|error| error.to_string())?;
            let dimensions: Option<i64> = row.get(12).map_err(|error| error.to_string())?;
            let Some(vector) = bytes
                .zip(dimensions)
                .and_then(|(bytes, dimensions)| decode_vector(&bytes, dimensions).ok())
            else {
                continue;
            };
            entries.push(IndexedMemory {
                memory,
                vector,
                group,
            });
        }
        drop(rows);
        drop(statement);
        let snapshot = Arc::new(IndexSnapshot {
            service_key: key.into(),
            pipeline_version,
            memory_version,
            entries,
            totals,
            local,
            groups,
        });
        state.snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    async fn query_vector(
        &self,
        query: &str,
        service_key: &str,
        credentials: impl FnOnce() -> Result<ServiceCredentials, String>,
    ) -> Result<QueryVector, QueryFailure> {
        let mut credentials = Some(credentials);
        let key = blake3::hash(format!("{service_key}\n{query}").as_bytes())
            .to_hex()
            .to_string();
        let owner = uuid::Uuid::new_v4().to_string();
        loop {
            match self.claim_query(&key, &owner)? {
                QueryCache::Ready(vector) => {
                    return Ok(QueryVector {
                        vector,
                        cached: true,
                        input_tokens: 0,
                        latency_ms: 0,
                        usage_reported: true,
                    })
                }
                QueryCache::Claimed => {
                    let service = match credentials.take().unwrap()() {
                        Ok(service) => service,
                        Err(message) => {
                            self.state
                                .lock()
                                .map_err(|e| e.to_string())?
                                .db
                                .execute(
                                    "DELETE FROM query_vectors WHERE cache_key=?1 AND owner=?2",
                                    params![key, owner],
                                )
                                .map_err(|e| e.to_string())?;
                            return Err(message.into());
                        }
                    };
                    let started = Instant::now();
                    let result = model_api::embed(&service, query).await;
                    match result {
                        Ok(result) => {
                            let vector = normalize_vector(result.vector);
                            let mut state = self.state.lock().map_err(|error| error.to_string())?;
                            let transaction =
                                state.db.transaction().map_err(|error| error.to_string())?;
                            if let Ok(vector) = &vector {
                                let bytes: Vec<u8> = vector
                                    .iter()
                                    .flat_map(|value| value.to_le_bytes())
                                    .collect();
                                transaction.execute(
                                    "UPDATE query_vectors SET vector=?3,dimensions=?4 WHERE cache_key=?1 AND owner=?2",
                                    params![key,owner,bytes,vector.len() as i64],
                                ).map_err(|error| error.to_string())?;
                            } else {
                                transaction
                                    .execute(
                                        "DELETE FROM query_vectors WHERE cache_key=?1 AND owner=?2",
                                        params![key, owner],
                                    )
                                    .map_err(|error| error.to_string())?;
                            }
                            transaction
                                .execute(
                                    "INSERT OR IGNORE INTO api_usage VALUES(?1,?2,?3)",
                                    params![
                                        owner,
                                        result.usage.input_tokens as i64,
                                        result.latency_ms as i64
                                    ],
                                )
                                .map_err(|error| error.to_string())?;
                            if !result.usage.reported {
                                transaction.execute("INSERT OR IGNORE INTO meta VALUES('embedding_usage_incomplete','1')", []).map_err(|error| error.to_string())?;
                            }
                            transaction.commit().map_err(|error| error.to_string())?;
                            return Ok(QueryVector {
                                vector: vector.map_err(|message| QueryFailure {
                                    message,
                                    input_tokens: result.usage.input_tokens,
                                    latency_ms: result.latency_ms,
                                    usage_reported: result.usage.reported,
                                    remote_requests: 1,
                                })?,
                                cached: false,
                                input_tokens: result.usage.input_tokens,
                                latency_ms: result.latency_ms,
                                usage_reported: result.usage.reported,
                            });
                        }
                        Err(error) => {
                            let state = self.state.lock().map_err(|error| error.to_string())?;
                            state
                                .db
                                .execute(
                                    "DELETE FROM query_vectors WHERE cache_key=?1 AND owner=?2",
                                    params![key, owner],
                                )
                                .map_err(|error| error.to_string())?;
                            state.db.execute("INSERT OR IGNORE INTO meta VALUES('embedding_usage_incomplete','1')", []).map_err(|error| error.to_string())?;
                            return Err(QueryFailure {
                                message: error,
                                input_tokens: 0,
                                latency_ms: started.elapsed().as_millis() as u64,
                                usage_reported: false,
                                remote_requests: 1,
                            });
                        }
                    }
                }
                QueryCache::Waiting => tokio::time::sleep(Duration::from_millis(75)).await,
            }
        }
    }

    fn claim_query(&self, key: &str, owner: &str) -> Result<QueryCache, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as i64;
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let transaction = state.db.transaction().map_err(|error| error.to_string())?;
        // The API client times out after 90 seconds, so a 120-second claim can be recovered after process exit.
        transaction
            .execute(
                "INSERT INTO query_vectors(cache_key,owner,expires_at) VALUES(?1,?2,?3)
            ON CONFLICT(cache_key) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at
            WHERE query_vectors.vector IS NULL AND query_vectors.expires_at<?4",
                params![key, owner, now + 120_000, now],
            )
            .map_err(|error| error.to_string())?;
        let (holder, bytes, dimensions): (String, Option<Vec<u8>>, Option<i64>) = transaction
            .query_row(
                "SELECT owner,vector,dimensions FROM query_vectors WHERE cache_key=?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        match bytes.zip(dimensions) {
            Some((bytes, dimensions)) => Ok(QueryCache::Ready(decode_vector(&bytes, dimensions)?)),
            None if holder == owner => Ok(QueryCache::Claimed),
            None => Ok(QueryCache::Waiting),
        }
    }
}

fn validate_request(request: &SearchRequest) -> Result<(String, usize), String> {
    let query = request.query.trim();
    if request.group_of.is_some() && (!query.is_empty() || request.mode != SearchMode::Auto) {
        return Err("groupOf 与 query、mode 互斥".into());
    }
    if request.group_of.is_none() && request.offset.is_some() {
        return Err("offset 仅用于分组分页".into());
    }
    if request.group_of.is_none() && query.is_empty() {
        return Err("请输入检索内容".into());
    }
    let limit = request.limit.unwrap_or(5);
    if !(1..=20).contains(&limit) {
        return Err("检索条数应在 1 到 20 之间".into());
    }
    Ok((query.into(), limit))
}

fn decode_vector(bytes: &[u8], dimensions: i64) -> Result<Vec<f32>, String> {
    if dimensions <= 0 || bytes.len() != dimensions as usize * 4 {
        return Err("索引中的向量维数无效".into());
    }
    normalize_vector(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect(),
    )
}

fn normalize_vector(mut vector: Vec<f32>) -> Result<Vec<f32>, String> {
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err("向量服务返回了无法计算相似度的向量".into());
    }
    for value in &mut vector {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(vector)
}

fn rank(
    entries: &[IndexedMemory],
    repository: Option<&str>,
    query: &[f32],
    limit: usize,
) -> (Vec<SearchHit>, usize) {
    let mut groups: BTreeMap<&str, (&IndexedMemory, f64)> = BTreeMap::new();
    let mut compatible = 0;
    for entry in entries {
        if repository.is_some_and(|id| entry.memory.repository_id != id)
            || entry.vector.len() != query.len()
        {
            continue;
        }
        compatible += 1;
        let score = f64::from(dot(&entry.vector, query)).clamp(-1.0, 1.0);
        let best = groups.entry(&entry.group).or_insert((entry, score));
        if score > best.1 || score == best.1 && entry.memory.id < best.0.memory.id {
            *best = (entry, score);
        }
    }
    let mut best = groups.into_values().collect::<Vec<_>>();
    best.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.memory.id.cmp(&b.0.memory.id))
    });
    (
        best.into_iter()
            .take(limit)
            .map(|(entry, score)| SearchHit {
                memory: entry.memory.clone(),
                score,
                group_count: 1,
                matched_sections: Vec::new(),
                matches_history_only: false,
                group: entry.group.clone(),
            })
            .collect(),
        compatible,
    )
}

struct QueryFailure {
    message: String,
    input_tokens: u64,
    latency_ms: u64,
    usage_reported: bool,
    remote_requests: usize,
}

impl From<String> for QueryFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            input_tokens: 0,
            latency_ms: 0,
            usage_reported: true,
            remote_requests: 0,
        }
    }
}

fn group_key(repository: &str, hash: &str) -> String {
    format!("{repository}\n{hash}")
}

fn local_hit(
    entry: &LocalMemory,
    score: f64,
    groups: &BTreeMap<String, usize>,
    query: Option<&KeywordQuery>,
) -> SearchHit {
    let matched_sections = query
        .map(|q| {
            entry
                .sections
                .iter()
                .filter(|(_, text)| q.score("", text) > 0)
                .map(|(title, _)| MatchedSection {
                    title: title.clone(),
                    status: match title.as_str() {
                        "此前结论" | "变更记录" | "更新日志" => "historical",
                        "待采用建议" => "suggested",
                        _ => "current",
                    }
                    .into(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let matches_history_only = query.is_some_and(|q| q.score(&entry.title, &entry.current) == 0)
        && !matched_sections.is_empty()
        && matched_sections.iter().all(|s| s.status != "current");
    SearchHit {
        memory: entry.memory.clone(),
        score,
        group_count: groups[&entry.group],
        matched_sections,
        matches_history_only,
        group: entry.group.clone(),
    }
}

fn keyword_rank(
    snapshot: &IndexSnapshot,
    repository: Option<&str>,
    query: &KeywordQuery,
    limit: usize,
    pending_only: bool,
) -> Vec<SearchHit> {
    let mut groups: BTreeMap<&str, (&LocalMemory, usize)> = BTreeMap::new();
    for entry in &snapshot.local {
        if repository.is_some_and(|id| entry.memory.repository_id != id)
            || pending_only && entry.memory.embedding_status == "ready"
        {
            continue;
        }
        let score = query.score(&entry.title, &entry.text);
        if score == 0 {
            continue;
        }
        let best = groups.entry(&entry.group).or_insert((entry, score));
        if score > best.1 || score == best.1 && entry.memory.id < best.0.memory.id {
            *best = (entry, score);
        }
    }
    let mut ranked = groups.into_values().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.memory.id.cmp(&b.0.memory.id))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(entry, score)| local_hit(entry, score as f64, &snapshot.groups, Some(query)))
        .collect()
}

fn fuse(semantic: Vec<SearchHit>, lexical: Vec<SearchHit>, limit: usize) -> Vec<SearchHit> {
    let mut hits: BTreeMap<String, (SearchHit, usize)> = BTreeMap::new();
    for (rank, mut hit) in semantic.into_iter().enumerate() {
        hit.score = 1.0 / (61 + rank) as f64;
        hits.insert(hit.group.clone(), (hit, rank));
    }
    for (rank, mut hit) in lexical.into_iter().enumerate() {
        hit.score = 1.0 / (61 + rank) as f64;
        if let Some((existing, best_rank)) = hits.get_mut(&hit.group) {
            let score = existing.score + hit.score;
            if rank < *best_rank || rank == *best_rank && hit.memory.id < existing.memory.id {
                *existing = hit;
                *best_rank = rank;
            }
            existing.score = score;
        } else {
            hits.insert(hit.group.clone(), (hit, rank));
        }
    }
    let mut hits = hits.into_values().map(|(hit, _)| hit).collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.memory.id.cmp(&b.memory.id))
    });
    hits.truncate(limit);
    hits
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    let mut sums = [0.0; 4];
    let mut left_chunks = left.chunks_exact(4);
    let mut right_chunks = right.chunks_exact(4);
    for (left, right) in left_chunks.by_ref().zip(right_chunks.by_ref()) {
        sums[0] += left[0] * right[0];
        sums[1] += left[1] * right[1];
        sums[2] += left[2] * right[2];
        sums[3] += left[3] * right[3];
    }
    let remainder = left_chunks
        .remainder()
        .iter()
        .zip(right_chunks.remainder())
        .map(|(left, right)| left * right)
        .sum::<f32>();
    sums.into_iter().sum::<f32>() + remainder
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod performance_tests;
