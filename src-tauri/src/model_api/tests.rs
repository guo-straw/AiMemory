use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

pub(crate) struct HttpRequest {
    pub path: String,
    pub headers: String,
    pub body: Value,
}

fn serve(status: u16, response: Value) -> (ServiceCredentials, mpsc::Receiver<HttpRequest>) {
    serve_responses(vec![(status, response)])
}

pub(crate) fn serve_responses(
    responses: Vec<(u16, Value)>,
) -> (ServiceCredentials, mpsc::Receiver<HttpRequest>) {
    serve_bodies(
        responses
            .into_iter()
            .map(|(status, body)| (status, body.to_string()))
            .collect(),
    )
}

fn serve_bodies(
    responses: Vec<(u16, String)>,
) -> (ServiceCredentials, mpsc::Receiver<HttpRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for (status, response) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = Vec::new();
            let header_end = loop {
                let mut chunk = [0u8; 1024];
                let size = stream.read(&mut chunk).unwrap();
                assert!(size > 0);
                buffer.extend_from_slice(&chunk[..size]);
                if let Some(index) = buffer.windows(4).position(|part| part == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while buffer.len() < header_end + content_length {
                let mut chunk = [0u8; 1024];
                let size = stream.read(&mut chunk).unwrap();
                assert!(size > 0);
                buffer.extend_from_slice(&chunk[..size]);
            }
            sender
                .send(HttpRequest {
                    path: headers
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .into(),
                    body: if content_length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&buffer[header_end..header_end + content_length])
                            .unwrap()
                    },
                    headers,
                })
                .unwrap();
            let body = response;
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (
        ServiceCredentials {
            base_url: format!("http://{address}/v1/"),
            model: "test-model".into(),
            api_key: "test-only-key".into(),
        },
        receiver,
    )
}

fn turn() -> CodexTurn {
    CodexTurn {
        source_id: "session:turn".into(),
        session_id: "session".into(),
        turn_id: "turn".into(),
        completed_at: "2026-11-21T00:00:00Z".into(),
        cwd: "/project".into(),
        repository_remote: None,
        repository_name: "project".into(),
        user_text: "每两秒查询任务，终态停止。".into(),
        assistant_text: "页面卸载时清理计时器。".into(),
        session_path: "/sessions/session.jsonl".into(),
    }
}

#[tokio::test]
async fn chat_extends_an_existing_topic_and_separates_an_independent_change() {
    let mut turn = turn();
    turn.assistant_text = "页面卸载时清理计时器。另外，头像上传已支持 PNG。".into();
    let topics = [ArchiveTopic {
        id: "ARC-export".into(),
        title: "导出进度".into(),
        summary: "导出状态通过轮询刷新。".into(),
    }];
    let content = json!({"memories":[
        {"op":"update","target_id":"ARC-export", "title":"导出进度", "kind":"技术决策", "summary":"每两秒轮询进度，终态或卸载时停止", "details":"终态和卸载均清理计时器", "evidence":"每两秒查询任务，终态停止。", "tags":["轮询"]},
        {"op":"create","target_id":null, "title":"头像上传", "kind":"工作记录", "summary":"支持 PNG 头像", "details":"可选择 PNG 文件", "evidence":"头像上传已支持 PNG。", "tags":[]}
    ]});
    let (service, request) = serve(
        200,
        json!({"choices":[{"finish_reason":"stop","message":{"content":content.to_string()}}],"usage":{"prompt_tokens":40,"completion_tokens":20}}),
    );
    let result = chat_extract(&service, &turn, &topics).await.unwrap();
    let request = request.recv().unwrap();
    assert_eq!(request.path, "/v1/chat/completions");
    assert!(request
        .headers
        .to_lowercase()
        .contains("authorization: bearer test-only-key"));
    assert_eq!(request.body["response_format"]["type"], "json_object");
    assert_eq!(request.body["model"], "test-model");
    assert_eq!(request.body["max_completion_tokens"], 3000);
    assert!(request.body.get("max_tokens").is_none());
    let material: Value =
        serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(material["topics"], json!(topics));
    assert_eq!(result.memories.len(), 2);
    assert_eq!(result.memories[0].target_id.as_deref(), Some("ARC-export"));
    assert_eq!(result.memories[1].target_id, None);
    assert_eq!(result.usage.input_tokens, 40);
    assert_eq!(result.usage.output_tokens, 20);
    assert!(result.usage.reported);
    assert!(!result.input_truncated);
}

#[tokio::test]
async fn long_turn_has_a_bounded_api_input_and_retains_head_and_tail() {
    let (service, request) = serve(
        200,
        json!({"choices":[{"message":{"content":"{\"memories\":[]}"}}]}),
    );
    let mut turn = turn();
    turn.user_text = "开头".to_string() + &"中".repeat(10_000) + "结尾";
    turn.assistant_text = "开始".to_string() + &"文".repeat(20_000) + "结束";
    let result = chat_extract(&service, &turn, &[]).await.unwrap();
    let request = request.recv().unwrap();
    let material: Value =
        serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let user = material["user"].as_str().unwrap();
    let assistant = material["assistant"].as_str().unwrap();
    assert!(user.chars().count() + assistant.chars().count() <= 24_000);
    assert!(user.starts_with("开头") && user.ends_with("结尾"));
    assert!(assistant.starts_with("开始") && assistant.ends_with("结束"));
    assert!(user.contains("中间内容已省略"));
    assert!(result.memories.is_empty());
    assert!(result.input_truncated);
}

#[tokio::test]
async fn fabricated_evidence_is_rejected_before_markdown_is_created() {
    let content = json!({"memories":[{"op":"create","title":"轮询", "kind":"技术决策", "summary":"改用轮询", "details":"说明", "evidence":"这是模型改写的依据", "tags":[]}]});
    let (service, _request) = serve(
        200,
        json!({"choices":[{"message":{"content":content.to_string()}}]}),
    );
    let error = chat_extract(&service, &turn(), &[]).await.unwrap_err();
    assert!(error.contains("逐字找到"));
}

#[tokio::test]
async fn existing_topics_only_allow_one_update_with_evidence_from_this_turn() {
    let topics = [ArchiveTopic {
        id: "ARC-export".into(),
        title: "导出进度".into(),
        summary: "改用轮询跟踪导出。".into(),
    }];
    let memory = json!({"op":"update","target_id":"ARC-export","title":"导出进度","kind":"工作记录",
        "summary":"轮询跟踪导出，终态停止","details":"每两秒查询一次","evidence":"终态停止。"});
    let mut unknown = memory.clone();
    unknown["target_id"] = json!("ARC-other");
    let mut stale = memory.clone();
    stale["evidence"] = json!(topics[0].summary);
    for (memories, expected_error) in [
        (json!([unknown]), "候选范围以外"),
        (json!([memory, memory]), "拆成了多条"),
        (json!([stale]), "逐字找到"),
    ] {
        let (service, _request) = serve(
            200,
            json!({"choices":[{"message":{"content":json!({"memories":memories}).to_string()}}]}),
        );
        let error = chat_extract(&service, &turn(), &topics).await.unwrap_err();
        assert!(error.contains(expected_error), "{error}");
    }
}

#[tokio::test]
async fn embedding_uses_float_format_and_distinguishes_missing_usage() {
    let (service, request) = serve(200, json!({"data":[{"embedding":[0.1,-0.2,0.3]}]}));
    let result = embed(&service, "记忆摘要").await.unwrap();
    let request = request.recv().unwrap();
    assert_eq!(request.path, "/v1/embeddings");
    assert_eq!(request.body["encoding_format"], "float");
    assert_eq!(request.body["input"], "记忆摘要");
    assert_eq!(result.vector, vec![0.1, -0.2, 0.3]);
    assert!(!result.usage.reported);
}

#[tokio::test]
async fn test_connection_uses_configured_chat_endpoint() {
    let (service, request) = serve(
        200,
        json!({"choices":[{"message":{"content":"OK"}}],"usage":{"prompt_tokens":4,"completion_tokens":1}}),
    );
    let result = test_connection(&service, ServiceKind::Chat).await.unwrap();
    let request = request.recv().unwrap();
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.body["max_completion_tokens"], 1024);
    assert!(request.body.get("max_tokens").is_none());
    assert_eq!(result.input_tokens, 4);
    assert!(result.usage_reported);
}

#[tokio::test]
async fn service_error_reports_whitelisted_fields_and_hides_credentials() {
    let (service, _request) = serve(
        400,
        json!({"error":{
            "message":"Unsupported parameter 'max_tokens'. Use max_completion_tokens instead. Key: test-only-key",
            "code":"unsupported_parameter",
            "param":"max_tokens",
            "internal":"provider-internal-detail"
        }, "request":"unlisted-provider-body"}),
    );
    let error = embed(&service, "test").await.unwrap_err();
    assert!(error.contains("HTTP 400"));
    assert!(
        error.contains("Unsupported parameter 'max_tokens'. Use max_completion_tokens instead.")
    );
    assert!(error.contains("code: unsupported_parameter"));
    assert!(error.contains("param: max_tokens"));
    assert!(!error.contains("test-only-key"));
    assert!(!error.contains("provider-internal-detail"));
    assert!(!error.contains("unlisted-provider-body"));
}

#[tokio::test]
async fn service_error_redacts_encoded_keys_and_other_echoed_tokens() {
    let key = "test/key+quoted\"slash\\value";
    let encoded: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    let escaped = serde_json::to_string(key).unwrap();
    let escaped = &escaped[1..escaped.len() - 1];
    let (mut service, _request) = serve(
        401,
        json!({"error":{
            "message":format!("key {key}; url {encoded}; json {escaped}; bEaReR opaque.fake-token==; sk-proj-other_secret-123"),
            "code":"sk-proj-code-secret",
            "param":"Bearer parameter-secret"
        }}),
    );
    service.api_key = key.into();
    let error = embed(&service, "test").await.unwrap_err();
    for secret in [
        key,
        encoded.as_str(),
        escaped,
        "opaque.fake-token",
        "sk-proj-other_secret-123",
        "sk-proj-code-secret",
        "parameter-secret",
    ] {
        assert!(!error.contains(secret), "{error}");
    }
    assert!(error.contains("[已隐藏]"));
}

#[tokio::test]
async fn non_json_service_error_has_a_generic_message() {
    let (service, _request) = serve_bodies(vec![(
        502,
        "<html>private-provider-page test-only-key</html>".into(),
    )]);
    let error = embed(&service, "test").await.unwrap_err();
    assert!(error.contains("HTTP 502"));
    assert!(!error.contains("private-provider-page"));
    assert!(!error.contains("test-only-key"));
}

#[tokio::test]
async fn service_error_limits_displayed_field_lengths() {
    let (service, _request) = serve(
        400,
        json!({"error":{
            "message":"错".repeat(1000), "code":"c".repeat(1000), "param":"p".repeat(1000)
        }}),
    );
    let error = embed(&service, "test").await.unwrap_err();
    assert!(error.chars().count() < 610);
    assert!(error.contains('…'));
}

#[tokio::test]
async fn model_listing_uses_get_and_reports_invalid_payloads() {
    let (service, request) = serve(
        200,
        json!({"data":[{"id":"z-model"},{"id":"a-model"},{"id":"z-model"}]}),
    );
    assert_eq!(
        list_models(&service).await.unwrap(),
        vec!["a-model", "z-model"]
    );
    let request = request.recv().unwrap();
    assert!(request.headers.starts_with("GET /v1/models "));
    assert!(request
        .headers
        .to_lowercase()
        .contains("authorization: bearer test-only-key"));
    assert!(request.body.is_null());
    let (service, _request) = serve(200, json!({"models":["other-format"]}));
    assert!(list_models(&service)
        .await
        .unwrap_err()
        .contains("有效模型列表"));
}

#[tokio::test]
async fn deepseek_test_and_extraction_use_its_chat_parameters() {
    let (mut service, requests) = serve_responses(vec![
        (200, json!({"choices":[{"message":{"content":"OK"}}]})),
        (
            200,
            json!({"choices":[{"message":{"content":"{\"memories\":[]}"}}]}),
        ),
    ]);
    service.model = "deepseek-flash".into();
    test_connection(&service, ServiceKind::Chat).await.unwrap();
    chat_extract(&service, &turn(), &[]).await.unwrap();
    for expected_limit in [1024, 3000] {
        let request = requests.recv().unwrap();
        assert_eq!(request.body["model"], "deepseek-flash");
        assert_eq!(request.body["max_tokens"], expected_limit);
        assert_eq!(request.body["thinking"]["type"], "disabled");
        assert!(request.body.get("max_completion_tokens").is_none());
    }
}
