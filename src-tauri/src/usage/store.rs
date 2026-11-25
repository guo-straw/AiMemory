#[cfg(test)]
#[path = "pricing_tests.rs"]
mod pricing_tests;

use super::{
    parser::{self, Cursor},
    *,
};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, Row,
    Transaction,
};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
};

// Official model IDs select reference rates, independently of the logged routing provider.
const PRICED: &str = "WITH rates AS (
    SELECT u.*,
        CASE WHEN p.model IS NOT NULL THEN p.input ELSE o.input END AS price_input,
        CASE WHEN p.model IS NOT NULL THEN p.cached ELSE o.cached END AS price_cached,
        CASE WHEN p.model IS NOT NULL THEN p.cache_write ELSE o.cache_write END AS price_write,
        CASE WHEN p.model IS NOT NULL THEN p.output ELSE o.output END AS price_output,
        CASE WHEN p.model IS NOT NULL THEN p.source ELSE o.source END AS price_source,
        p.model IS NOT NULL AS price_custom
    FROM usage_records u
    LEFT JOIN usage_prices p ON p.model=u.model AND p.provider=u.provider AND p.tier=u.tier
        AND (p.input IS NOT NULL OR p.cached IS NOT NULL OR p.cache_write IS NOT NULL OR p.output IS NOT NULL)
    LEFT JOIN usage_official_prices o ON p.model IS NULL AND o.model=u.model
        AND o.tier=CASE u.tier WHEN '' THEN 'standard' WHEN 'default' THEN 'standard' WHEN 'auto' THEN 'standard' WHEN 'priority' THEN 'fast' ELSE u.tier END
        AND u.input>=o.min_input AND (o.max_input IS NULL OR u.input<=o.max_input)
        AND u.at>=o.valid_from AND (o.valid_until IS NULL OR u.at<o.valid_until)
), priced AS (
    SELECT *, CASE WHEN price_input IS NOT NULL AND price_output IS NOT NULL AND cached IS NOT NULL
        AND (cached=0 OR price_cached IS NOT NULL) AND (cache_write=0 OR price_write IS NOT NULL)
        THEN ((input-cached-cache_write)*price_input+cached*COALESCE(price_cached,0)+cache_write*COALESCE(price_write,0)+output*price_output)/1000000.0 END AS cost
    FROM rates) ";
const WHERE: &str = "r.at>=?1 AND r.at<?2 AND (?3 IS NULL OR r.project=?3) AND (?4 IS NULL OR r.model=?4) AND (?5 IS NULL OR r.role=?5) AND r.source IN (?6,?7) AND (?8 IS NULL OR r.client=?8)";
const AGG: &str = "COUNT(*),COUNT(DISTINCT r.thread_id),COALESCE(SUM(r.input),0),COALESCE(SUM(r.cached),0),COALESCE(SUM(r.cache_write),0),COALESCE(SUM(r.output),0),COALESCE(SUM(r.reasoning),0),COALESCE(SUM(CASE WHEN r.cached IS NOT NULL THEN r.input ELSE 0 END),0),COALESCE(SUM(CASE WHEN r.cost IS NOT NULL THEN r.input+r.output ELSE 0 END),0),COUNT(r.cost),COALESCE(SUM(r.cost),0),COALESCE(SUM(CASE WHEN r.reasoning IS NOT NULL THEN r.output ELSE 0 END),0)";

pub struct Store {
    db: Connection,
}

pub(super) fn source_id(directory: &Path) -> String {
    let home = if directory
        .file_name()
        .is_some_and(|n| n == "sessions" || n == "archived_sessions")
    {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    blake3::hash(
        home.canonicalize()
            .unwrap_or_else(|_| home.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    )
    .to_hex()
    .to_string()
}

pub(super) fn harness_source_id(directory: &Path) -> String {
    format!(
        "harness:{}",
        crate::harness::root_directory(directory).to_string_lossy()
    )
}

fn sources(codex: &str, harness: &str, client: Option<&str>) -> [SqlValue; 2] {
    [
        if client.is_none() || client == Some("codex") {
            codex.to_string().into()
        } else {
            SqlValue::Null
        },
        if client.is_none() || client == Some("harness") {
            harness.to_string().into()
        } else {
            SqlValue::Null
        },
    ]
}

fn values(f: &Filters) -> Vec<SqlValue> {
    vec![
        f.start.into(),
        f.end.into(),
        f.project
            .clone()
            .map(SqlValue::Text)
            .unwrap_or(SqlValue::Null),
        f.model
            .clone()
            .map(SqlValue::Text)
            .unwrap_or(SqlValue::Null),
        f.role.clone().map(SqlValue::Text).unwrap_or(SqlValue::Null),
        f.source.clone().into(),
        f.harness_source.clone().into(),
        f.client
            .clone()
            .map(SqlValue::Text)
            .unwrap_or(SqlValue::Null),
    ]
}

fn summary(row: &Row<'_>, n: usize) -> rusqlite::Result<Summary> {
    Ok(Summary {
        records: row.get(n)?,
        sessions: row.get(n + 1)?,
        input: row.get(n + 2)?,
        cached: row.get(n + 3)?,
        cache_write: row.get(n + 4)?,
        output: row.get(n + 5)?,
        reasoning: row.get(n + 6)?,
        cache_known_input: row.get(n + 7)?,
        reasoning_known_output: row.get(n + 11)?,
        priced_tokens: row.get(n + 8)?,
        priced_records: row.get(n + 9)?,
        cost: row.get(n + 10)?,
    })
}

fn session(row: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        client: row.get(20)?,
        title: row.get(1)?,
        parent_id: row.get(2)?,
        role: row.get(3)?,
        model: row.get(4)?,
        project: row.get(5)?,
        first_at: row.get(6)?,
        last_at: row.get(7)?,
        summary: summary(row, 8)?,
    })
}

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|e| format!("{}: {e}", directory.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            collect(&entry.path(), paths)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn save_record(
    tx: &Transaction<'_>,
    source: &str,
    mut record: UsageRecord,
    projects: &mut HashMap<String, String>,
) -> Result<(), String> {
    if !record.project.is_empty() {
        let project = projects.entry(record.project.clone()).or_insert_with(|| {
            crate::git::command()
                .args(["-C", &record.project, "rev-parse", "--show-toplevel"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| record.project.clone())
        });
        record.project = project.clone();
    }
    let t = &record.tokens;
    tx.execute("INSERT INTO usage_records(id,source,thread_id,turn_id,at,model,provider,tier,project,role,input,cached,cache_write,output,reasoning,file,line,basis,client) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19) ON CONFLICT(id) DO UPDATE SET model=excluded.model,provider=excluded.provider,tier=excluded.tier,project=excluded.project,role=excluded.role,input=excluded.input,cached=excluded.cached,cache_write=excluded.cache_write,output=excluded.output,reasoning=excluded.reasoning,file=excluded.file,line=excluded.line,basis=excluded.basis,client=excluded.client",params![record.id,source,record.thread_id,record.turn_id,record.at,record.model,record.provider,record.tier,record.project,record.role,t.input,t.cached,t.cache_write,t.output,t.reasoning,record.file,record.line,record.basis,record.client]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(super) fn source_jsonl(log: &crate::harness::SessionLog) -> Result<String, String> {
    let mut content = serde_json::to_string(&log.header).map_err(|e| e.to_string())?;
    content.push('\n');
    let mut line = 2;
    for event in &log.events {
        while line < event.line {
            content.push('\n');
            line += 1;
        }
        content.push_str(&serde_json::to_string(&event.value).map_err(|e| e.to_string())?);
        content.push('\n');
        line = event.line + 1;
    }
    Ok(content)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS usage_files(source TEXT NOT NULL,name TEXT NOT NULL,path TEXT NOT NULL,cursor TEXT NOT NULL,PRIMARY KEY(source,name));
            CREATE TABLE IF NOT EXISTS usage_threads(source TEXT NOT NULL,id TEXT NOT NULL,parent TEXT,role TEXT NOT NULL,title TEXT,PRIMARY KEY(source,id));
            CREATE TABLE IF NOT EXISTS usage_records(id TEXT PRIMARY KEY,source TEXT NOT NULL,thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,at INTEGER NOT NULL,model TEXT NOT NULL,provider TEXT NOT NULL,tier TEXT NOT NULL,project TEXT NOT NULL,role TEXT NOT NULL,input INTEGER NOT NULL,cached INTEGER,cache_write INTEGER NOT NULL,output INTEGER NOT NULL,reasoning INTEGER,file TEXT NOT NULL,line INTEGER NOT NULL,basis TEXT NOT NULL,client TEXT NOT NULL DEFAULT 'codex');
            CREATE INDEX IF NOT EXISTS usage_time ON usage_records(source,at);
            CREATE INDEX IF NOT EXISTS usage_thread ON usage_records(source,thread_id,at);
            CREATE INDEX IF NOT EXISTS usage_model ON usage_records(source,model,at);
            CREATE INDEX IF NOT EXISTS usage_project ON usage_records(source,project,at);
            CREATE TABLE IF NOT EXISTS usage_prices(model TEXT NOT NULL,provider TEXT NOT NULL,tier TEXT NOT NULL,input REAL,cached REAL,cache_write REAL,output REAL,source TEXT NOT NULL,PRIMARY KEY(model,provider,tier));
            CREATE TABLE IF NOT EXISTS usage_issues(source TEXT NOT NULL,file TEXT NOT NULL,line INTEGER NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(source,file,line));
            CREATE TABLE IF NOT EXISTS usage_quota_snapshots(source TEXT NOT NULL,limit_id TEXT NOT NULL,at INTEGER NOT NULL,value TEXT NOT NULL,PRIMARY KEY(source,limit_id));").map_err(|e|e.to_string())?;
        let has_client: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_records') WHERE name='client')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !has_client {
            db.execute_batch(
                "ALTER TABLE usage_records ADD COLUMN client TEXT NOT NULL DEFAULT 'codex'",
            )
            .map_err(|e| e.to_string())?;
        }
        db.execute_batch(include_str!("official-prices.sql"))
            .map_err(|e| e.to_string())?;
        Ok(Self { db })
    }

    pub fn last_synced(&self) -> Result<Option<i64>, String> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM usage_meta WHERE key='last_synced'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .and_then(|v| v.parse().ok()))
    }

    pub fn preferences(&self) -> Result<Preferences, String> {
        self.db
            .query_row(
                "SELECT value FROM usage_meta WHERE key='preferences'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .unwrap_or_else(|| Ok(Preferences::default()))
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO usage_meta VALUES('preferences',?1)",
                [serde_json::to_string(preferences).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn scan_sources(
        &mut self,
        directories: &Directories,
        full: bool,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Vec<String>, String> {
        let mut errors = Vec::new();
        let paths = match crate::harness::list_sessions(Path::new(&directories.harness)) {
            Ok(paths) => paths,
            Err(error) => {
                errors.push(format!("DeepSeek Harness: {error}"));
                vec![]
            }
        };
        let mut codex_done = 0;
        let mut codex_total = 0;
        match self.scan(Path::new(&directories.codex), full, |done, total| {
            codex_done = done;
            codex_total = total;
            progress(done, total + paths.len());
        }) {
            Ok(codex_errors) => errors.extend(codex_errors),
            Err(error) => errors.push(format!("Codex: {error}")),
        }
        let source = harness_source_id(Path::new(&directories.harness));
        let mut projects = HashMap::new();
        progress(codex_done, codex_total + paths.len());
        for (i, path) in paths.iter().enumerate() {
            if let Err(error) = self.scan_harness_file(path, &source, &mut projects, full) {
                errors.push(format!("{}: {error}", path.display()));
            }
            progress(codex_done + i + 1, codex_total + paths.len());
        }
        self.db
            .execute(
                "INSERT OR REPLACE INTO usage_meta VALUES('last_synced',?1)",
                [Utc::now().timestamp_millis().to_string()],
            )
            .map_err(|e| e.to_string())?;
        Ok(errors)
    }

    pub fn scan(
        &mut self,
        directory: &Path,
        full: bool,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Vec<String>, String> {
        if !directory.try_exists().map_err(|e| e.to_string())? {
            progress(0, 0);
            return Ok(vec![]);
        }
        if !directory.is_dir() {
            return Err(format!("Codex 路径不是目录：{}", directory.display()));
        }
        let home = if directory
            .file_name()
            .is_some_and(|n| n == "sessions" || n == "archived_sessions")
        {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        let source = source_id(directory);
        let mut paths = Vec::new();
        if home.join("sessions").is_dir() || home.join("archived_sessions").is_dir() {
            collect(&home.join("sessions"), &mut paths)?;
            collect(&home.join("archived_sessions"), &mut paths)?;
        } else {
            collect(directory, &mut paths)?;
        }
        paths.sort();
        let mut titles = HashMap::new();
        if home.join("session_index.jsonl").is_file() {
            for line in BufReader::new(
                File::open(home.join("session_index.jsonl")).map_err(|e| e.to_string())?,
            )
            .lines()
            {
                let line = line.map_err(|e| e.to_string())?;
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if let (Some(id), Some(title)) =
                        (value["id"].as_str(), value["thread_name"].as_str())
                    {
                        titles.insert(id.to_string(), title.to_string());
                    }
                }
            }
        }
        let mut projects = HashMap::new();
        let mut errors = Vec::new();
        progress(0, paths.len());
        for (i, path) in paths.iter().enumerate() {
            if let Err(e) = self.scan_file(path, &source, &titles, &mut projects, full) {
                errors.push(format!("{}: {e}", path.display()));
            }
            progress(i + 1, paths.len());
        }
        for (id, title) in titles {
            self.db
                .execute(
                    "UPDATE usage_threads SET title=?1 WHERE source=?2 AND id=?3",
                    params![title, source, id],
                )
                .map_err(|e| e.to_string())?;
        }
        self.db
            .execute(
                "INSERT OR REPLACE INTO usage_meta VALUES('last_synced',?1)",
                [Utc::now().timestamp_millis().to_string()],
            )
            .map_err(|e| e.to_string())?;
        Ok(errors)
    }

    fn scan_file(
        &mut self,
        path: &Path,
        source: &str,
        titles: &HashMap<String, String>,
        projects: &mut HashMap<String, String>,
        full: bool,
    ) -> Result<(), String> {
        let name = path.file_name().unwrap().to_string_lossy();
        let path_text = path.to_string_lossy();
        let existing = self
            .db
            .query_row(
                "SELECT cursor,path FROM usage_files WHERE source=?1 AND name=?2",
                params![source, name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let mut cursor: Cursor = existing
            .as_ref()
            .map(|(s, _)| serde_json::from_str(s))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        if full {
            cursor = Cursor::default();
        }
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() < cursor.offset {
            cursor = Cursor::default();
        }
        file.seek(SeekFrom::Start(cursor.offset))
            .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(file);
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        if full {
            tx.execute(
                "DELETE FROM usage_issues WHERE source=?1 AND file=?2",
                params![source, path_text],
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some((_, previous)) = &existing {
            if previous != path_text.as_ref() {
                tx.execute(
                    "UPDATE usage_records SET file=?1 WHERE source=?2 AND file=?3",
                    params![path_text, source, previous],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    "UPDATE usage_issues SET file=?1 WHERE source=?2 AND file=?3",
                    params![path_text, source, previous],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| e.to_string())?;
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            cursor.offset += read as u64;
            cursor.line += 1;
            if !parser::relevant(&line) {
                continue;
            }
            let parsed = serde_json::from_slice::<Value>(&line)
                .map_err(|e| e.to_string())
                .and_then(|event| cursor.parse(&event, source, &path_text));
            match parsed {
                Ok(parsed) => {
                    if cursor.needs_baseline() {
                        let mut statement=tx.prepare("SELECT cursor FROM usage_files WHERE source=?1 AND name<?2 ORDER BY name DESC").map_err(|e|e.to_string())?;
                        let previous = statement
                            .query_map(params![source, name], |r| r.get::<_, String>(0))
                            .map_err(|e| e.to_string())?;
                        for value in previous {
                            let previous: Cursor =
                                serde_json::from_str(&value.map_err(|e| e.to_string())?)
                                    .map_err(|e| e.to_string())?;
                            if cursor.inherit_baseline(&previous) {
                                break;
                            }
                        }
                    }
                    if let Some(record) = parsed.record {
                        save_record(&tx, source, record, projects)?;
                    }
                    if let Some(quota) = parsed.quota {
                        let key = quota.data["limit_id"].as_str().unwrap_or_default();
                        tx.execute("INSERT INTO usage_quota_snapshots VALUES(?1,?2,?3,?4) ON CONFLICT(source,limit_id) DO UPDATE SET at=excluded.at,value=excluded.value WHERE excluded.at>=usage_quota_snapshots.at",params![source,key,quota.at,serde_json::to_string(&quota).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
                    }
                }
                Err(reason) => {
                    tx.execute(
                        "INSERT OR REPLACE INTO usage_issues VALUES(?1,?2,?3,?4)",
                        params![source, path_text, cursor.line, reason],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        if !cursor.thread.is_empty() {
            tx.execute("INSERT INTO usage_threads VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source,id) DO UPDATE SET title=COALESCE(excluded.title,usage_threads.title)",params![source,cursor.thread,cursor.parent,cursor.role,titles.get(&cursor.thread)]).map_err(|e|e.to_string())?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO usage_files VALUES(?1,?2,?3,?4)",
            params![
                source,
                name,
                path_text,
                serde_json::to_string(&cursor).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn scan_harness_file(
        &mut self,
        path: &Path,
        source: &str,
        projects: &mut HashMap<String, String>,
        full: bool,
    ) -> Result<(), String> {
        let name = path
            .parent()
            .and_then(Path::file_name)
            .ok_or("Harness 会话路径缺少会话目录")?
            .to_string_lossy();
        let path_text = path.to_string_lossy();
        let revision = crate::harness::revision(path)?;
        let existing = self
            .db
            .query_row(
                "SELECT cursor,path FROM usage_files WHERE source=?1 AND name=?2",
                params![source, name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if !full
            && existing.as_ref().is_some_and(|(cursor, previous)| {
                cursor == &revision && previous == path_text.as_ref()
            })
        {
            return Ok(());
        }
        let session = crate::harness::read_session(path)?;
        let id = session.header["id"].as_str().ok_or("Harness 会话缺少 id")?;
        let thread = format!("harness:{id}");
        let parent = session.header["parentSession"]
            .as_str()
            .map(|id| format!("harness:{id}"));
        let role = if session.header["origin"] == "subagent" {
            "agent"
        } else {
            "main"
        };
        let cwd = session.header["cwd"].as_str().unwrap_or_default();
        let mut cursor = harness_parser::Cursor::new(id, cwd, role);
        let mut records = BTreeMap::new();
        let mut issues = BTreeMap::new();
        let mut title: Option<String> = None;
        if !session.warnings.is_empty() {
            issues.insert(0, session.warnings.join("；"));
        }
        for event in &session.events {
            let value = &event.value;
            let inherited = session
                .seed_cutoff
                .is_some_and(|cutoff| value["seq"].as_u64().is_some_and(|seq| seq <= cutoff));
            if inherited
                && !matches!(
                    value["type"].as_str(),
                    Some("request/header" | "working-directory/change")
                )
            {
                continue;
            }
            if value["type"] == "session/title" {
                title = value["data"]["title"].as_str().map(str::to_string);
            }
            match cursor.parse(value, source, &path_text, event.line as i64) {
                Ok(Some(parsed)) => {
                    if let Some(warning) = parsed.warning {
                        issues.insert(event.line as i64, warning);
                    }
                    records.insert(parsed.record.id.clone(), parsed.record);
                }
                Ok(None) => {}
                Err(reason) => {
                    issues.insert(event.line as i64, reason);
                }
            }
        }
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM usage_records WHERE source=?1 AND thread_id=?2",
            params![source, thread],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM usage_issues WHERE source=?1 AND file IN (?2,?3)",
            params![source, path_text, existing.as_ref().map(|(_, path)| path)],
        )
        .map_err(|e| e.to_string())?;
        for (_, record) in records {
            save_record(&tx, source, record, projects)?;
        }
        for (line, reason) in issues {
            tx.execute(
                "INSERT INTO usage_issues VALUES(?1,?2,?3,?4)",
                params![source, path_text, line, reason],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("INSERT INTO usage_threads VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source,id) DO UPDATE SET parent=excluded.parent,role=excluded.role,title=excluded.title", params![source, thread, parent, role, title]).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO usage_files VALUES(?1,?2,?3,?4)",
            params![source, name, path_text, revision],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn aggregate(&self, f: &Filters) -> Result<Summary, String> {
        self.db
            .query_row(
                &format!("{PRICED} SELECT {AGG} FROM priced r WHERE {WHERE}"),
                params_from_iter(values(f)),
                |r| summary(r, 0),
            )
            .map_err(|e| e.to_string())
    }

    fn groups(&self, f: &Filters, column: &str) -> Result<Vec<Group>, String> {
        self.db.prepare(&format!("{PRICED} SELECT r.{column},{AGG} FROM priced r WHERE {WHERE} GROUP BY r.{column} ORDER BY SUM(r.input+r.output) DESC")).map_err(|e|e.to_string())?.query_map(params_from_iter(values(f)),|r|Ok(Group{key:r.get(0)?,summary:summary(r,1)?})).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    fn choices(&self, column: &str, f: &Filters) -> Result<Vec<String>, String> {
        self.db
            .prepare(&format!(
                "SELECT DISTINCT {column} FROM usage_records WHERE source IN (?1,?2) AND (?3 IS NULL OR client=?3) ORDER BY {column}"
            ))
            .map_err(|e| e.to_string())?
            .query_map(params![f.source, f.harness_source, f.client], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn overview(
        &self,
        f: &Filters,
        directories: &Directories,
        status: ScanStatus,
    ) -> Result<Overview, String> {
        let _read = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut trend = Vec::new();
        for b in &f.buckets {
            let bucket = Filters {
                start: b.start.max(f.start),
                end: b.end.min(f.end),
                buckets: vec![],
                ..f.clone()
            };
            trend.push(Trend {
                label: b.label.clone(),
                start: b.start,
                summary: self.aggregate(&bucket)?,
            });
        }
        let quota = self
            .db
            .query_row(
                "SELECT value FROM usage_quota_snapshots WHERE source=?1 ORDER BY at DESC LIMIT 1",
                [if f.client.as_deref() == Some("harness") {
                    None
                } else {
                    Some(&f.source)
                }],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()?;
        let issue_count = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM usage_issues WHERE source IN (?1,?2)",
                params_from_iter(sources(&f.source, &f.harness_source, f.client.as_deref())),
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(Overview {
            summary: self.aggregate(f)?,
            models: self.groups(f, "model")?,
            projects: self.groups(f, "project")?,
            roles: self.choices("role", f)?,
            available_models: self.choices("model", f)?,
            available_projects: self.choices("project", f)?,
            trend,
            quota,
            issue_count,
            status,
            preferences: self.preferences()?,
            directory: directories.codex.clone(),
            directories: directories.clone(),
        })
    }

    fn session_sql() -> String {
        format!("{PRICED} SELECT r.thread_id,COALESCE(t.title,SUBSTR(r.thread_id,1,4)||'…'||SUBSTR(r.thread_id,-4)),t.parent,r.role,CASE WHEN COUNT(DISTINCT r.model)>1 THEN '*' ELSE MIN(r.model) END,CASE WHEN COUNT(DISTINCT r.project)>1 THEN '*' ELSE MIN(r.project) END,MIN(r.at),MAX(r.at),{AGG},r.client FROM priced r LEFT JOIN usage_threads t ON t.id=r.thread_id AND t.source=r.source WHERE {WHERE}")
    }

    pub fn sessions(&self, f: &Filters, page: u32, sort: &str) -> Result<SessionPage, String> {
        let _read = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        let order = match sort {
            "recent" => "MAX(r.at)",
            "tokens" => "SUM(r.input+r.output)",
            "cost" => "SUM(r.cost)",
            _ => return Err("无效排序方式".into()),
        };
        let sql = format!(
            "{} GROUP BY r.thread_id ORDER BY {order} DESC,r.thread_id LIMIT 20 OFFSET ?9",
            Self::session_sql()
        );
        let mut args = values(f);
        args.push((i64::from(page) * 20).into());
        let items = self
            .db
            .prepare(&sql)
            .map_err(|e| e.to_string())?
            .query_map(params_from_iter(args), session)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(SessionPage {
            items,
            total: self.aggregate(f)?.sessions,
        })
    }

    pub fn records(
        &self,
        f: &Filters,
        thread: Option<&str>,
        page: Option<u32>,
    ) -> Result<Vec<UsageRecord>, String> {
        let sql=format!("{PRICED} SELECT id,thread_id,turn_id,at,model,provider,tier,project,role,input,cached,cache_write,output,reasoning,cost,file,line,basis,client FROM priced r WHERE {WHERE} AND (?9 IS NULL OR thread_id=?9) ORDER BY at DESC,id LIMIT ?10 OFFSET ?11");
        let mut args = values(f);
        args.push(
            thread
                .map(|s| SqlValue::Text(s.into()))
                .unwrap_or(SqlValue::Null),
        );
        args.push(if page.is_some() { 100i64 } else { -1 }.into());
        args.push((i64::from(page.unwrap_or(0)) * 100).into());
        self.db
            .prepare(&sql)
            .map_err(|e| e.to_string())?
            .query_map(params_from_iter(args), |r| {
                Ok(UsageRecord {
                    id: r.get(0)?,
                    client: r.get(18)?,
                    thread_id: r.get(1)?,
                    turn_id: r.get(2)?,
                    at: r.get(3)?,
                    model: r.get(4)?,
                    provider: r.get(5)?,
                    tier: r.get(6)?,
                    project: r.get(7)?,
                    role: r.get(8)?,
                    tokens: Tokens {
                        input: r.get(9)?,
                        cached: r.get(10)?,
                        cache_write: r.get(11)?,
                        output: r.get(12)?,
                        reasoning: r.get(13)?,
                    },
                    cost: r.get(14)?,
                    file: r.get(15)?,
                    line: r.get(16)?,
                    basis: r.get(17)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn detail(&self, f: &Filters, thread: &str, page: u32) -> Result<Detail, String> {
        let _read = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut args = values(f);
        args.push(thread.to_string().into());
        let session = self
            .db
            .query_row(
                &format!(
                    "{} AND r.thread_id=?9 GROUP BY r.thread_id",
                    Self::session_sql()
                ),
                params_from_iter(args),
                session,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("当前范围中没有该会话用量")?;
        let total_records = session.summary.records;
        let parts_sql=format!("{PRICED} SELECT COALESCE(SUM((r.input-r.cached-r.cache_write)*r.price_input/1000000.0),0),COALESCE(SUM(r.cached*COALESCE(r.price_cached,0)/1000000.0),0),COALESCE(SUM(r.cache_write*COALESCE(r.price_write,0)/1000000.0),0),COALESCE(SUM(r.output*r.price_output/1000000.0),0) FROM priced r WHERE {WHERE} AND r.thread_id=?9 AND r.cost IS NOT NULL");
        let mut args = values(f);
        args.push(thread.to_string().into());
        let cost_parts = self
            .db
            .query_row(&parts_sql, params_from_iter(args), |r| {
                Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])
            })
            .map_err(|e| e.to_string())?;
        let groups_sql=format!("{PRICED} SELECT r.model,r.provider,r.tier,r.price_input,r.price_cached,r.price_write,r.price_output,r.price_source,SUM(r.input),SUM(r.cached),SUM(r.cache_write),SUM(r.output),r.price_custom FROM priced r WHERE {WHERE} AND r.thread_id=?9 AND r.cost IS NOT NULL GROUP BY r.model,r.provider,r.tier,r.price_input,r.price_cached,r.price_write,r.price_output,r.price_source,r.price_custom ORDER BY SUM(r.input+r.output) DESC");
        let mut args = values(f);
        args.push(thread.to_string().into());
        let cost_groups = self
            .db
            .prepare(&groups_sql)
            .map_err(|e| e.to_string())?
            .query_map(params_from_iter(args), |r| {
                Ok(CostGroup {
                    price: Price {
                        model: r.get(0)?,
                        provider: r.get(1)?,
                        tier: r.get(2)?,
                        input: r.get(3)?,
                        cached: r.get(4)?,
                        cache_write: r.get(5)?,
                        output: r.get(6)?,
                        source: r.get(7)?,
                        custom: r.get(12)?,
                    },
                    input: r.get(8)?,
                    cached: r.get(9)?,
                    cache_write: r.get(10)?,
                    output: r.get(11)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(Detail {
            session,
            records: self.records(f, Some(thread), Some(page))?,
            total_records,
            cost_parts,
            cost_groups,
            memories: vec![],
        })
    }

    pub fn prices(&self) -> Result<Vec<Price>, String> {
        self.db.prepare("SELECT k.model,k.provider,k.tier,
            CASE WHEN p.model IS NOT NULL THEN p.input ELSE o.input END,
            CASE WHEN p.model IS NOT NULL THEN p.cached ELSE o.cached END,
            CASE WHEN p.model IS NOT NULL THEN p.cache_write ELSE o.cache_write END,
            CASE WHEN p.model IS NOT NULL THEN p.output ELSE o.output END,
            CASE WHEN p.model IS NOT NULL THEN p.source ELSE COALESCE(o.source,'暂无已核实的官方参考价') END,p.model IS NOT NULL
            FROM (SELECT model,provider,tier FROM usage_records UNION SELECT model,provider,tier FROM usage_prices) k
            LEFT JOIN usage_prices p ON p.model=k.model AND p.provider=k.provider AND p.tier=k.tier
                AND (p.input IS NOT NULL OR p.cached IS NOT NULL OR p.cache_write IS NOT NULL OR p.output IS NOT NULL)
            LEFT JOIN usage_official_prices o ON p.model IS NULL AND o.model=k.model AND o.min_input=0
                AND o.tier=CASE k.tier WHEN '' THEN 'standard' WHEN 'default' THEN 'standard' WHEN 'auto' THEN 'standard' WHEN 'priority' THEN 'fast' ELSE k.tier END
                AND ?1>=o.valid_from AND (o.valid_until IS NULL OR ?1<o.valid_until)
            ORDER BY k.model,k.provider,k.tier")
            .map_err(|e|e.to_string())?.query_map([Utc::now().timestamp_millis()],|r|Ok(Price{model:r.get(0)?,provider:r.get(1)?,tier:r.get(2)?,input:r.get(3)?,cached:r.get(4)?,cache_write:r.get(5)?,output:r.get(6)?,source:r.get(7)?,custom:r.get(8)?}))
            .map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn save_prices(&self, prices: &[Price]) -> Result<(), String> {
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        for p in prices {
            if !p.custom
                || [p.input, p.cached, p.cache_write, p.output]
                    .iter()
                    .all(Option::is_none)
            {
                tx.execute(
                    "DELETE FROM usage_prices WHERE model=?1 AND provider=?2 AND tier=?3",
                    params![p.model, p.provider, p.tier],
                )
                .map_err(|e| e.to_string())?;
                continue;
            }
            tx.execute(
                "INSERT OR REPLACE INTO usage_prices VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    p.model,
                    p.provider,
                    p.tier,
                    p.input,
                    p.cached,
                    p.cache_write,
                    p.output,
                    p.source
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn issues(
        &self,
        codex: &str,
        harness: &str,
        client: Option<&str>,
    ) -> Result<Vec<Issue>, String> {
        self.db.prepare("SELECT file,line,reason,CASE WHEN source LIKE 'harness:%' THEN 'harness' ELSE 'codex' END FROM usage_issues WHERE source IN (?1,?2) ORDER BY file,line LIMIT 500").map_err(|e|e.to_string())?.query_map(params_from_iter(sources(codex,harness,client)),|r|Ok(Issue{file:r.get(0)?,line:r.get(1)?,reason:r.get(2)?,client:r.get(3)?})).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn source_view(&self, record: &str, directory: &Path) -> Result<String, String> {
        let path = self.source_path(record)?;
        let client: String = self
            .db
            .query_row(
                "SELECT client FROM usage_records WHERE id=?1",
                [record],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if client != "harness" {
            return Ok(path);
        }
        let revision = crate::harness::revision(Path::new(&path))?;
        let name = blake3::hash(format!("{path}\n{revision}").as_bytes()).to_hex();
        let view = directory.join(format!("{name}.jsonl"));
        if !view.is_file() {
            let log = crate::harness::read_session(Path::new(&path))?;
            let content = source_jsonl(&log)?;
            fs::create_dir_all(directory).map_err(|e| e.to_string())?;
            let mut temporary =
                tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
            temporary
                .write_all(content.as_bytes())
                .map_err(|e| e.to_string())?;
            let file = temporary.persist(&view).map_err(|e| e.error.to_string())?;
            // Windows persist 会重设文件属性，落盘后再锁定只读副本。
            let mut permissions = file.metadata().map_err(|e| e.to_string())?.permissions();
            permissions.set_readonly(true);
            file.set_permissions(permissions)
                .map_err(|e| e.to_string())?;
        }
        Ok(view.to_string_lossy().into_owned())
    }

    pub fn source_path(&self, record: &str) -> Result<String, String> {
        let path = self
            .db
            .query_row(
                "SELECT file FROM usage_records WHERE id=?1",
                [record],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?;
        if !Path::new(&path).is_file() {
            return Err("源日志已不在本地".into());
        }
        Ok(path)
    }
}
