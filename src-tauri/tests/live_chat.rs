use knowtrace_lib::{
    codex::{self, CodexTurn},
    config::{ServiceCredentials, ServiceKind, SettingsStore},
    model_api,
};
use serde_json::{json, Value};
use std::{env, fs, path::PathBuf};

fn credentials() -> ServiceCredentials {
    assert_eq!(env::var("AIMEMORY_LIVE_CHAT").as_deref(), Ok("1"));
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data");
    let settings = SettingsStore::load(&directory, codex::default_codex_directory()).unwrap();
    let kind = match env::var("AIMEMORY_LIVE_CREDENTIAL_SERVICE").as_deref() {
        Ok("embedding") => ServiceKind::Embedding,
        _ => ServiceKind::Chat,
    };
    let mut service = settings.credentials(kind).unwrap();
    assert_eq!(service.base_url, "https://api.openai.com/v1");
    assert!(!service.api_key.is_empty(), "请先在应用中保存 API 凭据");
    service.model = env::var("AIMEMORY_LIVE_CHAT_MODEL").unwrap_or("gpt-6.1-sol".into());
    service
}

fn record(name: &str, value: Value) {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/chat-api-acceptance");
    fs::create_dir_all(&directory).unwrap();
    let phase = env::var("AIMEMORY_LIVE_PHASE").unwrap_or("current".into());
    fs::write(
        directory.join(format!("{name}-{phase}.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    println!("{value}");
}

#[tokio::test]
#[ignore = "显式启用后使用系统凭据库调用真实聊天服务"]
async fn real_chat_connection() {
    let service = credentials();
    let result = model_api::test_connection(&service, ServiceKind::Chat).await;
    let report = match &result {
        Ok(connection) => {
            json!({"model": service.model, "success": true, "connection": connection})
        }
        Err(error) => json!({"model": service.model, "success": false, "error": error}),
    };
    record("connection", report);
    assert!(result.is_ok(), "连接测试失败，请查看脱敏诊断记录");
}

#[tokio::test]
#[ignore = "显式启用后向真实聊天服务发送示例轮次验证归档兼容性"]
async fn real_chat_extraction() {
    let service = credentials();
    let turn = CodexTurn {
        source_id: "acceptance:chat-400".into(),
        session_id: "acceptance".into(),
        turn_id: "chat-400".into(),
        completed_at: "2026-11-21T00:00:00Z".into(),
        cwd: "/example/frontend-web".into(),
        repository_remote: Some("https://github.com/aimemory-test/frontend-web".into()),
        repository_name: "frontend-web".into(),
        user_text: "技术决定：导出进度每两秒轮询一次，任务进入终态后停止。".into(),
        assistant_text: "已确认，离开导出页时清理轮询计时器。".into(),
        session_path: "/example/session.jsonl".into(),
        code_repositories: Vec::new(),
    };
    let topics = [model_api::ArchiveTopic {
        id: "ARC-export".into(),
        title: "导出进度".into(),
        summary: "导出页需要展示后台导出任务的进度。".into(),
        repository_id: turn.repositories()[0].id.clone(),
        can_update: true,
        ..Default::default()
    }];
    let result = model_api::chat_extract(&service, &turn, &topics).await;
    let report = match &result {
        Ok(extraction) => {
            json!({"model": service.model, "success": true, "extraction": extraction})
        }
        Err(error) => json!({"model": service.model, "success": false, "error": error}),
    };
    record("extraction", report);
    let extraction = result.expect("归档测试失败，请查看脱敏诊断记录");
    assert_eq!(extraction.memories.len(), 1);
    assert_eq!(
        extraction.memories[0].target_id.as_deref(),
        Some("ARC-export")
    );
}
