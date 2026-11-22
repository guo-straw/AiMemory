use super::*;
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
};

fn serve(status: u16, headers: &str, body: &str) -> (String, thread::JoinHandle<String>) {
    let response = format!(
        "HTTP/1.1 {status} Test\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (base, server) = serve_raw(response.into_bytes());
    (format!("{base}/releases/latest"), server)
}

fn serve_raw(response: Vec<u8>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = BufReader::new(&stream)
            .lines()
            .map(Result::unwrap)
            .take_while(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        stream.write_all(&response).unwrap();
        request
    });
    (endpoint, server)
}

#[tokio::test]
async fn compares_release_precedence_and_links_the_returned_tag() {
    for (current, tag, expected) in [
        ("0.2.2", "v0.10.0", "available"),
        ("0.2.2-rc.1", "v0.2.2", "available"),
        ("0.2.2", "v0.2.2", "current"),
        ("0.2.2+local", "0.2.2+ci", "current"),
        ("0.2.2", "v0.2.1", "ahead"),
    ] {
        let (endpoint, server) = serve(
            200,
            "",
            &json!({ "tag_name": tag, "assets": [] }).to_string(),
        );
        let result = check_release(&endpoint, &current.parse().unwrap(), None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "latestVersion": tag.trim_start_matches('v'),
                "status": expected,
                "releaseUrl": format!("{RELEASES}/{tag}"),
                "installer": null
            })
        );
        let request = server.join().unwrap().to_lowercase();
        assert!(request.starts_with("get /releases/latest "));
        assert!(request.contains(concat!("user-agent: knowtrace/", env!("CARGO_PKG_VERSION"))));
        assert!(!request.contains("authorization:"));
    }
}

#[tokio::test]
async fn can_retry_private_release_with_a_token() {
    let current = Version::new(0, 2, 2);
    let (endpoint, server) = serve(404, "", "{}");
    let error = check_release(&endpoint, &current, None).await.unwrap_err();
    assert!(matches!(error, UpdateError::Access(_)));
    server.join().unwrap();

    let (endpoint, server) = serve(200, "", r#"{"tag_name":"v0.2.3","assets":[]}"#);
    let result = check_release(&endpoint, &current, Some(" test-token "))
        .await
        .unwrap();
    assert!(matches!(result.status, UpdateStatus::Available));
    let headers = server.join().unwrap().to_lowercase();
    assert!(headers.contains("authorization: bearer test-token"));
    assert!(headers.contains("accept: application/vnd.github+json"));
}

#[tokio::test]
async fn reports_rate_limits_access_failures_and_unusable_release_data() {
    for (status, headers, body, token, kind) in [
        (403, "X-RateLimit-Remaining: 0\r\n", "{}", None, "access"),
        (429, "", "{}", Some("token"), "check"),
        (401, "", "{}", Some("invalid"), "access"),
        (403, "", "{}", Some("unprivileged"), "access"),
        (503, "", "{}", None, "check"),
        (
            200,
            "",
            r#"{"tag_name":"vNext","assets":[]}"#,
            None,
            "check",
        ),
        (200, "", "{}", None, "check"),
    ] {
        let (endpoint, server) = serve(status, headers, body);
        let error = check_release(&endpoint, &Version::new(0, 2, 2), token)
            .await
            .unwrap_err();
        let wire_error = serde_json::to_value(error).unwrap();
        assert_eq!(wire_error["kind"], kind);
        assert!(!wire_error["message"].as_str().unwrap().is_empty());
        server.join().unwrap();
    }
}

#[test]
fn selects_the_published_installer_for_each_desktop_target() {
    let assets = json!([
        { "id": 1, "name": "aimemory-mcp_0.2.1_x86_64-pc-windows-msvc.exe", "size": 35 },
        { "id": 2, "name": "AiMemory_0.2.1_aarch64.dmg", "size": 13321232 },
        { "id": 3, "name": "AiMemory_0.2.1_x64.dmg", "size": 14209275 },
        { "id": 4, "name": "AiMemory_0.2.1_x64-setup.exe", "size": 9807572 }
    ]);
    for (os, arch, id) in [
        ("macos", "aarch64", Some(2)),
        ("macos", "x86_64", Some(3)),
        ("windows", "x86_64", Some(4)),
        ("windows", "aarch64", None),
    ] {
        let selected = select_installer(
            serde_json::from_value(assets.clone()).unwrap(),
            &Version::new(0, 2, 1),
            os,
            arch,
        );
        assert_eq!(selected.map(|asset| asset.asset_id), id);
    }
}

#[test]
fn selects_knowtrace_installers_after_the_rename() {
    let assets = json!([
        { "id": 1, "name": "KnowTrace_0.2.3_aarch64.dmg", "size": 13321232 },
        { "id": 2, "name": "KnowTrace_0.2.3_x64.dmg", "size": 14209275 },
        { "id": 3, "name": "KnowTrace_0.2.3_x64-setup.exe", "size": 9807572 }
    ]);
    for (os, arch, id) in [
        ("macos", "aarch64", 1),
        ("macos", "x86_64", 2),
        ("windows", "x86_64", 3),
    ] {
        let selected = select_installer(
            serde_json::from_value(assets.clone()).unwrap(),
            &Version::new(0, 2, 3),
            os,
            arch,
        );
        assert_eq!(selected.unwrap().asset_id, id);
    }
}

#[tokio::test]
async fn streams_private_asset_without_forwarding_token_to_download_host() {
    let contents = [0x41, 0x69, 0x00, 0xff];
    let mut binary =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 4\r\n\r\n"
            .to_vec();
    binary.extend_from_slice(&contents);
    let (cdn, cdn_server) = serve_raw(binary);
    let redirect =
        format!("HTTP/1.1 302 Found\r\nLocation: {cdn}/file\r\nContent-Length: 0\r\n\r\n");
    let (api, api_server) = serve_raw(redirect.into_bytes());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("AiMemory.dmg");
    std::fs::write(&path, b"previous data").unwrap();
    download_asset(
        &format!("{api}/releases/assets/123"),
        4,
        &path,
        Some("test-key"),
    )
    .await
    .unwrap();
    let api_request = api_server.join().unwrap().to_lowercase();
    assert!(api_request.starts_with("get /releases/assets/123 "));
    assert!(api_request.contains("accept: application/octet-stream"));
    assert!(api_request.contains("authorization: bearer test-key"));
    let cdn_request = cdn_server.join().unwrap().to_lowercase();
    assert!(cdn_request.starts_with("get /file "));
    assert!(!cdn_request.contains("authorization:"));
    assert_eq!(std::fs::read(&path).unwrap(), contents);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn incomplete_downloads_leave_the_selected_file_intact_and_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("AiMemory.dmg");
    std::fs::write(&destination, b"old").unwrap();
    for (response, kind) in [
        (
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            "access",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npart",
            "check",
        ),
        ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad", "check"),
    ] {
        let (api, server) = serve_raw(response.as_bytes().to_vec());
        let error = download_asset(&api, 4, &destination, None)
            .await
            .unwrap_err();
        assert_eq!(serde_json::to_value(error).unwrap()["kind"], kind);
        server.join().unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    let (api, server) = serve_raw(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfull".to_vec());
    download_asset(&api, 4, &destination, None).await.unwrap();
    server.join().unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"full");
}
