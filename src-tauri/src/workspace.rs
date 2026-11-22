use crate::{
    archive, pipeline::Pipeline, search::SearchEngine, storage, AppState, SavedWorkspace, Vault,
    Workspace,
};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub fn open(state: &AppState, path: Option<String>) -> Result<Workspace, String> {
    let root = match path {
        Some(path) => PathBuf::from(path),
        None => match fs::read_to_string(&state.settings_path) {
            Ok(json) => {
                serde_json::from_str::<SavedWorkspace>(&json)
                    .map_err(|error| format!("无法读取记忆库配置：{error}"))?
                    .root
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                state.default_root.clone()
            }
            Err(error) => return Err(format!("无法读取记忆库配置：{error}")),
        },
    };
    let vault = Vault::open(&root)?;
    let pipeline = Pipeline::open(&vault.root)?;
    if state
        .recovered_roots
        .lock()
        .map_err(|error| error.to_string())?
        .insert(vault.root.clone())
    {
        pipeline.recover_interrupted()?;
    }
    let config = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .config()
        .clone();
    pipeline.sync_embeddings(
        &vault,
        &archive::embedding_key(&config.embedding),
        archive::embedding_ready(&config.embedding),
    )?;
    let search = Arc::new(SearchEngine::open(&vault.root)?);
    let workspace = vault.workspace()?;
    let saved = SavedWorkspace {
        root: vault.root.clone(),
    };
    storage::write_atomic(
        &state.settings_path,
        &serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?,
    )?;
    let mut current = state.vault.lock().map_err(|error| error.to_string())?;
    let mut current_pipeline = state.pipeline.lock().map_err(|error| error.to_string())?;
    *current = Some(vault);
    *current_pipeline = Some(pipeline);
    *state.search.lock().map_err(|error| error.to_string())? = Some(search);
    Ok(workspace)
}
pub fn migrate(state: &AppState, destination: &Path) -> Result<Workspace, String> {
    let mut current = state.vault.lock().map_err(|error| error.to_string())?;
    let source = current.as_ref().ok_or("请先打开记忆库")?;
    let destination = validate_destination(&source.root, destination)?;
    if destination == source.root {
        return source.workspace();
    }
    let _transition = state
        .workspace_transition
        .try_write()
        .map_err(|_| "当前归档、向量或检索正在处理，请完成后重试迁移")?;
    let mut current_pipeline = state.pipeline.lock().map_err(|error| error.to_string())?;
    if current_pipeline
        .as_ref()
        .ok_or("请先打开记忆库")?
        .has_in_flight_requests()?
    {
        return Err("当前归档或向量请求正在处理，请完成后重试迁移".into());
    }
    let mut current_search = state.search.lock().map_err(|error| error.to_string())?;
    let mut recovered = state
        .recovered_roots
        .lock()
        .map_err(|error| error.to_string())?;

    copy_workspace(&source.root, &destination)?;
    let prepared = (|| {
        let vault = Vault::open(&destination)?;
        let pipeline = Pipeline::open(&destination)?;
        let search = Arc::new(SearchEngine::open(&destination)?);
        let workspace = vault.workspace()?;
        let saved = serde_json::to_vec_pretty(&SavedWorkspace {
            root: destination.clone(),
        })
        .map_err(|error| error.to_string())?;
        storage::write_atomic(&state.settings_path, &saved)?;
        Ok::<_, String>((vault, pipeline, search, workspace))
    })();
    let (vault, pipeline, search, workspace) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            fs::remove_dir_all(&destination)
                .and_then(|_| fs::create_dir(&destination))
                .map_err(|cleanup| {
                    format!("{error}；未切换记忆库，清理本次目标副本失败：{cleanup}")
                })?;
            return Err(error);
        }
    };

    recovered.insert(destination);
    *current = Some(vault);
    *current_pipeline = Some(pipeline);
    *current_search = Some(search);
    Ok(workspace)
}

fn validate_destination(source: &Path, destination: &Path) -> Result<PathBuf, String> {
    let destination = destination
        .canonicalize()
        .map_err(|error| format!("无法打开目标目录，请选择已创建的空目录：{error}"))?;
    if destination == source {
        return Ok(destination);
    }
    if destination.starts_with(source) {
        return Err("目标目录不能位于当前记忆库内部".into());
    }
    if fs::read_dir(&destination)
        .map_err(|error| error.to_string())?
        .next()
        .transpose()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("目标目录不是空目录，请选择空目录；已有记忆库请使用打开或导入".into());
    }
    Ok(destination)
}

fn copy_workspace(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination.parent().ok_or("目标目录无效")?;
    let staging = tempfile::Builder::new()
        .prefix(".aimemory-migrate-")
        .tempdir_in(parent)
        .map_err(|error| format!("无法创建迁移目录：{error}"))?;
    copy_directory(source, staging.path(), source)?;
    {
        let _vault = Vault::open(staging.path())?;
        let _pipeline = Pipeline::open(staging.path())?;
        let _search = SearchEngine::open(staging.path())?;
    }
    fs::remove_dir(destination).map_err(|error| format!("无法替换空目标目录：{error}"))?;
    if let Err(error) = fs::rename(staging.path(), destination) {
        let _ = fs::create_dir(destination);
        return Err(format!("无法完成记忆库迁移：{error}"));
    }
    Ok(())
}

fn copy_directory(directory: &Path, destination: &Path, root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source = entry.path();
        let relative = source
            .strip_prefix(root)
            .map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if relative == Path::new(".aimemory/index.sqlite-wal")
            || relative == Path::new(".aimemory/index.sqlite-shm")
            || relative == Path::new(".aimemory/pipeline.sqlite-wal")
            || relative == Path::new(".aimemory/pipeline.sqlite-shm")
        {
            continue;
        }
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            fs::create_dir(&target).map_err(|error| error.to_string())?;
            copy_directory(&source, &target, root)?;
        } else if relative == Path::new(".aimemory/index.sqlite")
            || relative == Path::new(".aimemory/pipeline.sqlite")
        {
            // SQLite snapshots include committed WAL content; copying only the main file would lose it.
            let db = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| error.to_string())?;
            db.busy_timeout(Duration::from_secs(5))
                .map_err(|error| error.to_string())?;
            db.execute("VACUUM main INTO ?1", [target.to_string_lossy().as_ref()])
                .map_err(|error| format!("无法复制数据库 {}：{error}", source.display()))?;
        } else {
            fs::copy(&source, &target)
                .map_err(|error| format!("无法复制 {}：{error}", source.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
