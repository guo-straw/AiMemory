use crate::storage::{normalize_remote, repository_id};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRepository {
    pub id: String,
    pub name: String,
    pub remote: String,
    pub root: String,
    pub files: Vec<String>,
}

pub(crate) fn collect_tool_paths(input: &str, cwd: &str, paths: &mut BTreeSet<PathBuf>) {
    if let Ok(arguments) = serde_json::from_str::<serde_json::Value>(input) {
        let cwd = arguments
            .get("workdir")
            .or_else(|| arguments.get("cwd"))
            .and_then(|value| value.as_str())
            .unwrap_or(cwd);
        collect_paths(input, cwd, paths);
        return;
    }
    let mut calls = input.split("tools.exec_command(");
    collect_paths(calls.next().unwrap_or_default(), cwd, paths);
    for call in calls {
        let workdir = call.split_once("workdir").and_then(|(_, rest)| {
            let (_, value) = rest.split_once(':')?;
            let value = value.trim_start();
            let delimiter = value.chars().next()?;
            if !matches!(delimiter, '\"' | '\'') {
                return None;
            }
            value[1..].split_once(delimiter).map(|(value, _)| value)
        });
        collect_paths(call, workdir.unwrap_or(cwd), paths);
    }
}

pub(crate) fn collect_paths(text: &str, cwd: &str, paths: &mut BTreeSet<PathBuf>) {
    let text = text.replace("\\n", "\n").replace("\\\"", "\"");
    let mut candidates = Vec::new();
    for line in text.lines() {
        for marker in [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some((_, path)) = line.split_once(marker) {
                candidates.push((path.trim(), true));
            }
        }
    }
    for (index, _) in text.match_indices("](") {
        if let Some((path, _)) = text[index + 2..].split_once(')') {
            candidates.push((path.trim_matches(['<', '>']), true));
        }
    }
    let mut quote = None;
    for (index, character) in text.char_indices() {
        if let Some((delimiter, start)) = quote {
            if character == delimiter {
                candidates.push((&text[start..index], false));
                quote = None;
            }
        } else if matches!(character, '\'' | '"' | '`') {
            quote = Some((character, index + 1));
        }
    }
    candidates.extend(
        text.split(|c: char| c.is_whitespace() || "\"'`()[]{}<>,;=".contains(c))
            .map(|path| (path, false)),
    );
    for (candidate, explicit) in candidates {
        let mut candidate = candidate.trim_end_matches(['.', ',', '。', '，']);
        for _ in 0..2 {
            if let Some((path, line)) = candidate.rsplit_once(':') {
                if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
                    candidate = path;
                }
            }
        }
        if candidate.is_empty()
            || candidate.contains("://")
            || candidate.contains(['\n', '*', '?', '$', '\0'])
        {
            continue;
        }
        let path = Path::new(candidate);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(cwd).join(path)
        };
        let file = explicit
            || path.is_file()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    !extension.is_empty()
                        && extension.len() <= 12
                        && extension.chars().all(|c| c.is_ascii_alphanumeric())
                });
        if !file && !(path.is_absolute() && path.is_dir()) {
            continue;
        }
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => {}
                component => normalized.push(component),
            }
        }
        paths.insert(normalized);
    }
}

pub(crate) fn resolve_repositories(paths: &BTreeSet<PathBuf>) -> Vec<CodeRepository> {
    let mut repositories = BTreeMap::<String, CodeRepository>::new();
    let mut roots = BTreeMap::<PathBuf, Option<(String, String)>>::new();
    for path in paths {
        let directory = if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(path)
        };
        let Some(root) = directory
            .ancestors()
            .find(|path| path.join(".git").exists())
        else {
            continue;
        };
        let identity = roots.entry(root.to_path_buf()).or_insert_with(|| {
            let remote = crate::codex::git_remote(&root.display().to_string())?;
            let remote = normalize_remote(&remote).ok()?;
            Some((repository_id(&remote), remote))
        });
        let Some((id, remote)) = identity.as_ref() else {
            continue;
        };
        let repository = repositories
            .entry(id.clone())
            .or_insert_with(|| CodeRepository {
                id: id.clone(),
                name: remote.rsplit('/').next().unwrap_or_default().to_string(),
                remote: remote.clone(),
                root: root.display().to_string(),
                files: Vec::new(),
            });
        if !path.is_dir() {
            if let Ok(file) = path.strip_prefix(root) {
                let file = file.to_string_lossy().replace('\\', "/");
                if !repository.files.contains(&file) {
                    repository.files.push(file);
                }
            }
        }
    }
    repositories.into_values().collect()
}
