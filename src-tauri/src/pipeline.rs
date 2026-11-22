use crate::{
    codex::{CodexTurn, RolloutCursor},
    model_api::{ArchiveTopic, ChatExtraction, EmbeddingResult},
    storage::{
        normalize_remote, repository_id, ArchiveSource, KeywordQuery, MemoryInput, MemorySummary,
        RepositoryInput, Vault,
    },
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveTurn {
    pub source_id: String,
    pub repository: String,
    pub session_id: String,
    pub turn_id: String,
    pub completed_at: String,
    pub status: String,
    pub memory_count: usize,
    pub chat_input_tokens: u64,
    pub chat_output_tokens: u64,
    pub embedding_input_tokens: u64,
    pub chat_latency_ms: Option<u64>,
    pub embedding_latency_ms: Option<u64>,
    pub error: Option<String>,
    pub usage_incomplete: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveStatus {
    pub enabled: bool,
    pub started_at: Option<String>,
    pub scanning: bool,
    pub processed_turns: usize,
    pub written_memories: usize,
    pub memory_writes: usize,
    pub pending_embeddings: usize,
    pub failed_embeddings: usize,
    pub failed_embedding_details: Vec<FailedEmbedding>,
    pub processing_embeddings: usize,
    pub chat_input_tokens: u64,
    pub chat_output_tokens: u64,
    pub embedding_input_tokens: u64,
    pub recent_turns: Vec<ArchiveTurn>,
    pub last_error: Option<String>,
    pub usage_incomplete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedEmbedding {
    pub memory_id: String,
    pub error: Option<String>,
}

pub struct Pipeline {
    pub root: PathBuf,
    db: Connection,
}

#[derive(Clone)]
pub struct EmbeddingJob {
    pub request_id: String,
    pub source_id: Option<String>,
    pub memory_id: String,
    pub input_hash: String,
    pub service_key: String,
    pub content: String,
}

impl Pipeline {
    pub fn open(root: &Path) -> Result<Self, String> {
        let db =
            Connection::open(root.join(".aimemory/pipeline.sqlite")).map_err(|e| e.to_string())?;
        db.execute(
            "ATTACH DATABASE ?1 AS memory_index",
            [root
                .join(".aimemory/index.sqlite")
                .to_string_lossy()
                .as_ref()],
        )
        .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS cursors (path TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS turns (
                source_id TEXT PRIMARY KEY, repository TEXT NOT NULL, session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL, completed_at TEXT NOT NULL, status TEXT NOT NULL,
                turn_json TEXT NOT NULL, extraction TEXT, memory_count INTEGER NOT NULL DEFAULT 0,
                chat_input INTEGER NOT NULL DEFAULT 0, chat_output INTEGER NOT NULL DEFAULT 0,
                embedding_input INTEGER NOT NULL DEFAULT 0, chat_latency INTEGER,
                embedding_latency INTEGER, error TEXT, usage_incomplete INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS archive_memories (memory_id TEXT PRIMARY KEY, source_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS embeddings (
                memory_id TEXT PRIMARY KEY, input_hash TEXT NOT NULL, service_key TEXT NOT NULL,
                content TEXT NOT NULL, source_id TEXT, status TEXT NOT NULL, vector BLOB,
                dimensions INTEGER, error TEXT
            );
            CREATE INDEX IF NOT EXISTS embeddings_status ON embeddings(status);
            CREATE TABLE IF NOT EXISTS api_usage (request_id TEXT PRIMARY KEY, input_tokens INTEGER NOT NULL, latency INTEGER NOT NULL);
            ")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.to_path_buf(),
            db,
        })
    }

    pub fn recover_interrupted(&self) -> Result<(), String> {
        self.db.execute_batch("INSERT OR IGNORE INTO meta SELECT 'unknown_usage:'||source_id,'1' FROM turns WHERE status='extracting';
            UPDATE turns SET status='failed',error='应用在提取请求期间退出，可重试此轮',usage_incomplete=1 WHERE status='extracting';
            INSERT OR IGNORE INTO meta SELECT 'embedding_usage_incomplete','1' FROM embeddings WHERE status='processing' LIMIT 1;
            UPDATE embeddings SET status='failed',error='应用在向量请求期间退出，可单独重试' WHERE status='processing';").map_err(|e|e.to_string())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.db
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn has_in_flight_requests(&self) -> Result<bool, String> {
        self.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE status='extracting')
                    OR EXISTS(SELECT 1 FROM embeddings WHERE status='processing')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.db.execute("INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key,value]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn cursors(&self) -> Result<BTreeMap<PathBuf, RolloutCursor>, String> {
        let mut statement = self
            .db
            .prepare("SELECT path,value FROM cursors")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut cursors = BTreeMap::new();
        for row in rows {
            let (path, value) = row.map_err(|e| e.to_string())?;
            cursors.insert(
                PathBuf::from(path),
                serde_json::from_str(&value).map_err(|e| format!("归档读取位置格式错误：{e}"))?,
            );
        }
        Ok(cursors)
    }

    pub fn baseline(
        &mut self,
        directory: &str,
        enabled_at: &str,
        cursors: &[(PathBuf, RolloutCursor)],
    ) -> Result<(), String> {
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        transaction
            .execute("DELETE FROM cursors", [])
            .map_err(|e| e.to_string())?;
        for (path, cursor) in cursors {
            transaction
                .execute(
                    "INSERT INTO cursors VALUES(?1,?2)",
                    params![
                        path.display().to_string(),
                        serde_json::to_string(cursor).map_err(|e| e.to_string())?
                    ],
                )
                .map_err(|e| e.to_string())?;
        }
        for (key, value) in [("codex_directory", directory), ("enabled_at", enabled_at)] {
            transaction.execute("INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key,value]).map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }

    pub fn ingest(
        &mut self,
        path: &Path,
        cursor: &RolloutCursor,
        turns: &[CodexTurn],
    ) -> Result<(), String> {
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        for turn in turns {
            let (status, error) = if turn.repository_remote.is_none() {
                ("skipped", Some("该轮工作目录没有 Git 远程仓库"))
            } else {
                ("pending", None)
            };
            transaction.execute("INSERT OR IGNORE INTO turns(source_id,repository,session_id,turn_id,completed_at,status,turn_json,error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![turn.source_id,turn.repository_name,turn.session_id,turn.turn_id,turn.completed_at,status,serde_json::to_string(turn).map_err(|e| e.to_string())?,error]).map_err(|e| e.to_string())?;
        }
        transaction.execute("INSERT INTO cursors VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET value=excluded.value", params![path.display().to_string(),serde_json::to_string(cursor).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())
    }

    pub fn next_turn(&self) -> Result<Option<CodexTurn>, String> {
        let json:Option<String> = self.db.query_row("SELECT turn_json FROM turns WHERE status='pending' AND extraction IS NULL ORDER BY completed_at, rowid LIMIT 1", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        json.map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    }

    pub fn archive_topics(&self, turn: &CodexTurn) -> Result<Vec<ArchiveTopic>, String> {
        let remote = turn
            .repository_remote
            .as_deref()
            .ok_or("该轮缺少 Git 远程仓库")?;
        let repository = repository_id(&normalize_remote(remote)?);
        let mut statement = self.db.prepare(
            "SELECT m.id, m.title, m.summary, m.tags, m.markdown,
                    EXISTS(SELECT 1 FROM memory_index.memory_sources s WHERE s.memory_id=m.id AND s.session_id=?2)
             FROM memory_index.memories m
             WHERE m.repository_id=?1 AND EXISTS(SELECT 1 FROM memory_index.memory_sources s WHERE s.memory_id=m.id)
             ORDER BY (instr(lower(?3), lower(m.title)) > 0) DESC,
                      EXISTS(SELECT 1 FROM json_each(m.tags) tag WHERE length(tag.value)>1 AND instr(lower(?3),lower(tag.value))>0) DESC,
                      6 DESC,m.updated_at DESC,m.id"
        ).map_err(|e| e.to_string())?;
        let keywords = KeywordQuery::new(&turn.user_text);
        let rows = statement
            .query_map(
                params![repository, turn.session_id, turn.user_text],
                |row| {
                    let topic = ArchiveTopic {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        summary: row.get(2)?,
                    };
                    let text = format!(
                        "{} {} {}",
                        topic.summary,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?
                    )
                    .to_lowercase();
                    let score = keywords.score(&topic.title.to_lowercase(), &text);
                    Ok((topic, score))
                },
            )
            .map_err(|e| e.to_string())?;
        let rows = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut lexical = rows
            .iter()
            .filter(|(_, score)| *score > 0)
            .collect::<Vec<_>>();
        lexical.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        let mut topics = lexical
            .into_iter()
            .take(8)
            .map(|(topic, _)| topic.clone())
            .collect::<Vec<_>>();
        for (topic, _) in rows {
            if topics.len() == 16 {
                break;
            }
            if !topics.iter().any(|t| t.id == topic.id) {
                topics.push(topic);
            }
        }
        Ok(topics)
    }

    pub fn mark_extracting(&self, source_id: &str) -> Result<(), String> {
        self.db.execute("UPDATE turns SET status='extracting',error=NULL,usage_incomplete=1 WHERE source_id=?1", [source_id]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn extraction_complete(
        &self,
        source_id: &str,
        extraction: &ChatExtraction,
    ) -> Result<(), String> {
        let incomplete = !extraction.usage.reported
            || self.meta(&format!("unknown_usage:{source_id}"))?.is_some();
        self.db.execute("UPDATE turns SET extraction=?2,status='pending',chat_input=chat_input+?3,chat_output=chat_output+?4,chat_latency=?5,error=NULL,usage_incomplete=?6 WHERE source_id=?1 AND extraction IS NULL", params![source_id,serde_json::to_string(extraction).map_err(|e| e.to_string())?,extraction.usage.input_tokens as i64,extraction.usage.output_tokens as i64,extraction.latency_ms as i64,incomplete]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn extraction_rejected(
        &self,
        source_id: &str,
        error: &crate::model_api::ChatError,
    ) -> Result<(), String> {
        if let Some(usage) = &error.usage {
            self.db.execute("UPDATE turns SET chat_input=chat_input+?2,chat_output=chat_output+?3,chat_latency=?4,usage_incomplete=?5 WHERE source_id=?1 AND extraction IS NULL",params![source_id,usage.input_tokens as i64,usage.output_tokens as i64,error.latency_ms as i64,!usage.reported || self.meta(&format!("unknown_usage:{source_id}"))?.is_some()]).map_err(|e|e.to_string())?;
            self.turn_failed(source_id, &error.message)
        } else {
            self.extraction_failed(source_id, &error.message)
        }
    }

    pub fn extraction_failed(&self, source_id: &str, error: &str) -> Result<(), String> {
        self.set_meta(&format!("unknown_usage:{source_id}"), "1")?;
        self.turn_failed(source_id, error)
    }

    pub fn turn_failed(&self, source_id: &str, error: &str) -> Result<(), String> {
        self.db
            .execute(
                "UPDATE turns SET status='failed',error=?2 WHERE source_id=?1",
                [source_id, error],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn materialize(&self, vault: &mut Vault) -> Result<bool, String> {
        let mut statement = self.db.prepare("SELECT turn_json,extraction FROM turns WHERE extraction IS NOT NULL AND status='pending' ORDER BY completed_at").map_err(|e| e.to_string())?;
        let records = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut changed = false;
        for (turn, extraction) in records {
            let turn: CodexTurn = serde_json::from_str(&turn).map_err(|e| e.to_string())?;
            let extraction: ChatExtraction =
                serde_json::from_str(&extraction).map_err(|e| e.to_string())?;
            changed |= !extraction.memories.is_empty();
            let result = self.write_memories(vault, &turn, &extraction);
            match result {
                Ok(count) => {
                    self.db.execute("UPDATE turns SET status='saved',memory_count=?2,error=NULL WHERE source_id=?1", params![turn.source_id,count as i64]).map_err(|e| e.to_string())?;
                }
                Err(error) => self.turn_failed(&turn.source_id, &error)?,
            }
        }
        Ok(changed)
    }

    fn write_memories(
        &self,
        vault: &mut Vault,
        turn: &CodexTurn,
        extraction: &ChatExtraction,
    ) -> Result<usize, String> {
        if extraction.memories.is_empty() {
            return Ok(0);
        }
        let repository = vault.add_repository(RepositoryInput {
            name: turn.repository_name.clone(),
            remote: turn
                .repository_remote
                .clone()
                .ok_or("该轮缺少 Git 远程仓库")?,
        })?;
        let source = ArchiveSource {
            source_id: turn.source_id.clone(),
            session_id: turn.session_id.clone(),
            turn_id: turn.turn_id.clone(),
            input_truncated: extraction.input_truncated,
            message_role: None,
        };
        for (index, memory) in extraction.memories.iter().enumerate() {
            let mut source = source.clone();
            source.message_role = Some(
                if turn.user_text.contains(&memory.evidence) {
                    "user"
                } else {
                    "assistant"
                }
                .into(),
            );
            if let Some(id) = &memory.target_id {
                let baseline = self.archive_baseline(id)?;
                vault.extend_archive(id, &repository.id, memory, &source, baseline.as_deref())?;
                self.db
                    .execute(
                        "UPDATE archive_memories SET source_id=?2 WHERE memory_id=?1",
                        params![id, turn.source_id],
                    )
                    .map_err(|e| e.to_string())?;
                continue;
            }
            let id = format!(
                "ARC-{}",
                &blake3::hash(format!("{}:{index}", turn.source_id).as_bytes()).to_hex()[..24]
            );
            let quote = memory
                .evidence
                .split('\n')
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            let markdown = format!(
                "## 结论\n\n{}\n\n## 详细说明\n\n{}\n\n## 原文依据\n\n{quote}\n",
                memory.summary, memory.details
            );
            vault.save_archive(
                &id,
                MemoryInput {
                    id: None,
                    repository_id: repository.id.clone(),
                    title: memory.title.clone(),
                    kind: memory.kind.clone(),
                    tags: memory.tags.clone(),
                    markdown,
                    related_ids: Vec::new(),
                    revision: None,
                    source: Some(format!(
                        "Codex · 会话 {} · 轮次 {}",
                        turn.session_id, turn.turn_id
                    )),
                },
                &source,
            )?;
            self.db
                .execute(
                    "INSERT OR IGNORE INTO archive_memories VALUES(?1,?2)",
                    params![id, turn.source_id],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(extraction.memories.len())
    }

    fn archive_baseline(&self, id: &str) -> Result<Option<String>, String> {
        let mut statement = self.db.prepare("SELECT t.source_id,t.extraction FROM turns t JOIN archive_memories a ON a.source_id=t.source_id WHERE a.memory_id=?1 AND t.extraction IS NOT NULL AND t.status='saved'").map_err(|e|e.to_string())?;
        let rows = statement
            .query_map([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (source, json) = row.map_err(|e| e.to_string())?;
            let extraction: ChatExtraction =
                serde_json::from_str(&json).map_err(|e| e.to_string())?;
            let matching = extraction
                .memories
                .iter()
                .enumerate()
                .filter(|(index, m)| {
                    m.target_id.as_deref() == Some(id)
                        || m.target_id.is_none()
                            && format!(
                                "ARC-{}",
                                &blake3::hash(format!("{source}:{index}").as_bytes()).to_hex()
                                    [..24]
                            ) == id
                })
                .map(|(_, m)| &m.summary)
                .collect::<Vec<_>>();
            if let [summary] = matching.as_slice() {
                return Ok(Some((*summary).clone()));
            }
        }
        Ok(None)
    }

    pub fn sync_embeddings(
        &self,
        _vault: &Vault,
        service_key: &str,
        configured: bool,
    ) -> Result<(), String> {
        let mut statement = self.db.prepare("SELECT m.id,i.content FROM memory_index.memories m JOIN memory_index.memory_inputs i ON i.memory_id=m.id").map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, content) = row.map_err(|e| e.to_string())?;
            self.sync_embedding_content(&id, &content, service_key, configured)?;
        }
        self.db.execute("DELETE FROM embeddings WHERE NOT EXISTS(SELECT 1 FROM memory_index.memories m WHERE m.id=embeddings.memory_id)",[]).map_err(|e|e.to_string())?;
        Ok(())
    }

    pub fn sync_memory_embedding(
        &self,
        id: &str,
        service_key: &str,
        configured: bool,
    ) -> Result<(), String> {
        let content: String = self
            .db
            .query_row(
                "SELECT content FROM memory_index.memory_inputs WHERE memory_id=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        self.sync_embedding_content(id, &content, service_key, configured)
    }

    fn sync_embedding_content(
        &self,
        id: &str,
        content: &str,
        service_key: &str,
        configured: bool,
    ) -> Result<(), String> {
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        self.db.execute("INSERT INTO embeddings(memory_id,input_hash,service_key,content,source_id,status) VALUES(?1,?2,?3,?4,(SELECT source_id FROM archive_memories WHERE memory_id=?1),?5)
            ON CONFLICT(memory_id) DO UPDATE SET input_hash=excluded.input_hash,service_key=excluded.service_key,content=excluded.content,source_id=COALESCE(excluded.source_id,embeddings.source_id),status=excluded.status,vector=NULL,dimensions=NULL,error=NULL
            WHERE embeddings.input_hash<>excluded.input_hash OR embeddings.service_key<>excluded.service_key OR (embeddings.status='unconfigured' AND excluded.status='pending')",
            params![id,hash,service_key,content,if configured {"pending"} else {"unconfigured"}]).map_err(|e|e.to_string())?;
        Ok(())
    }

    pub fn next_embedding(&self) -> Result<Option<EmbeddingJob>, String> {
        let job=self.db.query_row("SELECT memory_id,input_hash,service_key,content,source_id FROM embeddings WHERE status='pending' ORDER BY rowid LIMIT 1",[],|r|Ok(EmbeddingJob {request_id:uuid::Uuid::new_v4().to_string(),memory_id:r.get(0)?,input_hash:r.get(1)?,service_key:r.get(2)?,content:r.get(3)?,source_id:r.get(4)?})).optional().map_err(|e| e.to_string())?;
        if let Some(job) = &job {
            self.db
                .execute(
                    "UPDATE embeddings SET status='processing' WHERE memory_id=?1",
                    [&job.memory_id],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(job)
    }

    pub fn embedding_complete(
        &mut self,
        job: &EmbeddingJob,
        result: Result<EmbeddingResult, String>,
    ) -> Result<(), String> {
        match result {
            Ok(result) => {
                let vector: Vec<u8> = result.vector.iter().flat_map(|n| n.to_le_bytes()).collect();
                let current: Option<String> = self
                    .db
                    .query_row(
                        "SELECT content FROM memory_index.memory_inputs WHERE memory_id=?1",
                        [&job.memory_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let transaction = self.db.transaction().map_err(|e| e.to_string())?;
                let inserted = transaction
                    .execute(
                        "INSERT OR IGNORE INTO api_usage VALUES(?1,?2,?3)",
                        params![
                            job.request_id,
                            result.usage.input_tokens as i64,
                            result.latency_ms as i64
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                if inserted > 0 {
                    if let Some(content) = current {
                        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
                        if hash != job.input_hash {
                            transaction.execute("UPDATE embeddings SET content=?4,input_hash=?5,status='pending',vector=NULL,dimensions=NULL,error=NULL WHERE memory_id=?1 AND input_hash=?2 AND service_key=?3", params![job.memory_id,job.input_hash,job.service_key,content,hash]).map_err(|e|e.to_string())?;
                        }
                    }
                    transaction.execute("UPDATE embeddings SET status='ready',vector=?4,dimensions=?5,error=NULL WHERE memory_id=?1 AND input_hash=?2 AND service_key=?3 AND status='processing'",params![job.memory_id,job.input_hash,job.service_key,vector,result.vector.len() as i64]).map_err(|e|e.to_string())?;
                    transaction.execute("UPDATE turns SET embedding_input=embedding_input+?2,embedding_latency=COALESCE(embedding_latency,0)+?3,usage_incomplete=usage_incomplete OR ?4 WHERE source_id=?1",params![job.source_id,result.usage.input_tokens as i64,result.latency_ms as i64,!result.usage.reported]).map_err(|e|e.to_string())?;
                    if !result.usage.reported {
                        transaction.execute("INSERT OR IGNORE INTO meta VALUES('embedding_usage_incomplete','1')",[]).map_err(|e|e.to_string())?;
                    }
                }
                transaction.commit().map_err(|e| e.to_string())?;
            }
            Err(error) => {
                self.db.execute("UPDATE embeddings SET status='failed',error=?4 WHERE memory_id=?1 AND input_hash=?2 AND service_key=?3",params![job.memory_id,job.input_hash,job.service_key,error]).map_err(|e|e.to_string())?;
                self.set_meta("embedding_usage_incomplete", "1")?;
                self.db
                    .execute(
                        "UPDATE turns SET usage_incomplete=1 WHERE source_id=?1",
                        [&job.source_id],
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn embedding_configuration_failed(
        &self,
        service_key: &str,
        error: &str,
    ) -> Result<(), String> {
        self.db.execute("UPDATE embeddings SET status='failed',error=?2 WHERE service_key=?1 AND status IN ('pending','processing')",[service_key,error]).map_err(|e|e.to_string())?;
        Ok(())
    }

    pub fn annotate(&self, memory: &mut MemorySummary) -> Result<(), String> {
        let status: Option<(String, Option<String>)> = self
            .db
            .query_row(
                "SELECT status,error FROM embeddings WHERE memory_id=?1",
                [&memory.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((status, error)) = status {
            memory.embedding_status = status;
            memory.embedding_error = error;
        }
        Ok(())
    }

    pub fn retry_failed(&self) -> Result<(), String> {
        self.db.execute_batch("UPDATE turns SET status='pending',error=NULL WHERE status='failed'; UPDATE embeddings SET status='pending',error=NULL WHERE status='failed';").map_err(|e|e.to_string())
    }

    pub fn status(
        &self,
        enabled: bool,
        started_at: Option<String>,
        scanning: bool,
        last_error: Option<String>,
    ) -> Result<ArchiveStatus, String> {
        let (processed, written, writes, chat_in, chat_out): (i64, i64, i64, i64, i64) = self.db.query_row(
            "SELECT COALESCE(SUM(status IN ('saved','skipped')),0), (SELECT COUNT(*) FROM archive_memories),
                    COALESCE(SUM(memory_count),0), COALESCE(SUM(chat_input),0), COALESCE(SUM(chat_output),0)
             FROM turns",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).map_err(|e| e.to_string())?;
        let pending:i64=self.db.query_row("SELECT COUNT(*) FROM embeddings WHERE status IN ('pending','processing','unconfigured')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        let failed: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM embeddings WHERE status='failed'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let processing: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM embeddings WHERE status='processing'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut failures = self.db.prepare("SELECT memory_id,error FROM embeddings WHERE status='failed' ORDER BY memory_id LIMIT 30").map_err(|e| e.to_string())?;
        let failed_embedding_details = failures
            .query_map([], |row| {
                Ok(FailedEmbedding {
                    memory_id: row.get(0)?,
                    error: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut statement=self.db.prepare("SELECT source_id,repository,session_id,turn_id,completed_at,status,memory_count,chat_input,chat_output,embedding_input,chat_latency,embedding_latency,error,usage_incomplete FROM turns ORDER BY completed_at DESC LIMIT 30").map_err(|e|e.to_string())?;
        let recent_turns = statement
            .query_map([], |r| {
                Ok(ArchiveTurn {
                    source_id: r.get(0)?,
                    repository: r.get(1)?,
                    session_id: r.get(2)?,
                    turn_id: r.get(3)?,
                    completed_at: r.get(4)?,
                    status: r.get(5)?,
                    memory_count: r.get::<_, i64>(6)? as usize,
                    chat_input_tokens: r.get::<_, i64>(7)? as u64,
                    chat_output_tokens: r.get::<_, i64>(8)? as u64,
                    embedding_input_tokens: r.get::<_, i64>(9)? as u64,
                    chat_latency_ms: r.get::<_, Option<i64>>(10)?.map(|value| value as u64),
                    embedding_latency_ms: r.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                    error: r.get(12)?,
                    usage_incomplete: r.get(13)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let embedding_total: i64 = self
            .db
            .query_row(
                "SELECT COALESCE(SUM(input_tokens),0) FROM api_usage",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let incomplete: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE usage_incomplete=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let usage_incomplete = incomplete > 0 || self.meta("embedding_usage_incomplete")?.is_some();
        Ok(ArchiveStatus {
            enabled,
            started_at,
            scanning,
            processed_turns: processed as usize,
            written_memories: written as usize,
            memory_writes: writes as usize,
            pending_embeddings: pending as usize,
            failed_embeddings: failed as usize,
            failed_embedding_details,
            processing_embeddings: processing as usize,
            chat_input_tokens: chat_in as u64,
            chat_output_tokens: chat_out as u64,
            embedding_input_tokens: embedding_total as u64,
            recent_turns,
            last_error,
            usage_incomplete,
        })
    }
}

#[cfg(test)]
mod tests;
