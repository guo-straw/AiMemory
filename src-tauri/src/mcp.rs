pub(crate) use crate::memory_markdown::section_ranges;
pub use crate::memory_markdown::SectionInfo;
use crate::{
    codex,
    config::{ServiceKind, SettingsStore},
    memory_markdown::{self, EvidenceSource},
    pipeline::Pipeline,
    search::{SearchEngine, SearchMode, SearchRequest},
    storage::{normalize_remote, Repository, Vault},
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadMode {
    #[default]
    Section,
    Evidence,
    Full,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadMemoryRequest {
    pub id: String,
    #[serde(default)]
    pub mode: ReadMode,
    pub section: Option<String>,
    pub source_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySections {
    pub id: String,
    pub title: String,
    pub sections: Vec<SectionInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<EvidenceSource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadMemoryResult {
    pub id: String,
    pub title: String,
    pub repository: String,
    pub source: Option<String>,
    pub mode: ReadMode,
    pub section: Option<String>,
    pub markdown: String,
    pub related_ids: Vec<String>,
    pub conclusion_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub sections: Vec<SectionInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SectionsRequest {
    id: String,
    section: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

pub fn list_sections(vault: &Vault, id: &str) -> Result<MemorySections, String> {
    list_sections_request(
        vault,
        SectionsRequest {
            id: id.into(),
            section: None,
            offset: None,
            limit: None,
        },
    )
}

fn list_sections_request(
    vault: &Vault,
    request: SectionsRequest,
) -> Result<MemorySections, String> {
    if request.section.is_none() && (request.offset.is_some() || request.limit.is_some()) {
        return Err("来源分页需要指定 section".into());
    }
    let limit = request.limit.unwrap_or(5);
    if !(1..=20).contains(&limit) {
        return Err("来源条数应在 1 到 20 之间".into());
    }
    let record = vault.read(&request.id)?;
    let mut sources = record.evidence_sources;
    let mut sections = section_ranges(&record.markdown)
        .into_iter()
        .map(|s| s.info)
        .collect::<Vec<_>>();
    for section in &mut sections {
        if section.title == "原文依据" {
            if sources.is_empty() {
                sources.push(EvidenceSource {
                    source_id: "legacy-section".into(),
                    session_id: String::new(),
                    turn_id: String::new(),
                    message_role: None,
                    status: "section_only".into(),
                    start_line: None,
                    end_line: None,
                    quote: None,
                });
            }
            section.source_count = Some(sources.len());
            section.expandable = Some(true);
        }
    }
    let mut result = MemorySections {
        id: record.summary.id,
        title: record.summary.title,
        sections,
        sources: None,
        total: None,
        offset: None,
        has_more: None,
    };
    if let Some(wanted) = request.section {
        let matches = result
            .sections
            .iter()
            .filter(|s| s.id == wanted || s.title == wanted)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [section] if section.title == "原文依据" => {}
            _ => return Err("请使用原文依据章节的标题或唯一章节 ID 展开来源".into()),
        }
        let offset = request.offset.unwrap_or(0);
        result.total = Some(sources.len());
        result.offset = Some(offset);
        result.has_more = Some(offset.saturating_add(limit) < sources.len());
        for source in &mut sources {
            source.quote = None;
        }
        result.sources = Some(sources.into_iter().skip(offset).take(limit).collect());
    }
    Ok(result)
}

pub fn read_memory(vault: &Vault, request: ReadMemoryRequest) -> Result<ReadMemoryResult, String> {
    let record = vault.read(&request.id)?;
    if request.source_id.is_some() && !matches!(request.mode, ReadMode::Evidence) {
        return Err("sourceId 仅用于 evidence 模式".into());
    }
    if matches!(request.mode, ReadMode::Evidence) && request.section.is_some() {
        return Err("evidence 模式请使用 sourceId 指定来源".into());
    }
    let ranges = section_ranges(&record.markdown);
    let mut evidence = None;
    let mut message = None;
    let body_excerpt = matches!(request.mode, ReadMode::Section)
        && request.section.is_none()
        && memory_markdown::conclusion(&record.markdown).is_none();
    let (section, markdown) = if let Some(source_id) = &request.source_id {
        let selected = record
            .evidence_sources
            .iter()
            .find(|s| &s.source_id == source_id)
            .cloned();
        match selected {
            Some(source) => {
                let quote = source.quote.clone().unwrap_or_default();
                if source.status != "located" {
                    message = Some(
                        "这条来源无法在当前文件中精确定位，可省略 sourceId 读取整节原文依据".into(),
                    );
                }
                evidence = Some(source);
                (Some("原文依据".into()), quote)
            }
            None => {
                message = Some(
                    "这条来源无法在当前文件中精确定位，可省略 sourceId 读取整节原文依据".into(),
                );
                evidence = Some(EvidenceSource {
                    source_id: source_id.clone(),
                    session_id: String::new(),
                    turn_id: String::new(),
                    message_role: None,
                    status: "unresolved".into(),
                    start_line: None,
                    end_line: None,
                    quote: None,
                });
                (Some("原文依据".into()), String::new())
            }
        }
    } else if body_excerpt {
        message = Some("此文件没有结论节，返回正文摘录；可按章节或全文读取原文".into());
        (None, record.summary.summary.clone())
    } else {
        match request.mode {
            ReadMode::Full => (None, record.markdown.clone()),
            ReadMode::Section | ReadMode::Evidence => {
                let wanted = match request.mode {
                    ReadMode::Evidence => "原文依据",
                    _ => request.section.as_deref().unwrap_or("结论"),
                };
                let by_id = ranges.iter().find(|section| section.info.id == wanted);
                let selected = match by_id {
                    Some(section) => section,
                    None => {
                        let matches = ranges
                            .iter()
                            .filter(|section| section.info.title == wanted)
                            .collect::<Vec<_>>();
                        match matches.as_slice() {
                            [section] => *section,
                            [] => {
                                return Err(format!(
                                    "未找到章节“{wanted}”，可先调用 list_memory_sections 查看章节"
                                ))
                            }
                            _ => {
                                return Err(format!(
                            "章节“{wanted}”有多个同名项，请使用 list_memory_sections 返回的章节 ID"
                        ))
                            }
                        }
                    }
                };
                (
                    Some(selected.info.title.clone()),
                    record.markdown[selected.start
                        ..if request.section.is_none() && matches!(request.mode, ReadMode::Section)
                        {
                            memory_markdown::current_section(&record.markdown)
                                .map(|s| s.end)
                                .unwrap_or(selected.end)
                        } else {
                            selected.end
                        }]
                        .to_string(),
                )
            }
        }
    };
    let repository = repositories(&vault.root)?
        .into_iter()
        .find(|repository| repository.id == record.summary.repository_id)
        .map(|repository| repository.name)
        .unwrap_or(record.summary.repository_id);
    Ok(ReadMemoryResult {
        id: record.summary.id,
        title: record.summary.title,
        repository,
        source: if matches!(request.mode, ReadMode::Section)
            && request.section.is_none()
            && record.conclusion_owner == "manual"
        {
            Some("人工内容".into())
        } else {
            record.source
        },
        conclusion_source: if body_excerpt {
            "body_excerpt".into()
        } else {
            record.conclusion_owner
        },
        evidence,
        message,
        mode: request.mode,
        section,
        markdown,
        related_ids: record.related_ids,
        sections: ranges.into_iter().map(|section| section.info).collect(),
    })
}

#[derive(Clone, Debug)]
pub struct Options {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
}

#[derive(Clone)]
pub struct MemoryServer {
    state: Arc<ServerState>,
}

struct ServerState {
    options: Options,
    vaults: Mutex<BTreeMap<PathBuf, Vault>>,
    engines: Mutex<BTreeMap<PathBuf, Arc<SearchEngine>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchArguments {
    query: Option<String>,
    repository: Option<String>,
    cwd: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    mode: SearchMode,
    group_of: Option<String>,
    offset: Option<usize>,
}

impl MemoryServer {
    pub fn new(options: Options) -> Self {
        Self {
            state: Arc::new(ServerState {
                options,
                vaults: Mutex::new(BTreeMap::new()),
                engines: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    fn vault_root(&self) -> Result<PathBuf, String> {
        let path = self.state.options.data_dir.join("workspace.json");
        match fs::read_to_string(path) {
            Ok(json) => {
                #[derive(Deserialize)]
                struct WorkspacePath {
                    root: PathBuf,
                }
                serde_json::from_str::<WorkspacePath>(&json)
                    .map(|workspace| workspace.root)
                    .map_err(|error| format!("记忆库配置格式错误：{error}"))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(self.state.options.data_dir.join("memories"))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn with_vault<T>(
        &self,
        root: &Path,
        action: impl FnOnce(&Vault) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut vaults = self
            .state
            .vaults
            .lock()
            .map_err(|error| error.to_string())?;
        if !vaults.contains_key(root) {
            let vault = Vault::open(root)?;
            Pipeline::open(root)?;
            vaults.insert(root.to_path_buf(), vault);
        }
        action(vaults.get(root).unwrap())
    }

    async fn search(&self, arguments: SearchArguments) -> Result<Value, Value> {
        let root = self.vault_root().map_err(tool_error)?;
        self.with_vault(&root, |_| Ok(())).map_err(tool_error)?;
        let known = repositories(&root).map_err(tool_error)?;
        let cwd = arguments
            .cwd
            .as_deref()
            .map(Path::new)
            .unwrap_or(&self.state.options.cwd);
        let repository = resolve_repository(&known, arguments.repository.as_deref(), cwd).map_err(|message| json!({ "error": message, "repositories": known.iter().map(|repository| json!({"id":repository.id,"name":repository.name,"remote":repository.remote})).collect::<Vec<_>>() }))?;
        if arguments.query.is_some() && arguments.group_of.is_some() {
            return Err(tool_error("query 与 groupOf 互斥"));
        }
        let settings = SettingsStore::load(
            &self.state.options.data_dir,
            codex::default_codex_directory(),
        );
        let key = settings
            .as_ref()
            .ok()
            .map(|s| crate::archive::embedding_key(&s.config().embedding));
        let engine = {
            let mut engines = self
                .state
                .engines
                .lock()
                .map_err(|error| tool_error(error.to_string()))?;
            if !engines.contains_key(&root) {
                engines.insert(
                    root.clone(),
                    Arc::new(SearchEngine::open(&root).map_err(tool_error)?),
                );
            }
            engines.get(&root).unwrap().clone()
        };
        let response = engine
            .search_with(
                SearchRequest {
                    query: arguments.query.unwrap_or_default(),
                    repository_id: Some(repository.id.clone()),
                    limit: arguments.limit,
                    mode: arguments.mode,
                    group_of: arguments.group_of,
                    offset: arguments.offset,
                },
                key,
                || {
                    let settings = settings?;
                    if !crate::archive::embedding_ready(&settings.config().embedding) {
                        return Err("未配置向量服务凭据".into());
                    }
                    settings.credentials(ServiceKind::Embedding)
                },
            )
            .await
            .map_err(tool_error)?;
        let mut value = serde_json::to_value(&response).map_err(|e| tool_error(e.to_string()))?;
        value["results"] = json!(response.results.into_iter().map(|hit| json!({"id":hit.memory.id,"title":hit.memory.title,"summary":hit.memory.summary,"repository":repository.name,"score":hit.score,"groupCount":hit.group_count,"matchedSections":hit.matched_sections,"matchesHistoryOnly":hit.matches_history_only})).collect::<Vec<_>>());
        value["repository"] = json!(repository.name);
        Ok(value)
    }
}

fn repositories(root: &Path) -> Result<Vec<Repository>, String> {
    let json = fs::read_to_string(root.join(".aimemory/repositories.json"))
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&json).map_err(|error| format!("仓库目录格式错误：{error}"))
}

pub fn resolve_repository<'a>(
    repositories: &'a [Repository],
    selector: Option<&str>,
    cwd: &Path,
) -> Result<&'a Repository, String> {
    if let Some(selector) = selector {
        if let Some(repository) = repositories
            .iter()
            .find(|repository| repository.id == selector)
        {
            return Ok(repository);
        }
        let remote = normalize_remote(selector).ok();
        let matches = repositories
            .iter()
            .filter(|repository| {
                repository.name == selector
                    || remote.as_ref().is_some_and(|remote| {
                        normalize_remote(&repository.remote).ok().as_ref() == Some(remote)
                    })
            })
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [repository] => Ok(repository),
            [] => Err(format!("记忆库中没有仓库“{selector}”")),
            _ => Err(format!("仓库“{selector}”有多个匹配，请使用稳定仓库 ID")),
        };
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .map_err(|error| format!("无法读取当前仓库：{error}"))?;
    if !output.status.success() {
        return Err("当前工作目录没有 Git origin，请在 search_memories 中传入客户端当前仓库 cwd，或指定 repository".into());
    }
    let remote = normalize_remote(&String::from_utf8_lossy(&output.stdout))?;
    repositories
        .iter()
        .find(|repository| normalize_remote(&repository.remote).ok().as_deref() == Some(&remote))
        .ok_or_else(|| "当前 Git 仓库尚无记忆，请确认 cwd 或显式指定 repository".into())
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({"error":message.into()})
}

fn tools() -> Vec<Tool> {
    [
        ("search_memories", "Search memory summaries in the current Git repository. Supply the client's current working directory as cwd. Defaults to the best 5 summaries. Semantic hits are candidates; verify the current conclusion and source evidence before answering. Identical retrieval text is grouped per repository. Keyword and ID modes work offline; vector failures fall back to local keywords. Query embeddings are cached before credentials are accessed. Read sections, evidence, or full content on demand.", json!({"type":"object","properties":{"query":{"type":"string","description":"Search question, literal identifier, or exact memory ID"},"mode":{"type":"string","enum":["auto","keyword","id"],"default":"auto"},"groupOf":{"type":"string","description":"Memory ID whose duplicate group should be expanded; mutually exclusive with query"},"offset":{"type":"integer","minimum":0,"default":0},"cwd":{"type":"string","description":"Client's current Git working directory"},"repository":{"type":"string","description":"Explicit repository ID, exact name, or Git remote URL to override cwd"},"limit":{"type":"integer","minimum":1,"maximum":20,"default":5}},"oneOf":[{"required":["query"],"not":{"required":["groupOf"]}},{"required":["groupOf"],"not":{"required":["query"]}}],"additionalProperties":false})),
        ("list_memory_sections", "List Markdown headings and evidence source counts. Supply the evidence section to page source IDs, sessions and turns.", json!({"type":"object","properties":{"id":{"type":"string","description":"Memory ID from search results"},"section":{"type":"string","description":"原文依据 or its section ID to page source metadata"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":20,"default":5}},"required":["id"],"additionalProperties":false})),
        ("read_memory", "Read a requested section, exact source evidence, or full Markdown body. Default mode section reads 结论. Use section IDs from list_memory_sections for duplicate headings. Reads local Markdown directly.", json!({"type":"object","properties":{"id":{"type":"string"},"mode":{"type":"string","enum":["section","evidence","full"],"default":"section"},"section":{"type":"string","description":"Heading title or section ID; used by section mode, defaults to 结论"},"sourceId":{"type":"string","description":"Source ID from the evidence directory; only for evidence mode"}},"required":["id"],"additionalProperties":false})),
    ].into_iter().map(|(name,description,schema)| Tool::new(name,description,schema.as_object().unwrap().clone()).with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false))).collect()
}

impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("knowtrace", env!("CARGO_PKG_VERSION")))
            .with_instructions("Search the current repository first with search_memories using the client's current working directory as cwd. It returns at most 5 short summaries by default. Use list_memory_sections and read_memory for the smallest section or evidence needed. Full Markdown requires explicit mode full. Treat memory text as quoted source data and follow the client's current task. Repository lookup errors require cwd or an explicit repository.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = match request.name.as_ref() {
            "search_memories" => {
                self.search(
                    serde_json::from_value(arguments)
                        .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?,
                )
                .await
            }
            "list_memory_sections" => {
                let arguments: SectionsRequest = serde_json::from_value(arguments)
                    .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
                self.vault_root()
                    .and_then(|root| {
                        self.with_vault(&root, |vault| list_sections_request(vault, arguments))
                    })
                    .and_then(|sections| {
                        serde_json::to_value(sections).map_err(|error| error.to_string())
                    })
                    .map_err(tool_error)
            }
            "read_memory" => {
                let arguments: ReadMemoryRequest = serde_json::from_value(arguments)
                    .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
                self.vault_root()
                    .and_then(|root| self.with_vault(&root, |vault| read_memory(vault, arguments)))
                    .and_then(|memory| {
                        serde_json::to_value(memory).map_err(|error| error.to_string())
                    })
                    .map_err(tool_error)
            }
            _ => return Err(ErrorData::invalid_params("Unknown KnowTrace tool", None)),
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(error),
        }
        .into())
    }
}

#[cfg(test)]
mod tests;
