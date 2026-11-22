use super::*;
use crate::transfer::{
    ImportPreview, LinkPreviewRow, MemoryPackage, MemoryPreviewRow, PortableLink, PortableMemory,
    PortableRepository,
};

struct IncomingMemory {
    id: String,
    repository: Repository,
    raw: String,
    document: Document,
    related: Vec<String>,
}

pub struct PreparedMemoryImport {
    entries: Vec<IncomingMemory>,
    repositories: Vec<Repository>,
    links: Vec<PortableLink>,
    invalid_links: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryImportResult {
    pub added_memories: usize,
    pub existing_memories: usize,
    pub new_links: usize,
    pub invalid_links: usize,
}

struct PendingDocument {
    path: PathBuf,
    raw: String,
    document: Document,
    related: Vec<String>,
    changed: bool,
    is_new: bool,
}

impl Vault {
    pub fn memory_package(&self) -> Result<MemoryPackage, String> {
        let graph = self.graph()?;
        let memories = graph
            .nodes
            .into_iter()
            .map(|node| {
                let repository = self
                    .repositories
                    .get(&node.repository_id)
                    .ok_or("未找到记忆所属仓库")?;
                Ok(PortableMemory {
                    id: node.id,
                    repository: repository_key(repository),
                    markdown: fs::read_to_string(&node.path)
                        .map_err(|error| format!("无法读取记忆原文：{error}"))?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(MemoryPackage {
            package_type: "aimemory.memories".into(),
            version: 1,
            memories,
            links: graph
                .edges
                .into_iter()
                .map(|edge| PortableLink {
                    from: edge.source,
                    to: edge.target,
                })
                .collect(),
            repositories: self
                .repositories
                .values()
                .map(|repository| PortableRepository {
                    id: repository.id.clone(),
                    name: repository.name.clone(),
                    remote: repository.remote.clone(),
                })
                .collect(),
        })
    }

    pub fn prepare_memory_import(
        &self,
        packet: MemoryPackage,
        preview: &mut ImportPreview,
    ) -> Result<PreparedMemoryImport, String> {
        let mut repositories = BTreeMap::<String, Repository>::new();
        for portable in packet.repositories {
            validate_identifier(&portable.id, "仓库")?;
            if portable.name.trim().is_empty() {
                return Err("仓库名称不能为空".into());
            }
            let remote = if portable.remote.trim().is_empty() {
                String::new()
            } else {
                normalize_remote(&portable.remote)?
            };
            let id = if remote.is_empty() {
                portable.id
            } else {
                let id = repository_id(&remote);
                if id != portable.id {
                    return Err("仓库标识与 Git 远程地址不一致".into());
                }
                id
            };
            let repository = Repository {
                id,
                name: portable.name,
                remote,
                memory_count: 0,
            };
            if repositories
                .insert(repository_key(&repository), repository)
                .is_some()
            {
                return Err("文件包含重复仓库标识".into());
            }
        }
        let graph = self.graph()?;
        let known: BTreeMap<_, _> = graph
            .nodes
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        let current_edges: BTreeSet<_> = graph
            .edges
            .into_iter()
            .map(|edge| (edge.source, edge.target))
            .collect();
        let mut ids: BTreeSet<_> = known.keys().cloned().collect();
        let mut incoming_ids = BTreeSet::new();
        let mut entries = Vec::new();
        let mut links = packet.links;
        for memory in packet.memories {
            validate_identifier(&memory.id, "记忆")?;
            if !incoming_ids.insert(memory.id.clone()) {
                return Err(format!("文件包含重复记忆 ID：{}", memory.id));
            }
            if memory.markdown.trim().is_empty() {
                return Err(format!("记忆 {} 的 Markdown 不能为空", memory.id));
            }
            let repository = if let Some(local_id) = memory.repository.strip_prefix("local:") {
                validate_identifier(local_id, "仓库")?;
                repositories
                    .get(&memory.repository)
                    .cloned()
                    .ok_or("本地仓库缺少 repositories 元信息")?
            } else {
                let remote = normalize_remote(&memory.repository)?;
                repositories
                    .entry(remote.clone())
                    .or_insert_with(|| Repository {
                        id: repository_id(&remote),
                        name: remote.rsplit('/').next().unwrap_or("repository").into(),
                        remote,
                        memory_count: 0,
                    })
                    .clone()
            };
            let document = parse_document_text(&memory.markdown)?;
            if let Some(declared_id) = field(&document.meta, "repository_id") {
                validate_identifier(&declared_id, "仓库")?;
            }
            if field(&document.meta, "id").is_some_and(|id| id != memory.id) {
                return Err(format!("记忆 {} 的原文 ID 与文件条目不一致", memory.id));
            }
            if let Some(remote) = field(&document.meta, "repository_remote")
                .filter(|remote| !remote.trim().is_empty())
            {
                if normalize_remote(&remote)? != repository.remote {
                    return Err(format!("记忆 {} 的原文仓库与文件条目不一致", memory.id));
                }
            } else if repository.remote.is_empty()
                && field(&document.meta, "repository_id").is_some_and(|id| id != repository.id)
            {
                return Err(format!("记忆 {} 的本地仓库标识不一致", memory.id));
            }
            let related = strings(&document.meta, "related_ids")?;
            strings(&document.meta, "tags")?;
            links.extend(related.iter().cloned().map(|target| PortableLink {
                from: memory.id.clone(),
                to: target,
            }));
            let local = known.get(&memory.id);
            let title = field(&document.meta, "title")
                .unwrap_or_else(|| title_from_markdown(&document.markdown, Path::new(&memory.id)));
            preview.memories.push(MemoryPreviewRow {
                id: memory.id.clone(),
                title,
                repository: repository.name.clone(),
                status: if local.is_some() { "existing" } else { "new" }.into(),
                markdown: memory.markdown.clone(),
                local_markdown: local
                    .map(|node| fs::read_to_string(&node.path).map_err(|error| error.to_string()))
                    .transpose()?,
                local_repository: local.and_then(|node| {
                    self.repositories
                        .get(&node.repository_id)
                        .map(|repo| repo.name.clone())
                }),
            });
            if local.is_some() {
                preview.existing_memories += 1;
            } else {
                preview.added_memories += 1;
            }
            ids.insert(memory.id.clone());
            entries.push(IncomingMemory {
                id: memory.id,
                repository,
                raw: memory.markdown,
                document,
                related,
            });
        }
        let unique: BTreeSet<_> = links
            .into_iter()
            .map(|link| canonical_link(&link.from, &link.to))
            .collect();
        let mut valid = Vec::new();
        for (from, to) in unique {
            let status = if from == to || !ids.contains(&from) || !ids.contains(&to) {
                preview.invalid_links += 1;
                "invalid"
            } else {
                valid.push(PortableLink {
                    from: from.clone(),
                    to: to.clone(),
                });
                if current_edges.contains(&(from.clone(), to.clone())) {
                    "existing"
                } else {
                    preview.new_links += 1;
                    "new"
                }
            };
            preview.links.push(LinkPreviewRow {
                from,
                to,
                status: status.into(),
            });
        }
        Ok(PreparedMemoryImport {
            entries,
            repositories: repositories.into_values().collect(),
            links: valid,
            invalid_links: preview.invalid_links,
        })
    }
}

impl PreparedMemoryImport {
    pub fn apply(self, vault: &mut Vault) -> Result<MemoryImportResult, String> {
        let graph = vault.graph()?;
        let known: BTreeMap<_, _> = graph
            .nodes
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        let current_edges: BTreeSet<_> = graph
            .edges
            .into_iter()
            .map(|edge| (edge.source, edge.target))
            .collect();
        let mut available: BTreeSet<_> = known.keys().cloned().collect();
        available.extend(self.entries.iter().map(|entry| entry.id.clone()));
        let mut documents = BTreeMap::<String, PendingDocument>::new();
        let mut added_memories = 0;
        let mut existing_memories = 0;
        for entry in self.entries {
            if known.contains_key(&entry.id) {
                existing_memories += 1;
                continue;
            }
            let mut document = entry.document;
            let mut changed = false;
            for (key, value) in [
                ("id", entry.id.as_str()),
                ("repository_id", entry.repository.id.as_str()),
                ("repository_name", entry.repository.name.as_str()),
                ("repository_remote", entry.repository.remote.as_str()),
            ] {
                if field(&document.meta, key).is_none() {
                    set(&mut document.meta, key, &value)?;
                    changed = true;
                }
            }
            let related = entry.related;
            let filtered: Vec<_> = related
                .iter()
                .filter(|target| *target != &entry.id && available.contains(*target))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if related.iter().cloned().collect::<BTreeSet<_>>()
                != filtered.iter().cloned().collect()
            {
                set(&mut document.meta, "related_ids", &filtered)?;
                changed = true;
            }
            let directory = format!(
                "imported-{}",
                &blake3::hash(entry.repository.id.as_bytes()).to_hex()[..12]
            );
            let filename = format!(
                "memory-{}.md",
                &blake3::hash(entry.id.as_bytes()).to_hex()[..24]
            );
            documents.insert(
                entry.id,
                PendingDocument {
                    path: vault.root.join(directory).join(filename),
                    raw: entry.raw,
                    document,
                    related: filtered,
                    changed,
                    is_new: true,
                },
            );
            added_memories += 1;
        }
        let mut represented = current_edges.clone();
        for (id, pending) in &documents {
            for target in &pending.related {
                represented.insert(canonical_link(id, target));
            }
        }
        let mut invalid_links = self.invalid_links;
        let mut new_links = 0;
        for link in self.links {
            if !available.contains(&link.from) || !available.contains(&link.to) {
                invalid_links += 1;
                continue;
            }
            let edge = (link.from.clone(), link.to.clone());
            if !current_edges.contains(&edge) {
                new_links += 1;
            }
            if represented.contains(&edge) {
                continue;
            }
            if !documents.contains_key(&link.from) {
                let local = known.get(&link.from).unwrap();
                let path = PathBuf::from(&local.path);
                let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                let document = parse_document_text(&raw)?;
                let related = strings(&document.meta, "related_ids")?;
                documents.insert(
                    link.from.clone(),
                    PendingDocument {
                        raw,
                        document,
                        related,
                        path,
                        changed: false,
                        is_new: false,
                    },
                );
            }
            let pending = documents.get_mut(&link.from).unwrap();
            if !pending.related.contains(&link.to) {
                pending.related.push(link.to);
            }
            set(&mut pending.document.meta, "related_ids", &pending.related)?;
            pending.changed = true;
            represented.insert(edge);
        }
        let mut writes: Vec<_> = documents.into_values().collect();
        writes.sort_by_key(|document| !document.is_new);
        for pending in writes {
            if !pending.is_new && !pending.changed {
                continue;
            }
            let raw = if pending.changed {
                format!(
                    "---\n{}---\n{}",
                    serde_yaml_ng::to_string(&pending.document.meta)
                        .map_err(|error| error.to_string())?,
                    pending.document.markdown
                )
            } else {
                pending.raw
            };
            let result = if pending.is_new {
                write_new(&pending.path, raw.as_bytes())
            } else {
                write_atomic(&pending.path, raw.as_bytes())
            };
            if let Err(error) = result {
                let _ = vault.refresh();
                return Err(format!("导入写入未完成，可重新导入继续处理：{error}"));
            }
        }
        for repository in self.repositories {
            vault
                .repositories
                .entry(repository.id.clone())
                .or_insert(repository);
        }
        vault.refresh()?;
        Ok(MemoryImportResult {
            added_memories,
            existing_memories,
            new_links,
            invalid_links,
        })
    }
}

fn repository_key(repository: &Repository) -> String {
    if repository.remote.is_empty() {
        format!("local:{}", repository.id)
    } else {
        repository.remote.clone()
    }
}

fn validate_identifier(id: &str, kind: &str) -> Result<(), String> {
    if id.trim().is_empty() || matches!(id, "." | "..") || id.contains(['/', '\\', '\0']) {
        return Err(format!("{kind} ID 无效，不能包含目录路径"));
    }
    Ok(())
}

fn canonical_link(from: &str, to: &str) -> (String, String) {
    if from <= to {
        (from.into(), to.into())
    } else {
        (to.into(), from.into())
    }
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("记忆保存路径无效")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(contents)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("无法保存新增记忆：{error}"))?;
    Ok(())
}
