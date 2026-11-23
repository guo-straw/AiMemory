use super::*;

#[test]
fn usage_detail_finds_each_archived_memory_once_by_session() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repo = add_repository(&mut vault, "usage-project");
    let source = ArchiveSource {
        source_id: "codex:usage-session:turn-1".into(),
        session_id: "usage-session".into(),
        turn_id: "turn-1".into(),
        input_truncated: false,
        message_role: None,
    };
    let input = || new_memory(&repo, "用量来源", "## 结论\n可按会话找到归档。\n");
    vault
        .save_archive("ARC-usage-source", input(), &source, &[])
        .unwrap();
    let followup = ArchiveSource {
        source_id: "codex:usage-session:turn-2".into(),
        turn_id: "turn-2".into(),
        ..source
    };
    vault
        .save_archive("ARC-usage-source", input(), &followup, &[])
        .unwrap();
    let found = vault.memories_for_session("usage-session").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "ARC-usage-source");
    assert!(vault
        .memories_for_session("other-session")
        .unwrap()
        .is_empty());
}

#[test]
fn repository_rename_keeps_identity_and_delete_removes_links_after_rebuild() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let frontend = add_repository(&mut vault, "frontend");
    let backend = add_repository(&mut vault, "backend");
    let first = vault
        .save(new_memory(&frontend, "前端轮询", "前端展示状态。"))
        .unwrap();
    let mut input = new_memory(&backend, "任务接口", "后端返回状态。");
    input.related_ids.push(first.summary.id.clone());
    let second = vault.save(input).unwrap();
    let original = fs::read(&first.summary.path).unwrap();
    assert!(vault.rename_repository(&frontend.id, "  ").is_err());
    assert_eq!(fs::read(&first.summary.path).unwrap(), original);
    vault
        .rename_repository(&frontend.id, "  前端仓库  ")
        .unwrap();
    let renamed = vault
        .workspace()
        .unwrap()
        .repositories
        .into_iter()
        .find(|r| r.id == frontend.id)
        .unwrap();
    assert_eq!(renamed.name, "前端仓库");
    assert_eq!(renamed.remote, frontend.remote);
    assert_eq!(renamed.memory_count, 1);
    assert!(fs::read_to_string(&first.summary.path)
        .unwrap()
        .contains("repository_name: 前端仓库"));
    assert_eq!(
        vault.read(&first.summary.id).unwrap().related_ids,
        vec![second.summary.id.clone()]
    );
    vault.rebuild().unwrap();
    let moved_backend = root.path().join(&frontend.id).join("backend-note.md");
    fs::rename(&second.summary.path, &moved_backend).unwrap();
    fs::write(
        root.path().join(&frontend.id).join("invalid.md"),
        "---\ninvalid: [\n---\n",
    )
    .unwrap();
    vault.refresh().unwrap();
    assert_eq!(vault.workspace().unwrap().warnings.len(), 1);
    vault.delete_repository(&frontend.id).unwrap();
    assert!(vault.workspace().unwrap().warnings.is_empty());
    assert!(!Path::new(&first.summary.path).exists());
    assert!(moved_backend.is_file());
    assert!(vault
        .read(&second.summary.id)
        .unwrap()
        .related_ids
        .is_empty());
    assert!(vault.graph().unwrap().edges.is_empty());
    let trash = fs::read_dir(root.path().join(".aimemory/trash"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(trash
        .join("memories")
        .join(&frontend.id)
        .join(format!("{}.md", first.summary.id))
        .is_file());
    assert!(fs::read_to_string(trash.join("repository.json"))
        .unwrap()
        .contains("前端仓库"));
    drop(vault);
    let mut vault = Vault::open(root.path()).unwrap();
    vault.rebuild().unwrap();
    assert_eq!(vault.workspace().unwrap().repositories.len(), 1);
    assert_eq!(vault.search(None, "").unwrap().len(), 1);
    assert!(vault
        .read(&second.summary.id)
        .unwrap()
        .outgoing_related_ids
        .is_empty());
    assert!(vault.delete_repository("missing").is_err());
}

#[test]
fn update_log_tracks_real_changes_and_plain_markdown_summary_ignores_the_log() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repo = add_repository(&mut vault, "update-log");
    let first = vault
        .save(new_memory(&repo, "更新日志", "手工结论。\n"))
        .unwrap();
    assert!(first.markdown.contains("## 更新日志"));
    assert!(first.markdown.contains("创建记忆"));
    assert_eq!(first.summary.summary, "手工结论。");
    let unchanged = vault.save(edit_memory(first)).unwrap();
    assert_eq!(unchanged.markdown.matches("创建记忆").count(), 1);
    let mut edit = edit_memory(unchanged);
    edit.tags.push("新标签".into());
    let edited = vault.save(edit).unwrap();
    assert!(edited.markdown.contains("人工编辑 · 标签"));
    assert_eq!(edited.summary.summary, "手工结论。");
    let raw = fs::read_to_string(&edited.summary.path)
        .unwrap()
        .replace("手工结论。", "手工结论已修订。");
    fs::write(&edited.summary.path, raw).unwrap();
    vault.refresh().unwrap();
    let observed = vault.read(&edited.summary.id).unwrap();
    assert!(observed.markdown.contains("文件更新"));
    assert_eq!(observed.summary.summary, "手工结论已修订。");
    vault.refresh().unwrap();
    assert_eq!(
        vault.read(&edited.summary.id).unwrap().revision,
        observed.revision
    );
}

#[test]
fn external_conclusion_adoption_and_source_quotes_survive_reopen() {
    use crate::model_api::{ArchiveOp, ExtractedMemory};
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repo = add_repository(&mut vault, "revision-flow");
    let source = |id: &str| ArchiveSource {
        source_id: format!("source:{id}"),
        session_id: format!("session:{id}"),
        turn_id: id.into(),
        input_truncated: false,
        message_role: Some("user".into()),
    };
    let first = vault
        .save_archive(
            "ARC-revision-flow",
            new_memory(
                &repo,
                "导出进度",
                "## 结论\n\n采用 SSE。\n\n## 原文依据\n\n> 首轮确认 SSE。\n",
            ),
            &source("1"),
            &[],
        )
        .unwrap();
    assert_eq!(first.conclusion_owner, "auto");
    let raw = fs::read_to_string(&first.summary.path)
        .unwrap()
        .replace("采用 SSE。", "采用两秒轮询。");
    fs::write(&first.summary.path, raw).unwrap();
    vault.refresh().unwrap();
    let edited = vault.read(&first.summary.id).unwrap();
    assert_eq!(edited.conclusion_owner, "manual");
    assert_eq!(edited.summary.summary, "采用两秒轮询。");
    assert!(edited.markdown.contains("变更记录"));
    vault.refresh().unwrap();
    assert_eq!(
        vault.read(&first.summary.id).unwrap().revision,
        edited.revision
    );
    let change = ExtractedMemory {
        op: Some(ArchiveOp::Update),
        target_id: Some(first.summary.id.clone()),
        title: "导出进度".into(),
        kind: "技术决策".into(),
        summary: "终态停止两秒轮询。".into(),
        details: String::new(),
        evidence: "终态时停止。\n\n离开页面也停止。\n".into(),
        tags: Vec::new(),
        ..Default::default()
    };
    let next = vault
        .extend_archive(&first.summary.id, &repo.id, &change, &source("2"), None)
        .unwrap();
    assert_eq!(next.summary.summary, edited.summary.summary);
    assert_eq!(next.evidence_sources.len(), 2);
    assert_eq!(
        next.evidence_sources[1].quote.as_deref(),
        Some(change.evidence.as_str())
    );
    assert!(vault
        .adopt_suggestion(&first.summary.id, "source:2", &edited.revision)
        .unwrap_err()
        .contains("已更新"));
    let adopted = vault
        .adopt_suggestion(&first.summary.id, "source:2", &next.revision)
        .unwrap();
    assert_eq!(adopted.current_conclusion, change.summary);
    assert_eq!(adopted.conclusion_owner, "manual");
    assert!(adopted.suggestions.is_empty());
    drop(vault);
    let vault = Vault::open(root.path()).unwrap();
    let reopened = vault.read(&first.summary.id).unwrap();
    assert_eq!(reopened.current_conclusion, change.summary);
    assert_eq!(
        reopened.evidence_sources[1].quote.as_deref(),
        Some(change.evidence.as_str())
    );
}

#[test]
fn single_save_and_refresh_touch_only_changed_records_and_owned_edges() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repo = add_repository(&mut vault, "incremental");
    let a = vault.save(new_memory(&repo, "A", "## 结论\nA")).unwrap();
    let b = vault.save(new_memory(&repo, "B", "## 结论\nB")).unwrap();
    let mut c = new_memory(&repo, "C", "## 结论\nC");
    c.related_ids = vec![a.summary.id.clone()];
    let c = vault.save(c).unwrap();
    vault.db.execute_batch("CREATE TEMP TABLE writes(id TEXT); CREATE TEMP TRIGGER track_memory AFTER INSERT ON memories BEGIN INSERT INTO writes VALUES(new.id); END; CREATE TEMP TABLE edges(id TEXT); CREATE TEMP TRIGGER track_edge AFTER DELETE ON relations BEGIN INSERT INTO edges VALUES(old.from_id); END;").unwrap();
    let mut input = edit_memory(a);
    input.related_ids = vec![b.summary.id.clone()];
    let a = vault.save(input).unwrap();
    let writes: String = vault
        .db
        .query_row("SELECT group_concat(id) FROM writes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(writes, a.summary.id);
    assert!(vault
        .read(&a.summary.id)
        .unwrap()
        .related_ids
        .contains(&c.summary.id));
    vault
        .db
        .execute_batch("DELETE FROM writes; DELETE FROM edges;")
        .unwrap();
    vault.refresh().unwrap();
    let count: i64 = vault
        .db
        .query_row("SELECT count(*) FROM writes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let moved = Path::new(&a.summary.path).with_file_name("moved.md");
    fs::rename(&a.summary.path, &moved).unwrap();
    fs::remove_file(&b.summary.path).unwrap();
    vault.refresh().unwrap();
    assert_eq!(
        vault.read(&a.summary.id).unwrap().summary.path,
        moved.display().to_string()
    );
    assert!(vault.read(&b.summary.id).is_err());
    let graph = serde_json::to_value(vault.graph().unwrap()).unwrap();
    let rows = serde_json::to_value(vault.search(None, "").unwrap()).unwrap();
    vault.rebuild().unwrap();
    assert_eq!(serde_json::to_value(vault.graph().unwrap()).unwrap(), graph);
    assert_eq!(
        serde_json::to_value(vault.search(None, "").unwrap()).unwrap(),
        rows
    );
}

fn fixture() -> tempfile::TempDir {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&directory).unwrap();
    tempfile::tempdir_in(directory).unwrap()
}

fn add_repository(vault: &mut Vault, name: &str) -> Repository {
    vault
        .add_repository(RepositoryInput {
            name: name.into(),
            remote: format!("git@github.com:aimemory-demo/{name}.git"),
        })
        .unwrap()
}

fn new_memory(repository: &Repository, title: &str, markdown: &str) -> MemoryInput {
    MemoryInput {
        id: None,
        repository_id: repository.id.clone(),
        title: title.into(),
        kind: "技术决策".into(),
        tags: vec!["本地测试".into()],
        markdown: markdown.into(),
        related_ids: Vec::new(),
        revision: None,
        source: Some("手动记录".into()),
    }
}

fn edit_memory(memory: MemoryRecord) -> MemoryInput {
    MemoryInput {
        id: Some(memory.summary.id),
        repository_id: memory.summary.repository_id,
        title: memory.summary.title,
        kind: memory.summary.kind,
        tags: memory.summary.tags,
        markdown: memory.markdown,
        related_ids: memory.outgoing_related_ids,
        revision: Some(memory.revision),
        source: memory.source,
    }
}

#[test]
fn create_edit_reopen_and_search_real_markdown() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repository = add_repository(&mut vault, "frontend-web");
    let record = vault
        .save(new_memory(
            &repository,
            "任务进度改用短轮询",
            "## 结论\n每 2 秒查询任务状态。\n\n## 原文依据\n> 旧代理会缓冲 SSE。\n",
        ))
        .unwrap();
    let id = record.summary.id.clone();
    let path = PathBuf::from(&record.summary.path);
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("repository_remote: https://github.com/aimemory-demo/frontend-web"));
    assert!(raw.contains("每 2 秒查询任务状态。"));
    assert_eq!(
        vault
            .search(Some(&repository.id), "为什么改成轮询")
            .unwrap()[0]
            .id,
        id
    );
    let mut edited = edit_memory(record);
    edited.markdown = "## 结论\n每 3 秒查询任务状态，离开页面停止。\n".into();
    let updated = vault.save(edited).unwrap();
    assert_eq!(updated.summary.id, id);
    assert_eq!(updated.summary.path, path.display().to_string());
    drop(vault);
    let reopened = Vault::open(root.path()).unwrap();
    let restored = reopened.read(&id).unwrap();
    assert!(restored.markdown.contains("每 3 秒"));
    assert_eq!(restored.source.as_deref(), Some("手动记录"));
    assert_eq!(
        reopened.workspace().unwrap().repositories[0].memory_count,
        1
    );
    assert_eq!(
        reopened
            .search(Some(&repository.id), "完全不存在的词xyz")
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn imports_plain_markdown_and_preserves_custom_frontmatter_on_edit() {
    let root = fixture();
    fs::create_dir_all(root.path().join("existing")).unwrap();
    let path = root.path().join("existing/手写.md");
    fs::write(&path, "---\ncustom_owner: Alice\ncustom_flags:\n  important: true\n---\n# 旧记忆\n\n## 结论\n这是直接存入目录的正文。\n").unwrap();
    let mut vault = Vault::open(root.path()).unwrap();
    let found = vault.search(None, "手写").unwrap();
    assert!(found.is_empty());
    let memory = vault.search(None, "直接存入").unwrap().remove(0);
    let id = memory.id.clone();
    let mut input = edit_memory(vault.read(&id).unwrap());
    input.title = "更新后的旧记忆".into();
    vault.save(input).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("custom_owner: Alice"));
    assert!(raw.contains("important: true"));
    assert_eq!(vault.read(&id).unwrap().summary.title, "更新后的旧记忆");
    drop(vault);
    assert_eq!(
        Vault::open(root.path())
            .unwrap()
            .read(&id)
            .unwrap()
            .summary
            .id,
        id
    );
}

#[test]
fn refresh_reconciles_external_edits_and_removed_files() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repository = add_repository(&mut vault, "frontend-web");
    let record = vault
        .save(new_memory(&repository, "外部编辑", "## 结论\n旧内容\n"))
        .unwrap();
    let mut input = edit_memory(vault.read(&record.summary.id).unwrap());
    input.markdown = "## 结论\n应用中的待保存内容\n".into();
    let raw = fs::read_to_string(&record.summary.path)
        .unwrap()
        .replace("旧内容", "外部更新的正文");
    fs::write(&record.summary.path, &raw).unwrap();
    assert!(vault.save(input).unwrap_err().contains("其他编辑器"));
    assert_eq!(fs::read_to_string(&record.summary.path).unwrap(), raw);
    vault.refresh().unwrap();
    assert_eq!(
        vault
            .search(Some(&repository.id), "外部更新")
            .unwrap()
            .len(),
        1
    );
    fs::remove_file(&record.summary.path).unwrap();
    vault.refresh().unwrap();
    assert_eq!(vault.search(None, "").unwrap().len(), 0);
    assert_eq!(vault.workspace().unwrap().repositories[0].memory_count, 0);
}

#[test]
fn cross_repository_relations_are_bidirectional_and_persist() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let frontend = add_repository(&mut vault, "frontend-web");
    let backend = add_repository(&mut vault, "task-api");
    let target = vault
        .save(new_memory(
            &backend,
            "后端轮询约束",
            "## 结论\n终态必须停止查询。\n",
        ))
        .unwrap();
    let mut input = new_memory(&frontend, "前端轮询决定", "## 结论\n使用后端状态约定。\n");
    input.related_ids.push(target.summary.id.clone());
    let source = vault.save(input).unwrap();
    let target_record = vault.read(&target.summary.id).unwrap();
    assert_eq!(target_record.related_ids, vec![source.summary.id.clone()]);
    assert!(target_record.outgoing_related_ids.is_empty());
    assert_eq!(source.outgoing_related_ids, vec![target.summary.id.clone()]);
    assert!(vault
        .search(Some(&frontend.id), "轮询")
        .unwrap()
        .iter()
        .all(|m| m.repository_id == frontend.id));
    drop(vault);
    let reopened = Vault::open(root.path()).unwrap();
    assert_eq!(
        reopened.read(&source.summary.id).unwrap().related_ids,
        vec![target.summary.id]
    );
}

#[test]
fn graph_uses_indexed_summaries_and_existing_undirected_relations() {
    let root = fixture();
    for (id, repository, related) in [
        ("A", "frontend", "[B, C, A, MISSING]"),
        ("B", "backend", "[A]"),
        ("C", "frontend", "[B]"),
        ("D", "backend", "[]"),
    ] {
        fs::write(root.path().join(format!("{id}.md")), format!(
            "---\nid: {id}\nrepository_id: {repository}\nrelated_ids: {related}\ntitle: 记忆{id}\n---\n## 结论\n结论{id}\n\n## 详细说明\n仅在详情读取。\n"
        )).unwrap();
    }
    let mut vault = Vault::open(root.path()).unwrap();
    let graph = vault.graph().unwrap();
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B", "C", "D"]
    );
    assert_eq!(graph.nodes[0].repository_id, "frontend");
    assert_eq!(graph.nodes[1].repository_id, "backend");
    assert_eq!(graph.nodes[0].summary, "结论A");
    assert_eq!(
        graph.edges,
        vec![
            MemoryGraphEdge {
                source: "A".into(),
                target: "B".into()
            },
            MemoryGraphEdge {
                source: "A".into(),
                target: "C".into()
            },
            MemoryGraphEdge {
                source: "B".into(),
                target: "C".into()
            },
        ]
    );
    let serialized = serde_json::to_value(&graph).unwrap();
    assert!(serialized["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|node| node.get("markdown").is_none()));
    fs::remove_file(root.path().join("B.md")).unwrap();
    assert_eq!(vault.graph().unwrap().nodes.len(), 4);
    vault.refresh().unwrap();
    let refreshed = vault.graph().unwrap();
    assert_eq!(refreshed.nodes.len(), 3);
    assert_eq!(
        refreshed.edges,
        vec![MemoryGraphEdge {
            source: "A".into(),
            target: "C".into()
        }]
    );
}

#[test]
fn graph_empty_vault_has_no_nodes_or_edges() {
    let root = fixture();
    let vault = Vault::open(root.path()).unwrap();
    let graph = vault.graph().unwrap();
    assert!(graph.nodes.is_empty());
    assert!(graph.edges.is_empty());
}

#[test]
fn canonical_git_remote_deduplicates_repository() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let first = add_repository(&mut vault, "frontend-web");
    let second = vault
        .add_repository(RepositoryInput {
            name: "同一个仓库".into(),
            remote: "https://github.com/aimemory-demo/frontend-web.git/".into(),
        })
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(vault.workspace().unwrap().repositories.len(), 1);
}

#[test]
fn malformed_file_is_reported_and_other_memories_still_open() {
    let root = fixture();
    fs::write(root.path().join("bad.md"), "---\ntags: [broken\n---\n正文").unwrap();
    fs::write(
        root.path().join("good.md"),
        "# 有效记忆\n\n## 结论\n仍然可读。\n",
    )
    .unwrap();
    let vault = Vault::open(root.path()).unwrap();
    assert_eq!(vault.workspace().unwrap().warnings.len(), 1);
    assert_eq!(vault.search(None, "有效记忆").unwrap().len(), 1);
}

#[test]
fn archive_followups_preserve_edits_links_and_evidence_and_replay_once() {
    use crate::{
        mcp::{read_memory, ReadMemoryRequest, ReadMode},
        model_api::ExtractedMemory,
    };
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let repo = add_repository(&mut vault, "frontend-web");
    let other = add_repository(&mut vault, "another-repo");
    let related = vault.save(new_memory(&other, "关联", "外部约束")).unwrap();
    let source = |n| ArchiveSource {
        source_id: format!("codex:s:{n}"),
        session_id: "s".into(),
        turn_id: format!("t{n}"),
        input_truncated: n == 2,
        message_role: None,
    };
    let id = "ARC-update";
    let mut input = new_memory(&repo, "检查更新", "## 结论\n\n支持检查正式版本。\n\n## 详细说明\n\n版本列表来自发布仓库。\n\n## 原文依据\n\n> 检查更新。\n");
    input.related_ids.push(related.summary.id.clone());
    let first = vault.save_archive(id, input, &source(1), &[]).unwrap();
    let mut edit = edit_memory(first);
    edit.title = "应用更新（人工命名）".into();
    edit.markdown = edit
        .markdown
        .replace("支持检查正式版本。", "仅检查正式版本。");
    edit.markdown
        .push_str("\n## 人工验证\n\n启动时不访问网络。\n");
    let edited = vault.save(edit).unwrap();
    let mut change = ExtractedMemory {
        op: None,
        target_id: Some(id.into()),
        title: "新标题".into(),
        kind: "实现约束".into(),
        tags: vec!["本地测试".into(), "安装包".into()],
        summary: "正式版支持直接下载。".into(),
        details: "在设置中选择下载位置。".into(),
        evidence: "不能直接下载吗？".into(),
        ..Default::default()
    };
    assert!(vault
        .extend_archive(id, &other.id, &change, &source(2), None)
        .is_err());
    assert_eq!(
        vault
            .extend_archive(id, &repo.id, &change, &source(1), None)
            .unwrap()
            .revision,
        edited.revision
    );
    let second = vault
        .extend_archive(id, &repo.id, &change, &source(2), None)
        .unwrap();
    assert_eq!(second.summary.title, "应用更新（人工命名）");
    assert_eq!(second.summary.tags, ["本地测试", "安装包"]);
    assert_eq!(second.outgoing_related_ids, [related.summary.id.clone()]);
    assert_eq!(second.summary.summary, "仅检查正式版本。");
    assert_eq!(second.conclusion_owner, "manual");
    assert_eq!(second.suggestions[0].summary, change.summary);
    assert!(second.markdown.contains("仅检查正式版本。"));
    drop(vault);

    let mut vault = Vault::open(root.path()).unwrap();
    assert_eq!(
        vault
            .extend_archive(id, &repo.id, &change, &source(2), None)
            .unwrap()
            .revision,
        second.revision
    );
    change.summary = "正式版支持直接下载并核对大小。".into();
    change.details = "下载完成后核对大小。".into();
    change.evidence = "检查安装包字节数。".into();
    let third = vault
        .extend_archive(id, &repo.id, &change, &source(3), None)
        .unwrap();
    assert_eq!(third.summary.path, edited.summary.path);
    assert_eq!(third.summary.summary, "仅检查正式版本。");
    assert_eq!(third.suggestions.len(), 2);
    assert_eq!(third.suggestions[1].summary, change.summary);
    assert!(third.markdown.contains("正式版支持直接下载。"));
    for text in [
        "启动时不访问网络。",
        "在设置中选择下载位置。",
        "下载完成后核对大小。",
    ] {
        assert_eq!(third.markdown.matches(text).count(), 1);
    }
    let read = |mode| {
        read_memory(
            &vault,
            ReadMemoryRequest {
                source_id: None,
                id: id.into(),
                mode,
                section: None,
            },
        )
        .unwrap()
        .markdown
    };
    let conclusion = read(ReadMode::Section);
    assert!(conclusion.contains("仅检查正式版本。"));
    assert!(!conclusion.contains(&change.summary));
    assert!(!conclusion.contains("人工验证"));
    let evidence = read(ReadMode::Evidence);
    for quote in ["> 检查更新。", "> 不能直接下载吗？", "> 检查安装包字节数。"]
    {
        assert_eq!(evidence.matches(quote).count(), 1);
    }
    let document = parse_document(Path::new(&third.summary.path)).unwrap();
    assert_eq!(
        field(&document.meta, "codex_turn_id").as_deref(),
        Some("t1")
    );
    let followups: Vec<ArchiveSource> =
        serde_yaml_ng::from_value(document.meta["codex_followups"].clone()).unwrap();
    assert_eq!(followups.len(), 2);
    assert_eq!(followups[0].turn_id, "t2");
    assert!(followups[0].input_truncated);
    assert_eq!(vault.read(&related.summary.id).unwrap().related_ids, [id]);
}
