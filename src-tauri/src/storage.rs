use crate::memory_markdown::{self, conclusion, excerpt, EvidenceSource, Suggestion};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub remote: String,
    #[serde(default)]
    pub memory_count: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInput {
    pub name: String,
    pub remote: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub root: String,
    pub repositories: Vec<Repository>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummary {
    pub id: String,
    pub repository_id: String,
    pub title: String,
    pub summary: String,
    pub kind: String,
    pub tags: Vec<String>,
    pub updated_at: String,
    pub path: String,
    pub embedding_status: String,
    pub embedding_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGraph {
    pub nodes: Vec<MemorySummary>,
    pub edges: Vec<MemoryGraphEdge>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGraphEdge {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    #[serde(flatten)]
    pub summary: MemorySummary,
    pub markdown: String,
    pub related_ids: Vec<String>,
    pub outgoing_related_ids: Vec<String>,
    pub code_paths: Vec<String>,
    pub source: Option<String>,
    pub revision: String,
    pub conclusion_owner: String,
    pub suggestions: Vec<Suggestion>,
    pub evidence_sources: Vec<EvidenceSource>,
    pub current_conclusion: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInput {
    pub id: Option<String>,
    pub repository_id: String,
    pub title: String,
    pub kind: String,
    pub tags: Vec<String>,
    pub markdown: String,
    pub related_ids: Vec<String>,
    pub revision: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchiveSource {
    pub source_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub input_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_role: Option<String>,
}

#[derive(Clone)]
struct Document {
    meta: Mapping,
    markdown: String,
    revision: String,
}

pub struct Vault {
    pub root: PathBuf,
    db: Connection,
    repositories: BTreeMap<String, Repository>,
    warnings: Vec<String>,
}

impl Vault {
    pub fn open(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| format!("无法打开记忆库：{e}"))?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let internal = root.join(".aimemory");
        fs::create_dir_all(&internal).map_err(|e| e.to_string())?;
        let repositories: Vec<Repository> =
            match fs::read_to_string(internal.join("repositories.json")) {
                Ok(json) => {
                    serde_json::from_str(&json).map_err(|e| format!("仓库配置格式错误：{e}"))?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.to_string()),
            };
        let db = Connection::open(internal.join("index.sqlite")).map_err(|e| e.to_string())?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY, repository_id TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL, summary TEXT NOT NULL, kind TEXT NOT NULL,
                tags TEXT NOT NULL, updated_at TEXT NOT NULL, markdown TEXT NOT NULL,
                source TEXT, revision TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS memories_repository ON memories(repository_id, updated_at);
            CREATE TABLE IF NOT EXISTS relations (from_id TEXT NOT NULL, to_id TEXT NOT NULL,
                PRIMARY KEY(from_id, to_id));
            CREATE INDEX IF NOT EXISTS relations_target ON relations(to_id);
            CREATE TABLE IF NOT EXISTS memory_inputs (memory_id TEXT PRIMARY KEY, content TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memory_sources (memory_id TEXT NOT NULL, source_id TEXT NOT NULL, session_id TEXT NOT NULL, PRIMARY KEY(memory_id,source_id));",
        )
        .map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS memory_code_paths (memory_id TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(memory_id,path));
            CREATE INDEX IF NOT EXISTS memory_code_path ON memory_code_paths(path);")
            .map_err(|e| e.to_string())?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if version < 1 {
            db.execute_batch("UPDATE memories SET revision=''; PRAGMA user_version=1;")
                .map_err(|e| e.to_string())?;
        }
        let mut vault = Self {
            root,
            db,
            repositories: repositories
                .into_iter()
                .map(|r| (r.id.clone(), r))
                .collect(),
            warnings: Vec::new(),
        };
        vault.refresh()?;
        Ok(vault)
    }

    pub fn workspace(&self) -> Result<Workspace, String> {
        let mut repositories: Vec<_> = self.repositories.values().cloned().collect();
        for repository in &mut repositories {
            let count: i64 = self
                .db
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE repository_id=?1",
                    [&repository.id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            repository.memory_count = count as usize;
        }
        repositories.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Workspace {
            root: self.root.display().to_string(),
            repositories,
            warnings: self.warnings.clone(),
        })
    }

    pub fn add_repository(&mut self, input: RepositoryInput) -> Result<Repository, String> {
        let name = repository_name(&input.name)?;
        let remote = normalize_remote(&input.remote)?;
        if let Some(repository) = self.repositories.values().find(|r| r.remote == remote) {
            return Ok(repository.clone());
        }
        let repository = Repository {
            id: repository_id(&remote),
            name: name.into(),
            remote,
            memory_count: 0,
        };
        fs::create_dir_all(self.root.join(&repository.id)).map_err(|e| e.to_string())?;
        self.repositories
            .insert(repository.id.clone(), repository.clone());
        self.persist_repositories()?;
        Ok(repository)
    }

    pub fn rename_repository(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = repository_name(name)?;
        let repository = self.repositories.get(id).ok_or("未找到这个仓库")?;
        if repository.name == name {
            return Ok(());
        }
        for memory in self.search(Some(id), "")? {
            let path = Path::new(&memory.path);
            let mut document = parse_document(path)?;
            set(&mut document.meta, "repository_name", &name)?;
            self.persist_document(
                &memory.id,
                path,
                &document.meta,
                &document.markdown,
                Some(&document.revision),
                &format!("仓库重命名 · {name}"),
            )?;
        }
        self.repositories.get_mut(id).unwrap().name = name.into();
        self.persist_repositories()
    }

    pub fn delete_repository(&mut self, id: &str) -> Result<(), String> {
        let repository = self.repositories.get(id).ok_or("未找到这个仓库")?;
        let memories = self.search(Some(id), "")?;
        let ids: BTreeSet<_> = memories.iter().map(|memory| memory.id.as_str()).collect();
        let trash = self.root.join(".aimemory/trash").join(format!(
            "{}-{}",
            id,
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
        write_atomic(
            &trash.join("repository.json"),
            &serde_json::to_vec_pretty(repository).map_err(|e| e.to_string())?,
        )?;
        let others = self
            .search(None, "")?
            .into_iter()
            .filter(|m| m.repository_id != id)
            .collect::<Vec<_>>();
        for memory in &others {
            let path = Path::new(&memory.path);
            let mut document = parse_document(path)?;
            let related = strings(&document.meta, "related_ids")?;
            let remaining: Vec<_> = related
                .iter()
                .filter(|target| !ids.contains(target.as_str()))
                .collect();
            if remaining.len() != related.len() {
                set(&mut document.meta, "related_ids", &remaining)?;
                self.persist_document(
                    &memory.id,
                    path,
                    &document.meta,
                    &document.markdown,
                    Some(&document.revision),
                    "删除关联仓库 · 移除已删除记忆的关联",
                )?;
            }
        }
        let mut paths = memories
            .iter()
            .map(|memory| PathBuf::from(&memory.path))
            .collect::<Vec<_>>();
        let directory = self.root.join(id);
        if directory.is_dir() {
            markdown_files(&directory, &mut paths)?;
        }
        paths.sort();
        paths.dedup();
        let other_paths = others
            .iter()
            .map(|memory| Path::new(&memory.path))
            .collect::<BTreeSet<_>>();
        for path in paths
            .iter()
            .filter(|path| !other_paths.contains(path.as_path()))
        {
            let destination = trash
                .join("memories")
                .join(path.strip_prefix(&self.root).map_err(|e| e.to_string())?);
            fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::rename(path, &destination).map_err(|e| e.to_string())?;
        }
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        for memory in &memories {
            for sql in [
                "DELETE FROM relations WHERE from_id=?1 OR to_id=?1",
                "DELETE FROM memory_inputs WHERE memory_id=?1",
                "DELETE FROM memory_sources WHERE memory_id=?1",
                "DELETE FROM memory_code_paths WHERE memory_id=?1",
                "DELETE FROM memories WHERE id=?1",
            ] {
                transaction
                    .execute(sql, [&memory.id])
                    .map_err(|e| e.to_string())?;
            }
        }
        transaction.commit().map_err(|e| e.to_string())?;
        self.repositories.remove(id);
        self.refresh()
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        let existing = {
            let mut statement = self
                .db
                .prepare("SELECT path,id,revision,markdown,source FROM memories")
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        (
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?,
                        ),
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<BTreeMap<_, _>, _>>()
                .map_err(|e| e.to_string())?
        };
        let mut paths = Vec::new();
        markdown_files(&self.root, &mut paths)?;
        paths.sort();
        let mut records = Vec::new();
        let mut ids = BTreeSet::new();
        self.warnings.clear();
        for path in paths {
            let result = (|| {
                let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let previous = existing.get(&path.display().to_string());
                if let Some((id, revision, _, _)) = previous {
                    if blake3::hash(raw.as_bytes()).to_hex().as_str() == revision
                        && ids.insert(id.clone())
                    {
                        return Ok(None);
                    }
                }
                let mut document = parse_document_text(&raw)?;
                let moved = if previous.is_none() {
                    field(&document.meta, "id").and_then(|id| {
                        existing.iter().find(|(old_path, entry)| {
                            entry.0 == id && Path::new(old_path).starts_with(&self.root)
                        })
                    })
                } else {
                    None
                };
                let observed = previous.or_else(|| moved.map(|(_, entry)| entry));
                if let Some((_, _, old, source)) = observed.filter(|entry| {
                    !entry.1.is_empty() && (entry.1 != document.revision || moved.is_some())
                }) {
                    if field(&document.meta, "codex_source_id").is_some()
                        && conclusion(old) != conclusion(&document.markdown)
                    {
                        if let Some(old) = conclusion(old) {
                            document.markdown = memory_markdown::record_change(
                                &document.markdown,
                                old,
                                &format!(
                                    "外部编辑；此前来源：{}",
                                    source.as_deref().unwrap_or("人工内容")
                                ),
                            );
                        }
                        set(&mut document.meta, "conclusion_owner", &"manual")?;
                    }
                    let event = moved
                        .map(|(old_path, _)| {
                            format!("文件移动 · {} → {}", old_path, path.display())
                        })
                        .unwrap_or_else(|| "文件更新 · 刷新记忆库时识别到文件修改".into());
                    document.markdown = memory_markdown::record_update(&document.markdown, &event);
                    set(&mut document.meta, "updated_at", &Utc::now().to_rfc3339())?;
                    let raw = format!(
                        "---\n{}---\n{}",
                        serde_yaml_ng::to_string(&document.meta).map_err(|e| e.to_string())?,
                        document.markdown
                    );
                    write_atomic(&path, raw.as_bytes())?;
                    document = parse_document_text(&raw)?;
                }
                self.load_document(&path, document).map(Some)
            })();
            match result {
                Ok(Some((record, related))) if ids.insert(record.summary.id.clone()) => {
                    records.push((record, related))
                }
                Ok(Some((record, _))) => self.warnings.push(format!(
                    "重复记忆 ID {}，已跳过 {}",
                    record.summary.id,
                    path.display()
                )),
                Ok(None) => {}
                Err(error) => self.warnings.push(format!("{}：{error}", path.display())),
            }
        }
        let changed: BTreeSet<_> = records.iter().map(|(r, _)| r.summary.id.as_str()).collect();
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        for (id, _, _, _) in existing.values() {
            if !ids.contains(id) || changed.contains(id.as_str()) {
                transaction
                    .execute("DELETE FROM memories WHERE id=?1", [id])
                    .map_err(|e| e.to_string())?;
                transaction
                    .execute("DELETE FROM relations WHERE from_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                transaction
                    .execute("DELETE FROM memory_inputs WHERE memory_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                transaction
                    .execute("DELETE FROM memory_sources WHERE memory_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                transaction
                    .execute("DELETE FROM memory_code_paths WHERE memory_id=?1", [id])
                    .map_err(|e| e.to_string())?;
            }
        }
        for (record, related) in &records {
            index_record(&transaction, record, related)?;
        }
        transaction.commit().map_err(|e| e.to_string())?;
        self.persist_repositories()
    }

    pub fn rebuild(&mut self) -> Result<(), String> {
        self.db.execute_batch("DELETE FROM relations; DELETE FROM memories; DELETE FROM memory_inputs; DELETE FROM memory_sources; DELETE FROM memory_code_paths;").map_err(|e| e.to_string())?;
        self.refresh()
    }

    pub fn search(
        &self,
        repository_id: Option<&str>,
        query: &str,
    ) -> Result<Vec<MemorySummary>, String> {
        let mut statement = self.db.prepare("SELECT id,repository_id,title,summary,kind,tags,updated_at,path,markdown FROM memories WHERE (?1 IS NULL OR repository_id=?1) ORDER BY updated_at DESC,id").map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([repository_id], |r| {
                Ok((summary_row(r)?, r.get::<_, String>(8)?))
            })
            .map_err(|e| e.to_string())?;
        let query = KeywordQuery::new(query);
        let mut hits = Vec::new();
        for row in rows {
            let (summary, markdown) = row.map_err(|e| e.to_string())?;
            let title = summary.title.to_lowercase();
            let text = format!(
                "{} {} {} {}",
                summary.id,
                summary.summary,
                summary.tags.join(" "),
                markdown
            )
            .to_lowercase();
            let score = query.score(&title, &text);
            if score > 0 {
                hits.push((score, summary));
            }
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        Ok(hits.into_iter().map(|(_, summary)| summary).collect())
    }

    pub fn graph(&self) -> Result<MemoryGraph, String> {
        let mut statement = self.db.prepare("SELECT id,repository_id,title,summary,kind,tags,updated_at,path FROM memories ORDER BY id").map_err(|e| e.to_string())?;
        let nodes = statement
            .query_map([], summary_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut statement = self
            .db
            .prepare(
                "SELECT DISTINCT min(r.from_id,r.to_id) AS source,max(r.from_id,r.to_id) AS target
            FROM relations r
            JOIN memories a ON a.id=r.from_id
            JOIN memories b ON b.id=r.to_id
            WHERE r.from_id<>r.to_id
            ORDER BY source,target",
            )
            .map_err(|e| e.to_string())?;
        let edges = statement
            .query_map([], |row| {
                Ok(MemoryGraphEdge {
                    source: row.get(0)?,
                    target: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(MemoryGraph { nodes, edges })
    }

    pub fn read(&self, id: &str) -> Result<MemoryRecord, String> {
        let path: String = self
            .db
            .query_row("SELECT path FROM memories WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("未找到这条记忆，请刷新记忆库")?;
        let document = parse_document(&PathBuf::from(&path))?;
        let mut record = self.db.query_row("SELECT id,repository_id,title,summary,kind,tags,updated_at,path FROM memories WHERE id=?1", [id], summary_row).map_err(|e| e.to_string())?;
        record.title = field(&document.meta, "title")
            .unwrap_or_else(|| title_from_markdown(&document.markdown, &PathBuf::from(&path)));
        record.summary = excerpt(&document.markdown);
        let mut statement = self.db.prepare("SELECT to_id FROM relations WHERE from_id=?1 UNION SELECT from_id FROM relations WHERE to_id=?1").map_err(|e| e.to_string())?;
        let related = statement
            .query_map([id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let outgoing = strings(&document.meta, "related_ids")?;
        let conclusion_owner = conclusion_owner(&document);
        let suggestions = memory_markdown::suggestions(&document.markdown);
        let evidence_sources = memory_markdown::evidence_sources(
            &document.markdown,
            &archive_sources(&document.meta)?,
        );
        Ok(MemoryRecord {
            current_conclusion: conclusion(&document.markdown)
                .map(str::to_owned)
                .unwrap_or_else(|| record.summary.clone()),
            conclusion_owner,
            suggestions,
            evidence_sources,
            summary: record,
            markdown: document.markdown,
            source: field(&document.meta, "source"),
            related_ids: related,
            outgoing_related_ids: outgoing,
            code_paths: strings(&document.meta, "code_paths")?,
            revision: document.revision,
        })
    }

    pub fn save(&mut self, input: MemoryInput) -> Result<MemoryRecord, String> {
        self.save_document(input, None, None, None)
    }

    pub fn adopt_suggestion(
        &mut self,
        id: &str,
        source_id: &str,
        revision: &str,
    ) -> Result<MemoryRecord, String> {
        let record = self.read(id)?;
        if record.revision != revision {
            return Err("正文已更新，请读取最新结论和建议后再采用".into());
        }
        let suggestion = record
            .suggestions
            .iter()
            .find(|s| s.source_id == source_id)
            .ok_or("未找到这条建议，请刷新记忆")?;
        let path = Path::new(&record.summary.path);
        let mut document = parse_document(path)?;
        let body = memory_markdown::remove_suggestion(&document.markdown, source_id);
        let body = memory_markdown::replace_conclusion(&body, &suggestion.summary);
        let body = memory_markdown::record_change(
            &body,
            conclusion(&document.markdown).unwrap_or(&record.summary.summary),
            &format!(
                "人工确认采用建议；建议来源：{source_id}；此前归属：{}",
                record.conclusion_owner
            ),
        );
        set(&mut document.meta, "conclusion_owner", &"manual")?;
        set(&mut document.meta, "updated_at", &Utc::now().to_rfc3339())?;
        self.persist_document(
            id,
            path,
            &document.meta,
            &body,
            Some(revision),
            &format!("采用建议 · 来源 {source_id}，由人工确认当前结论"),
        )
    }

    pub fn save_archive(
        &mut self,
        id: &str,
        input: MemoryInput,
        source: &ArchiveSource,
        code_paths: &[String],
    ) -> Result<MemoryRecord, String> {
        let exists = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memories WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            return self.read(id);
        }
        self.save_document(input, Some(id), Some(source), Some(code_paths))
    }

    pub fn memories_for_code(
        &self,
        repository_id: &str,
        paths: &[String],
    ) -> Result<Vec<String>, String> {
        let mut statement = self.db.prepare("SELECT DISTINCT m.id FROM memories m JOIN memory_code_paths p ON p.memory_id=m.id WHERE m.repository_id=?1 AND p.path IN (SELECT value FROM json_each(?2)) ORDER BY m.id").map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(
                params![
                    repository_id,
                    serde_json::to_string(paths).map_err(|e| e.to_string())?
                ],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn extend_archive(
        &mut self,
        id: &str,
        repository_id: &str,
        memory: &crate::model_api::ExtractedMemory,
        source: &ArchiveSource,
        previous_auto_summary: Option<&str>,
    ) -> Result<MemoryRecord, String> {
        let path: String = self
            .db
            .query_row(
                "SELECT path FROM memories WHERE id=?1 AND repository_id=?2",
                params![id, repository_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("未找到本仓库中的归档事项")?;
        let path = Path::new(&path);
        let mut document = parse_document(path)?;
        let first = field(&document.meta, "codex_source_id").ok_or("该记忆没有归档来源")?;
        let mut followups: Vec<ArchiveSource> = document
            .meta
            .get("codex_followups")
            .cloned()
            .map(serde_yaml_ng::from_value)
            .transpose()
            .map_err(|e| format!("事项的归档来源格式错误：{e}"))?
            .unwrap_or_default();
        if first == source.source_id || followups.iter().any(|s| s.source_id == source.source_id) {
            return self.read(id);
        }
        if field(&document.meta, "archive_summary_hash").is_none() {
            if let Some(baseline) = previous_auto_summary
                .filter(|baseline| Some(baseline.trim()) == conclusion(&document.markdown))
            {
                set(
                    &mut document.meta,
                    "archive_summary_hash",
                    &blake3::hash(baseline.trim().as_bytes())
                        .to_hex()
                        .to_string(),
                )?;
            }
        }
        let owner = conclusion_owner(&document);
        let previous = conclusion(&document.markdown).unwrap_or("");
        let mut markdown = document.markdown.clone();
        if previous != memory.summary.trim() {
            if owner == "auto" {
                markdown = memory_markdown::replace_conclusion(&markdown, &memory.summary);
                markdown = memory_markdown::record_change(
                    &markdown,
                    previous,
                    &format!(
                        "自动结论；来源：{}",
                        archive_sources(&document.meta)?
                            .iter()
                            .map(|s| s.source_id.as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                );
                set(
                    &mut document.meta,
                    "archive_summary_hash",
                    &blake3::hash(memory.summary.trim().as_bytes())
                        .to_hex()
                        .to_string(),
                )?;
            } else {
                markdown = memory_markdown::append_section(
                    &markdown,
                    "待采用建议",
                    &format!(
                        "### 建议 · {}\n\n<!-- suggestion:{} -->\n\n{}",
                        source.turn_id,
                        source.source_id,
                        memory.summary.trim()
                    ),
                );
            }
        }
        if !memory.details.trim().is_empty() {
            markdown =
                memory_markdown::append_section(&markdown, "详细说明", memory.details.trim());
        }
        markdown = memory_markdown::append_section(
            &markdown,
            "原文依据",
            &source_evidence(source, &memory.evidence),
        );
        let mut tags = strings(&document.meta, "tags")?;
        for tag in &memory.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        followups.push(source.clone());
        let mut code_paths = strings(&document.meta, "code_paths")?;
        code_paths.extend(memory.code_paths.iter().cloned());
        code_paths.sort();
        code_paths.dedup();
        let mut related = strings(&document.meta, "related_ids")?;
        related.extend(memory.related_ids.iter().cloned());
        related.sort();
        related.dedup();
        set(&mut document.meta, "code_paths", &code_paths)?;
        set(&mut document.meta, "related_ids", &related)?;
        set(&mut document.meta, "codex_followups", &followups)?;
        set(&mut document.meta, "tags", &tags)?;
        set(&mut document.meta, "conclusion_owner", &owner)?;
        set(
            &mut document.meta,
            "updated_at",
            &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )?;
        self.persist_document(
            id,
            path,
            &document.meta,
            &markdown,
            Some(&document.revision),
            &format!(
                "自动续写 · 来源 {} · {}",
                source.source_id,
                if owner == "auto" {
                    "更新结论与依据"
                } else {
                    "保留人工结论，补充依据与建议"
                }
            ),
        )
    }

    fn save_document(
        &mut self,
        input: MemoryInput,
        archive_id: Option<&str>,
        source: Option<&ArchiveSource>,
        code_paths: Option<&[String]>,
    ) -> Result<MemoryRecord, String> {
        validate_memory(&input)?;
        let mut body = input.markdown.clone();
        let mut update = if source.is_some() {
            "自动归档".to_string()
        } else {
            "创建记忆".to_string()
        };
        let repository = self
            .repositories
            .get(&input.repository_id)
            .cloned()
            .ok_or("请选择有效仓库")?;
        let (id, path, mut meta, previous_revision) = match input.id {
            Some(id) => {
                let existing = self.read(&id)?;
                if input.revision.as_deref() != Some(existing.revision.as_str()) {
                    return Err(
                        "正文已被其他编辑器修改，请重新读取后再编辑。当前输入仍保留。".into(),
                    );
                }
                if existing.summary.repository_id != input.repository_id {
                    return Err("编辑记忆时请保留原仓库".into());
                }
                let mut fields = Vec::new();
                if input.title.trim() != existing.summary.title {
                    fields.push("标题");
                }
                if input.kind.trim() != existing.summary.kind {
                    fields.push("类型");
                }
                if input.tags != existing.summary.tags {
                    fields.push("标签");
                }
                if input.markdown != existing.markdown {
                    fields.push("正文");
                }
                if input.related_ids != existing.outgoing_related_ids {
                    fields.push("关联");
                }
                if input.source != existing.source {
                    fields.push("来源说明");
                }
                if fields.is_empty() {
                    return Ok(existing);
                }
                update = format!("人工编辑 · {}", fields.join("、"));
                let path = PathBuf::from(existing.summary.path);
                let mut document = parse_document(&path)?;
                let owner = if conclusion(&body) != conclusion(&document.markdown) {
                    if let Some(old) = conclusion(&document.markdown) {
                        body = memory_markdown::record_change(
                            &body,
                            old,
                            &format!(
                                "人工编辑；此前来源：{}",
                                existing.source.as_deref().unwrap_or("人工内容")
                            ),
                        );
                    }
                    "manual".to_string()
                } else {
                    conclusion_owner(&document)
                };
                set(&mut document.meta, "conclusion_owner", &owner)?;
                (id, path, document.meta, Some(existing.revision))
            }
            None => {
                let id = archive_id
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("MEM-{}", uuid::Uuid::new_v4().simple()));
                let path = self.root.join(&repository.id).join(format!("{id}.md"));
                (id, path, Mapping::new(), None)
            }
        };
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if !meta.contains_key(Value::String("created_at".into())) {
            set(&mut meta, "created_at", &now)?;
        }
        set(&mut meta, "id", &id)?;
        set(&mut meta, "repository_id", &repository.id)?;
        set(&mut meta, "repository_name", &repository.name)?;
        set(&mut meta, "repository_remote", &repository.remote)?;
        set(&mut meta, "title", &input.title.trim())?;
        set(&mut meta, "kind", &input.kind.trim())?;
        set(&mut meta, "tags", &input.tags)?;
        set(&mut meta, "related_ids", &input.related_ids)?;
        set(&mut meta, "source", &input.source)?;
        set(&mut meta, "updated_at", &now)?;
        if let Some(code_paths) = code_paths {
            set(&mut meta, "code_paths", &code_paths)?;
        }
        if let Some(source) = source {
            set(&mut meta, "codex_source_id", &source.source_id)?;
            set(&mut meta, "codex_session_id", &source.session_id)?;
            set(&mut meta, "codex_turn_id", &source.turn_id)?;
            set(&mut meta, "codex_message_role", &source.message_role)?;
            set(&mut meta, "conclusion_owner", &"auto")?;
            set(
                &mut meta,
                "archive_summary_hash",
                &blake3::hash(conclusion(&body).unwrap_or("").as_bytes())
                    .to_hex()
                    .to_string(),
            )?;
            if let Some(section) = memory_markdown::section_ranges(&body)
                .into_iter()
                .find(|s| s.info.title == "原文依据")
            {
                body.insert_str(
                    section.body_start,
                    &format!(
                        "\n<!-- source:{} -->\n\n**会话 {} · 轮次 {}**\n",
                        source.source_id, source.session_id, source.turn_id
                    ),
                );
            }
            set(
                &mut meta,
                "archive_input_truncated",
                &source.input_truncated,
            )?;
        }
        self.persist_document(
            &id,
            &path,
            &meta,
            &body,
            previous_revision.as_deref(),
            &update,
        )
    }

    fn persist_document(
        &mut self,
        id: &str,
        path: &Path,
        meta: &Mapping,
        body: &str,
        previous_revision: Option<&str>,
        update: &str,
    ) -> Result<MemoryRecord, String> {
        let markdown = format!(
            "---\n{}---\n{}",
            serde_yaml_ng::to_string(meta).map_err(|e| e.to_string())?,
            memory_markdown::record_update(body, update)
        );
        // Recheck immediately before replacement because Markdown is also editable outside the app.
        if let Some(revision) = previous_revision {
            if parse_document(path)?.revision != revision {
                return Err("正文已在外部更新，请重新读取后再保存".into());
            }
        }
        write_atomic(path, markdown.as_bytes())?;
        let (record, related) = self.load_document(path, parse_document_text(&markdown)?)?;
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        transaction
            .execute("DELETE FROM memories WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        transaction
            .execute("DELETE FROM relations WHERE from_id=?1", [id])
            .map_err(|e| e.to_string())?;
        index_record(&transaction, &record, &related)?;
        transaction.commit().map_err(|e| e.to_string())?;
        self.read(id)
    }

    fn load_document(
        &mut self,
        path: &Path,
        document: Document,
    ) -> Result<(MemoryRecord, Vec<String>), String> {
        let relative = path.strip_prefix(&self.root).map_err(|e| e.to_string())?;
        let remote = field(&document.meta, "repository_remote").filter(|s| !s.trim().is_empty());
        let declared_id = field(&document.meta, "repository_id");
        if declared_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\']))
        {
            return Err("repository_id 必须是单个目录名称".into());
        }
        let folder = relative
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("本地记忆");
        let repository_id = if let Some(remote) = &remote {
            repository_id(&normalize_remote(remote)?)
        } else {
            declared_id.clone().unwrap_or_else(|| folder.to_string())
        };
        let repository = self
            .repositories
            .entry(repository_id.clone())
            .or_insert_with(|| Repository {
                id: repository_id.clone(),
                name: field(&document.meta, "repository_name")
                    .unwrap_or_else(|| folder.to_string()),
                remote: remote.unwrap_or_default(),
                memory_count: 0,
            });
        let id = field(&document.meta, "id").unwrap_or_else(|| {
            format!(
                "MD-{}",
                &blake3::hash(relative.to_string_lossy().as_bytes()).to_hex()[..16]
            )
        });
        let updated_at = field(&document.meta, "updated_at").unwrap_or_else(|| {
            fs::metadata(path)
                .and_then(|m| m.modified())
                .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
                .unwrap_or_default()
        });
        let related = strings(&document.meta, "related_ids")?;
        let record = MemoryRecord {
            current_conclusion: conclusion(&document.markdown)
                .map(str::to_owned)
                .unwrap_or_else(|| excerpt(&document.markdown)),
            conclusion_owner: conclusion_owner(&document),
            suggestions: memory_markdown::suggestions(&document.markdown),
            evidence_sources: memory_markdown::evidence_sources(
                &document.markdown,
                &archive_sources(&document.meta)?,
            ),
            summary: MemorySummary {
                id,
                repository_id: repository.id.clone(),
                title: field(&document.meta, "title")
                    .unwrap_or_else(|| title_from_markdown(&document.markdown, path)),
                summary: excerpt(&document.markdown),
                kind: field(&document.meta, "kind").unwrap_or_else(|| "手动记忆".into()),
                tags: strings(&document.meta, "tags")?,
                updated_at,
                path: path.display().to_string(),
                embedding_status: "unconfigured".into(),
                embedding_error: None,
            },
            markdown: document.markdown,
            related_ids: related.clone(),
            outgoing_related_ids: related.clone(),
            code_paths: strings(&document.meta, "code_paths")?,
            source: field(&document.meta, "source"),
            revision: document.revision,
        };
        Ok((record, related))
    }

    fn persist_repositories(&self) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&self.repositories.values().collect::<Vec<_>>())
            .map_err(|e| e.to_string())?;
        write_atomic(&self.root.join(".aimemory/repositories.json"), &json)
    }
}

fn repository_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入仓库名称".into());
    }
    Ok(name)
}

pub(crate) struct KeywordQuery {
    query: String,
    terms: BTreeSet<String>,
    literals: Vec<String>,
}

impl KeywordQuery {
    pub(crate) fn new(query: &str) -> Self {
        let identifiers = query
            .split(|c: char| {
                !c.is_ascii_alphanumeric() && !matches!(c, '/' | '\\' | '.' | '_' | '-' | ':')
            })
            .map(|s| s.trim_matches(['.', ':']))
            .filter(|s| s.len() > 1)
            .collect::<Vec<_>>();
        let literals = identifiers
            .iter()
            .filter(|s| {
                s.contains(['/', '\\', '.', '_', '-', ':'])
                    || s.len() >= 3
                        && (s.chars().any(|c| c.is_ascii_digit())
                            || s.chars().all(|c| c.is_ascii_uppercase())
                            || s.chars().skip(1).any(|c| c.is_ascii_uppercase()))
            })
            .map(|s| s.to_lowercase())
            .collect();
        let query = query.trim().to_lowercase();
        let mut terms: BTreeSet<String> = query.split_whitespace().map(str::to_string).collect();
        terms.extend(identifiers.iter().map(|s| s.to_lowercase()));
        let chars: Vec<_> = query.chars().collect();
        for pair in chars.windows(2) {
            if pair.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c)) {
                terms.insert(pair.iter().collect());
            }
        }
        Self {
            query,
            terms,
            literals,
        }
    }

    pub(crate) fn score(&self, title: &str, text: &str) -> usize {
        if self.query.is_empty() {
            return 1;
        }
        usize::from(title.contains(&self.query)) * 100
            + usize::from(text.contains(&self.query)) * 30
            + self
                .terms
                .iter()
                .map(|term| {
                    usize::from(title.contains(term)) * 6 + usize::from(text.contains(term))
                })
                .sum::<usize>()
    }

    pub(crate) fn literal_match(&self, text: &str) -> bool {
        self.literals.iter().any(|token| text.contains(token))
    }

    pub(crate) fn has_literals(&self) -> bool {
        !self.literals.is_empty()
    }
}

fn index_record(
    transaction: &rusqlite::Transaction<'_>,
    record: &MemoryRecord,
    related: &[String],
) -> Result<(), String> {
    let s = &record.summary;
    transaction
        .execute(
            "INSERT INTO memories VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                s.id,
                s.repository_id,
                s.path,
                s.title,
                s.summary,
                s.kind,
                serde_json::to_string(&s.tags).unwrap(),
                s.updated_at,
                record.markdown,
                record.source,
                record.revision
            ],
        )
        .map_err(|e| e.to_string())?;
    transaction.execute("INSERT INTO memory_inputs VALUES(?1,?2) ON CONFLICT(memory_id) DO UPDATE SET content=excluded.content", params![s.id, memory_markdown::retrieval_text(&s.title, &record.markdown, &s.tags)]).map_err(|e| e.to_string())?;
    transaction
        .execute("DELETE FROM memory_sources WHERE memory_id=?1", [&s.id])
        .map_err(|e| e.to_string())?;
    for source in &record.evidence_sources {
        transaction
            .execute(
                "INSERT INTO memory_sources VALUES(?1,?2,?3)",
                params![s.id, source.source_id, source.session_id],
            )
            .map_err(|e| e.to_string())?;
    }
    for target in related {
        transaction
            .execute(
                "INSERT OR IGNORE INTO relations VALUES(?1,?2)",
                params![s.id, target],
            )
            .map_err(|e| e.to_string())?;
    }
    transaction
        .execute("DELETE FROM memory_code_paths WHERE memory_id=?1", [&s.id])
        .map_err(|e| e.to_string())?;
    for path in &record.code_paths {
        transaction
            .execute(
                "INSERT OR IGNORE INTO memory_code_paths VALUES(?1,?2)",
                params![s.id, path],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn conclusion_owner(document: &Document) -> String {
    let hash = blake3::hash(conclusion(&document.markdown).unwrap_or("").as_bytes()).to_hex();
    if field(&document.meta, "conclusion_owner").as_deref() != Some("manual")
        && field(&document.meta, "archive_summary_hash").as_deref() == Some(hash.as_str())
    {
        "auto".into()
    } else {
        "manual".into()
    }
}

fn archive_sources(meta: &Mapping) -> Result<Vec<ArchiveSource>, String> {
    let mut sources = Vec::new();
    if let Some(source_id) = field(meta, "codex_source_id") {
        sources.push(ArchiveSource {
            source_id,
            session_id: field(meta, "codex_session_id").unwrap_or_default(),
            turn_id: field(meta, "codex_turn_id").unwrap_or_default(),
            input_truncated: meta
                .get("archive_input_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            message_role: field(meta, "codex_message_role"),
        });
    }
    if let Some(value) = meta.get("codex_followups") {
        sources.extend(
            serde_yaml_ng::from_value::<Vec<ArchiveSource>>(value.clone())
                .map_err(|e| format!("归档来源格式错误：{e}"))?,
        );
    }
    Ok(sources)
}

fn source_evidence(source: &ArchiveSource, evidence: &str) -> String {
    let quote = evidence
        .split('\n')
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<!-- source:{} -->\n\n**会话 {} · 轮次 {}**\n\n{quote}",
        source.source_id, source.session_id, source.turn_id
    )
}

fn summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemorySummary> {
    let tags: String = row.get(5)?;
    Ok(MemorySummary {
        id: row.get(0)?,
        repository_id: row.get(1)?,
        title: row.get(2)?,
        summary: row.get(3)?,
        kind: row.get(4)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        updated_at: row.get(6)?,
        path: row.get(7)?,
        embedding_status: "unconfigured".into(),
        embedding_error: None,
    })
}

fn validate_memory(input: &MemoryInput) -> Result<(), String> {
    if input.title.trim().is_empty() {
        return Err("请输入记忆标题".into());
    }
    if input.markdown.trim().is_empty() {
        return Err("请输入 Markdown 正文".into());
    }
    if input.kind.trim().is_empty() {
        return Err("请选择记忆类型".into());
    }
    Ok(())
}

pub fn normalize_remote(remote: &str) -> Result<String, String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let normalized = if !remote.contains("://") {
        let (host, path) = remote
            .split_once(':')
            .ok_or("请输入 Git HTTPS 或 SSH 远程地址")?;
        let host = host.rsplit('@').next().unwrap_or(host);
        format!("https://{host}/{path}")
    } else {
        remote.to_string()
    };
    let parsed = url::Url::parse(&normalized).map_err(|_| "Git 远程地址格式不正确")?;
    if !matches!(parsed.scheme(), "https" | "http" | "ssh" | "git")
        || parsed.host_str().is_none()
        || parsed.path().trim_matches('/').is_empty()
    {
        return Err("请输入包含仓库路径的 Git HTTPS 或 SSH 远程地址".into());
    }
    let host = parsed.host_str().unwrap().to_lowercase();
    let port = parsed
        .port()
        .filter(|p| !(parsed.scheme() == "ssh" && *p == 22))
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    Ok(format!(
        "https://{host}{port}{}",
        parsed.path().trim_end_matches('/').trim_end_matches(".git")
    ))
}

pub(crate) fn repository_id(remote: &str) -> String {
    let name: String = remote
        .rsplit('/')
        .next()
        .unwrap_or("repository")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "{}-{}",
        name,
        &blake3::hash(remote.as_bytes()).to_hex()[..8]
    )
}

fn parse_document(path: &Path) -> Result<Document, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_document_text(&raw)
}

fn parse_document_text(raw: &str) -> Result<Document, String> {
    let revision = blake3::hash(raw.as_bytes()).to_hex().to_string();
    let text = raw.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Ok(Document {
            meta: Mapping::new(),
            markdown: text.to_string(),
            revision,
        });
    }
    let mut offset = first.len();
    let header_start = offset;
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            let header = &text[header_start..offset];
            let meta = if header.trim().is_empty() {
                Mapping::new()
            } else {
                serde_yaml_ng::from_str::<Mapping>(header)
                    .map_err(|e| format!("Markdown 元数据格式错误：{e}"))?
            };
            return Ok(Document {
                meta,
                markdown: text[offset + line.len()..].to_string(),
                revision,
            });
        }
        offset += line.len();
    }
    Err("Markdown 元数据缺少结束分隔线".into())
}

pub mod transfer;

fn markdown_files(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            markdown_files(&entry.path(), paths)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("md"))
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn field(meta: &Mapping, name: &str) -> Option<String> {
    meta.get(Value::String(name.into()))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn strings(meta: &Mapping, name: &str) -> Result<Vec<String>, String> {
    match meta.get(Value::String(name.into())) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => {
            serde_yaml_ng::from_value(value.clone()).map_err(|_| format!("{name} 必须是文字列表"))
        }
    }
}

fn set(meta: &mut Mapping, name: &str, value: &impl Serialize) -> Result<(), String> {
    meta.insert(
        Value::String(name.into()),
        serde_yaml_ng::to_value(value).map_err(|e| e.to_string())?,
    );
    Ok(())
}

fn title_from_markdown(markdown: &str, path: &Path) -> String {
    markdown
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("保存路径无效")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(contents)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(path)
        .map_err(|e| format!("无法保存文件：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
