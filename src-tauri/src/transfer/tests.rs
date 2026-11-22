use super::*;
use crate::{
    config::{ServiceConfig, SettingsStore},
    storage::{MemoryInput, RepositoryInput},
    theme,
};
use serde_json::{json, Value};

fn fixture() -> tempfile::TempDir {
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

fn config() -> AppConfig {
    AppConfig {
        chat: ServiceConfig {
            provider: None,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1-mini".into(),
            has_api_key: true,
        },
        embedding: ServiceConfig {
            provider: None,
            base_url: "https://api.openai.com/v1".into(),
            model: "text-embedding-3-small".into(),
            has_api_key: true,
        },
        codex_directory: "/private/codex-path".into(),
        archive_enabled: false,
        archive_started_at: Some("2026-11-20T00:00:00Z".into()),
        run_in_background: true,
    }
}

fn package(memories: Value, links: Value) -> Value {
    json!({"type":"aimemory.memories","version":1,"memories":memories,"links":links})
}

fn preview(
    state: &mut TransferState,
    path: &Path,
    vault: &Vault,
    kind: TransferKind,
) -> ImportPreview {
    state
        .preview(kind, path, vault, &config(), &ThemeConfig::default())
        .unwrap()
}

fn apply_memory(
    state: &mut TransferState,
    preview: &ImportPreview,
    vault: &mut Vault,
) -> crate::storage::transfer::MemoryImportResult {
    let PreparedImport::Memories(plan) = state.take(&preview.token, &vault.root).unwrap() else {
        panic!("wrong import kind")
    };
    plan.apply(vault).unwrap()
}

#[test]
fn memory_roundtrip_preserves_complete_raw_metadata_and_cross_repository_relations() {
    let source_root = fixture();
    let mut source = Vault::open(source_root.path()).unwrap();
    let frontend = source
        .add_repository(RepositoryInput {
            name: "前端展示名".into(),
            remote: "git@github.com:demo/frontend.git".into(),
        })
        .unwrap();
    let backend = source
        .add_repository(RepositoryInput {
            name: "后端".into(),
            remote: "https://github.com/demo/backend.git".into(),
        })
        .unwrap();
    let second = source
        .save(MemoryInput {
            id: None,
            repository_id: backend.id.clone(),
            title: "后端约束".into(),
            kind: "实现约束".into(),
            tags: vec![],
            markdown: "## 结论\n终态停止。\n".into(),
            related_ids: vec![],
            revision: None,
            source: Some("来源会话 12".into()),
        })
        .unwrap();
    let first = source
        .save(MemoryInput {
            id: None,
            repository_id: frontend.id.clone(),
            title: "前端轮询".into(),
            kind: "技术决策".into(),
            tags: vec!["polling".into()],
            markdown: "## 结论\n每两秒查询。\n\n## 原文依据\n> 到终态就停。\n".into(),
            related_ids: vec![second.summary.id.clone()],
            revision: None,
            source: Some("来源会话 18".into()),
        })
        .unwrap();
    let original = fs::read_to_string(&first.summary.path).unwrap().replacen(
        "---\n",
        "---\ncustom_owner: Alice\ncustom_flags:\n  important: true\n",
        1,
    );
    fs::write(&first.summary.path, &original).unwrap();
    source.refresh().unwrap();
    let original = fs::read_to_string(&first.summary.path).unwrap();
    let path = source_root.path().join("export.json");
    export_memories(&path, &source).unwrap();
    let target_root = fixture();
    let mut target = Vault::open(target_root.path()).unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &target, TransferKind::Memories);
    assert_eq!((review.added_memories, review.new_links), (2, 1));
    assert!(target.graph().unwrap().nodes.is_empty());
    let result = apply_memory(&mut state, &review, &mut target);
    assert_eq!((result.added_memories, result.new_links), (2, 1));
    let imported = target.read(&first.summary.id).unwrap();
    assert_eq!(
        fs::read_to_string(&imported.summary.path).unwrap(),
        original
    );
    assert_eq!(imported.source, first.source);
    assert_eq!(
        target
            .workspace()
            .unwrap()
            .repositories
            .iter()
            .find(|repository| repository.id == frontend.id)
            .unwrap()
            .name,
        "前端展示名"
    );
    assert_eq!(
        target.read(&second.summary.id).unwrap().related_ids,
        vec![first.summary.id.clone()]
    );
    let review = preview(&mut state, &path, &target, TransferKind::Memories);
    assert_eq!(
        (
            review.added_memories,
            review.existing_memories,
            review.new_links
        ),
        (0, 2, 0)
    );
    let result = apply_memory(&mut state, &review, &mut target);
    assert_eq!(result.added_memories, 0);
    assert_eq!(
        fs::read_to_string(&imported.summary.path).unwrap(),
        original
    );
    drop(target);
    assert_eq!(
        Vault::open(target_root.path())
            .unwrap()
            .graph()
            .unwrap()
            .edges
            .len(),
        1
    );
}

#[test]
fn plain_local_markdown_roundtrip_keeps_identity_body_and_repository_name() {
    let source_root = fixture();
    fs::create_dir_all(source_root.path().join("我的笔记")).unwrap();
    let body = "# 手写记录\n\n## 结论\n保留逐字正文。\n";
    fs::write(source_root.path().join("我的笔记/原文.md"), body).unwrap();
    let source = Vault::open(source_root.path()).unwrap();
    let id = source.graph().unwrap().nodes[0].id.clone();
    let path = source_root.path().join("local.json");
    export_memories(&path, &source).unwrap();
    let target_root = fixture();
    let mut target = Vault::open(target_root.path()).unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &target, TransferKind::Memories);
    apply_memory(&mut state, &review, &mut target);
    assert_eq!(target.read(&id).unwrap().markdown, body);
    assert_eq!(target.workspace().unwrap().repositories[0].name, "我的笔记");
    assert!(target.workspace().unwrap().repositories[0]
        .remote
        .is_empty());
    drop(target);
    assert_eq!(
        Vault::open(target_root.path())
            .unwrap()
            .read(&id)
            .unwrap()
            .markdown,
        body
    );
}

#[test]
fn duplicate_id_preserves_local_body_and_repository_but_merges_valid_relations() {
    let root = fixture();
    let local = root.path().join("local.md");
    fs::write(&local, "---\nid: A\nrepository_id: local\nrepository_name: 本机仓库\ncustom_owner: Alice\n---\n## 结论\n用户已经更正的原文。\n").unwrap();
    let mut vault = Vault::open(root.path()).unwrap();
    let before = fs::read_to_string(&local).unwrap();
    let path = root.path().join("import.json");
    fs::write(&path, package(json!([
        {"id":"A","repository":"https://github.com/demo/other.git","markdown":"# 不应覆盖的传入正文\n"},
        {"id":"B","repository":"https://github.com/demo/other.git","markdown":"# 新记忆\n\n## 结论\n本地可读。\n"}
    ]), json!([{"from":"A","to":"B"},{"from":"B","to":"A"},{"from":"A","to":"A"},{"from":"B","to":"MISSING"}])).to_string()).unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &vault, TransferKind::Memories);
    assert_eq!(
        (
            review.added_memories,
            review.existing_memories,
            review.new_links,
            review.invalid_links
        ),
        (1, 1, 1, 2)
    );
    assert_eq!(
        review.memories[0].local_repository.as_deref(),
        Some("本机仓库")
    );
    assert_eq!(fs::read_to_string(&local).unwrap(), before);
    let result = apply_memory(&mut state, &review, &mut vault);
    assert_eq!(
        (
            result.added_memories,
            result.existing_memories,
            result.new_links
        ),
        (1, 1, 1)
    );
    let retained = vault.read("A").unwrap();
    assert_eq!(retained.summary.repository_id, "local");
    assert_eq!(retained.current_conclusion, "用户已经更正的原文。");
    assert!(retained
        .markdown
        .starts_with("## 结论\n用户已经更正的原文。\n"));
    assert!(retained.markdown.contains("## 更新日志"));
    assert_eq!(retained.related_ids, ["B"]);
    assert_eq!(vault.read("B").unwrap().related_ids, ["A"]);
    assert!(fs::read_to_string(&local)
        .unwrap()
        .contains("custom_owner: Alice"));
    assert_eq!(vault.graph().unwrap().edges.len(), 1);
}

#[test]
fn preview_uses_validated_snapshot_and_rejects_invalid_packets_without_writing() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let path = root.path().join("import.json");
    let packet = package(
        json!([{"id":"A","repository":"https://github.com/demo/repo.git","markdown":"# 预览确认的原文\n"}]),
        json!([]),
    );
    fs::write(&path, packet.to_string()).unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &vault, TransferKind::Memories);
    fs::write(&path, "not json").unwrap();
    apply_memory(&mut state, &review, &mut vault);
    assert!(vault.read("A").unwrap().markdown.contains("预览确认的原文"));
    for update in [
        json!({"type":"other"}),
        json!({"version":2}),
        json!({"memories":[{"id":"../escape","repository":"https://github.com/demo/repo.git","markdown":"# path"}]}),
    ] {
        let mut bad = packet.clone();
        for (key, value) in update.as_object().unwrap() {
            bad[key] = value.clone();
        }
        fs::write(&path, bad.to_string()).unwrap();
        assert!(state
            .preview(
                TransferKind::Memories,
                &path,
                &vault,
                &config(),
                &ThemeConfig::default()
            )
            .is_err());
    }
    assert_eq!(vault.graph().unwrap().nodes.len(), 1);
    assert!(!root.path().parent().unwrap().join("escape.md").exists());
    state.discard(&review.token);
    state.discard(&review.token);
    assert!(state.take(&review.token, &vault.root).is_err());
}

#[test]
fn frontmatter_repository_id_is_validated_before_any_import_write() {
    let root = fixture();
    let vault = Vault::open(root.path()).unwrap();
    let path = root.path().join("invalid-frontmatter.json");
    for invalid_id in ["../outside", "a/b", "a\\b", ".", "..", ""] {
        fs::write(&path, package(json!([{
            "id":"A", "repository":"https://github.com/demo/repo.git",
            "markdown":format!("---\nid: A\nrepository_id: {}\nrepository_remote: https://github.com/demo/repo.git\n---\n# 原文\n", serde_json::to_string(invalid_id).unwrap())
        }]), json!([])).to_string()).unwrap();
        let mut state = TransferState::default();
        assert!(state
            .preview(
                TransferKind::Memories,
                &path,
                &vault,
                &config(),
                &ThemeConfig::default()
            )
            .is_err());
        assert!(vault.graph().unwrap().nodes.is_empty());
        assert!(!root.path().join("imported").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}

#[test]
fn config_export_omits_device_secrets_and_old_import_preserves_theme() {
    let root = fixture();
    let vault = Vault::open(root.path()).unwrap();
    let path = root.path().join("config.json");
    export_config(&path, &config(), &ThemeConfig::default()).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    for excluded in [
        "hasApiKey",
        "apiKey",
        "codexDirectory",
        "archiveStartedAt",
        "private/codex-path",
        "2026-11-20T",
    ] {
        assert!(!raw.contains(excluded));
    }
    let mut packet: serde_json::Value = serde_json::from_str(&raw).unwrap();
    packet["config"].as_object_mut().unwrap().remove("theme");
    packet["config"]["chat"]["model"] = json!("gpt-6.1-sol");
    fs::write(&path, packet.to_string()).unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &vault, TransferKind::Config);
    assert!(!review.has_theme);
    assert!(review.theme.is_none());
    assert!(review
        .rows
        .iter()
        .any(|row| row.label == "聊天模型" && row.changed));
    let PreparedImport::Config(incoming) = state.take(&review.token, &vault.root).unwrap() else {
        panic!("config expected")
    };
    assert!(incoming.theme.is_none());
    packet["config"]["chat"]["apiKey"] = json!("should-not-import");
    fs::write(&path, packet.to_string()).unwrap();
    assert!(state
        .preview(
            TransferKind::Config,
            &path,
            &vault,
            &config(),
            &ThemeConfig::default()
        )
        .is_err());
}

#[test]
fn theme_files_validate_before_apply_and_config_snapshot_preserves_local_key_flags() {
    let root = fixture();
    let path = root.path().join("theme.json");
    let mut dark = ThemeConfig::default();
    dark.preset = "夜色".into();
    dark.background = "#202227".into();
    dark.foreground = "#F2F3F5".into();
    export_theme(&path, &dark).unwrap();
    assert_eq!(read_theme(&path).unwrap(), dark);
    assert!(!root.path().join("appearance.json").exists());
    let mut bad: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    bad["theme"]["background"] = json!("url(secret)");
    fs::write(&path, bad.to_string()).unwrap();
    assert!(read_theme(&path).is_err());
    assert_eq!(theme::load(root.path()).unwrap(), ThemeConfig::default());
    let mut settings = SettingsStore::load(root.path(), root.path().join("codex")).unwrap();
    let local = config();
    settings.apply_import(local.clone()).unwrap();
    assert!(settings.view().unwrap().chat.has_api_key);
    assert!(settings.view().unwrap().embedding.has_api_key);
    assert_eq!(
        settings.view().unwrap().archive_started_at,
        local.archive_started_at
    );
    assert_eq!(
        settings.view().unwrap().codex_directory,
        local.codex_directory
    );
    let json = fs::read_to_string(root.path().join("services.json")).unwrap();
    assert!(!json.contains("should-not-import"));
}

#[test]
fn failed_batch_can_resume_without_overwriting_completed_memories() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let remote_a = "https://github.com/demo/a";
    let remote_b = "https://github.com/demo/b";
    let blocked_repo = vault
        .add_repository(RepositoryInput {
            name: "b".into(),
            remote: remote_b.into(),
        })
        .unwrap()
        .id;
    let blocked = root.path().join(format!(
        "imported-{}",
        &blake3::hash(blocked_repo.as_bytes()).to_hex()[..12]
    ));
    fs::write(&blocked, "existing blocking file").unwrap();
    let path = root.path().join("batch.json");
    fs::write(
        &path,
        package(
            json!([
                {"id":"A","repository":remote_a,"markdown":"# 第一条\n"},
                {"id":"B","repository":remote_b,"markdown":"# 第二条\n"}
            ]),
            json!([]),
        )
        .to_string(),
    )
    .unwrap();
    let mut state = TransferState::default();
    let review = preview(&mut state, &path, &vault, TransferKind::Memories);
    let PreparedImport::Memories(plan) = state.take(&review.token, &vault.root).unwrap() else {
        panic!("memories expected")
    };
    assert!(plan.apply(&mut vault).is_err());
    let first = vault.read("A").unwrap();
    let raw = fs::read_to_string(&first.summary.path).unwrap();
    assert!(vault.read("B").is_err());
    fs::remove_file(blocked).unwrap();
    let review = preview(&mut state, &path, &vault, TransferKind::Memories);
    assert_eq!((review.added_memories, review.existing_memories), (1, 1));
    apply_memory(&mut state, &review, &mut vault);
    assert_eq!(fs::read_to_string(first.summary.path).unwrap(), raw);
    assert_eq!(vault.graph().unwrap().nodes.len(), 2);
}
