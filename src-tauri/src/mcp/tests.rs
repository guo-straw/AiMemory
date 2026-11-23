use super::*;
use crate::storage::{MemoryInput, RepositoryInput};
use std::process::Command;

fn fixture() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn memory(vault: &mut Vault, markdown: &str) -> String {
    let repository = vault
        .add_repository(RepositoryInput {
            name: "frontend-web".into(),
            remote: "git@github.com:aimemory-test/frontend-web.git".into(),
        })
        .unwrap();
    vault
        .save(MemoryInput {
            id: None,
            repository_id: repository.id,
            title: "任务进度改用轮询".into(),
            kind: "技术决策".into(),
            tags: vec!["测试".into()],
            markdown: markdown.into(),
            related_ids: Vec::new(),
            revision: None,
            source: Some("Codex · session-1 · turn-1".into()),
        })
        .unwrap()
        .summary
        .id
}

#[test]
fn reads_sections_evidence_and_full_without_changing_markdown() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let body = "# 任务进度改用轮询\n\n## 结论\n\n每两秒查询一次。\n\n## 详细说明\n\n旧代理会缓冲 SSE。\n\n### 清理\n\n离开页面时停止。\n\n## 原文依据\n\n> 终态后停止查询。\n\n来源轮次 1\n";
    let id = memory(&mut vault, body);
    let sections = list_sections(&vault, &id).unwrap();
    assert_eq!(sections.sections.len(), 6);
    let saved = vault.read(&id).unwrap().markdown;
    let conclusion = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: id.clone(),
            mode: ReadMode::Section,
            section: None,
        },
    )
    .unwrap();
    assert_eq!(conclusion.markdown, "## 结论\n\n每两秒查询一次。\n\n");
    assert!(!conclusion.markdown.contains("旧代理"));
    let detail = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: id.clone(),
            mode: ReadMode::Section,
            section: Some("详细说明".into()),
        },
    )
    .unwrap();
    assert!(detail.markdown.contains("### 清理"));
    assert!(!detail.markdown.contains("## 原文依据"));
    let evidence = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: id.clone(),
            mode: ReadMode::Evidence,
            section: None,
        },
    )
    .unwrap();
    assert_eq!(
        evidence.markdown.trim_end(),
        "## 原文依据\n\n> 终态后停止查询。\n\n来源轮次 1"
    );
    assert_eq!(
        evidence.source.as_deref(),
        Some("Codex · session-1 · turn-1")
    );
    let full = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id,
            mode: ReadMode::Full,
            section: None,
        },
    )
    .unwrap();
    assert_eq!(full.markdown, saved);
    assert!(full.markdown.starts_with(body));
}

#[test]
fn real_markdown_headings_exclude_code_blocks_and_include_setext_and_inline_text() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let body = "结论\n====\n\n决定。\n\n```md\n## 原文依据\n这行是代码。\n```\n\n## **详细** `说明`\n\n内容。\n\n原文依据\n--------\n\n> 真正依据。\n";
    let id = memory(&mut vault, body);
    let sections = list_sections(&vault, &id).unwrap();
    assert_eq!(
        sections
            .sections
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        vec!["结论", "详细 说明", "原文依据", "更新日志"]
    );
    let evidence = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id,
            mode: ReadMode::Evidence,
            section: None,
        },
    )
    .unwrap();
    assert_eq!(
        evidence.markdown.trim_end(),
        "原文依据\n--------\n\n> 真正依据。"
    );
}

#[test]
fn duplicate_heading_requires_section_id_and_unknown_section_is_an_error() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let id = memory(&mut vault, "## 结论\n\n第一条。\n\n## 结论\n\n第二条。\n");
    let error = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: id.clone(),
            mode: ReadMode::Section,
            section: None,
        },
    )
    .unwrap_err();
    assert!(error.contains("多个同名"));
    let second = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id: id.clone(),
            mode: ReadMode::Section,
            section: Some("section-2".into()),
        },
    )
    .unwrap();
    assert_eq!(second.markdown.trim_end(), "## 结论\n\n第二条。");
    assert!(read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id,
            mode: ReadMode::Evidence,
            section: None
        }
    )
    .unwrap_err()
    .contains("未找到章节"));
    assert!(list_sections(&vault, "not-found").is_err());
}

#[test]
fn headings_inside_evidence_quotes_and_lists_preserve_the_document_section() {
    let root = fixture();
    let mut vault = Vault::open(root.path()).unwrap();
    let body = "## 结论\n\n保留逐字依据。\n\n## 原文依据\n\n> ## copied heading\n> 原文仍在同一个引用中。\n>\n> ### nested heading\n> 更多依据。\n\n- ### list heading\n\n  列表正文仍属于依据。\n\n## 后续说明\n\n另一个文档章节。\n";
    let id = memory(&mut vault, body);
    let sections = list_sections(&vault, &id).unwrap();
    assert_eq!(
        sections
            .sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>(),
        vec!["结论", "原文依据", "后续说明", "更新日志"]
    );
    let evidence = read_memory(
        &vault,
        ReadMemoryRequest {
            source_id: None,
            id,
            mode: ReadMode::Evidence,
            section: None,
        },
    )
    .unwrap();
    assert_eq!(evidence.markdown, "## 原文依据\n\n> ## copied heading\n> 原文仍在同一个引用中。\n>\n> ### nested heading\n> 更多依据。\n\n- ### list heading\n\n  列表正文仍属于依据。\n\n");
}

#[test]
fn current_git_repository_is_resolved_without_broadening_to_another_repository() {
    let root = fixture();
    assert!(Command::new("git")
        .arg("init")
        .arg(root.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(root.path())
        .args([
            "remote",
            "add",
            "origin",
            "git@github.com:aimemory-test/frontend-web.git"
        ])
        .output()
        .unwrap()
        .status
        .success());
    let repositories = vec![
        Repository {
            id: "frontend-id".into(),
            name: "frontend-web".into(),
            remote: "https://github.com/aimemory-test/frontend-web".into(),
            memory_count: 5,
        },
        Repository {
            id: "backend-id".into(),
            name: "task-api".into(),
            remote: "https://github.com/aimemory-test/task-api".into(),
            memory_count: 5,
        },
    ];
    assert_eq!(
        resolve_repository(&repositories, None, root.path())
            .unwrap()
            .id,
        "frontend-id"
    );
    assert_eq!(
        resolve_repository(&repositories, Some("task-api"), root.path())
            .unwrap()
            .id,
        "backend-id"
    );
    assert!(resolve_repository(&repositories, Some("missing"), root.path()).is_err());
    let empty = root.path().join("without-origin");
    fs::create_dir(&empty).unwrap();
    assert_eq!(
        resolve_repository(&repositories, None, &empty).unwrap().id,
        "frontend-id"
    );
    assert!(Command::new("git")
        .arg("init")
        .arg(&empty)
        .output()
        .unwrap()
        .status
        .success());
    assert!(resolve_repository(&repositories, None, &empty)
        .unwrap_err()
        .contains("没有 Git origin"));
}
