use super::*;

#[test]
fn encrypted_storage_authenticates_ciphertext_and_recovers_when_a_new_key_is_supplied() {
    let fixture = fixture();
    let mut store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let mut input = update_input(&store);
    input.chat.api_key = Some("test-secret-internal-aes".into());
    store.update(input).unwrap();
    let first = store.keys.stored("chat").unwrap().unwrap();
    assert!(!fs::read(fixture.path().join("credentials.sqlite"))
        .unwrap()
        .windows(b"test-secret-internal-aes".len())
        .any(|s| s == b"test-secret-internal-aes"));
    assert_eq!(
        fs::read(fixture.path().join("credentials.key"))
            .unwrap()
            .len(),
        32
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(fixture.path().join("credentials.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let mut input = update_input(&store);
    input.chat.api_key = Some("test-secret-internal-aes".into());
    store.update(input).unwrap();
    let second = store.keys.stored("chat").unwrap().unwrap();
    assert_ne!(first.0, second.0);
    assert_ne!(first.1, second.1);
    let mut corrupted = second;
    corrupted.1.as_mut().unwrap()[0] ^= 1;
    store.keys.restore("chat", Some(&corrupted)).unwrap();
    assert!(store.credentials(ServiceKind::Chat).is_err());
    let mut input = update_input(&store);
    input.chat.api_key = Some("replacement-secret".into());
    store.update(input).unwrap();
    assert_eq!(
        store.credentials(ServiceKind::Chat).unwrap().api_key,
        "replacement-secret"
    );
    let mut input = update_input(&store);
    input.chat.api_key = Some(String::new());
    store.update(input).unwrap();
    assert_eq!(store.keys.stored("chat").unwrap(), Some((None, None)));
}

#[test]
fn failing_config_write_restores_encrypted_rows_without_reading_the_system_keyring() {
    let fixture = fixture();
    let mut store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let mut input = update_input(&store);
    input.chat.api_key = Some("original-secret".into());
    store.update(input).unwrap();
    let row = store.keys.stored("chat").unwrap();
    fs::remove_file(fixture.path().join("services.json")).unwrap();
    fs::create_dir(fixture.path().join("services.json")).unwrap();
    let mut input = update_input(&store);
    input.chat.api_key = Some("unsaved-replacement".into());
    assert!(store.update(input).is_err());
    assert_eq!(store.keys.stored("chat").unwrap(), row);
    assert_eq!(
        store.credentials(ServiceKind::Chat).unwrap().api_key,
        "original-secret"
    );
}

fn fixture() -> tempfile::TempDir {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dev-data/test-runs");
    fs::create_dir_all(&directory).unwrap();
    tempfile::tempdir_in(directory).unwrap()
}

fn update_input(store: &SettingsStore) -> UpdateSettings {
    let current = store.config();
    UpdateSettings {
        chat: ServiceInput {
            provider: None,
            base_url: current.chat.base_url.clone(),
            model: current.chat.model.clone(),
            api_key: None,
        },
        embedding: ServiceInput {
            provider: None,
            base_url: current.embedding.base_url.clone(),
            model: current.embedding.model.clone(),
            api_key: None,
        },
        codex_directory: current.codex_directory.clone(),
        archive_enabled: false,
        run_in_background: false,
    }
}

#[test]
fn settings_persist_endpoints_and_archive_origin_without_plaintext_keys() {
    let fixture = fixture();
    let sessions = fixture.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mut store = SettingsStore::load(fixture.path(), sessions.clone()).unwrap();
    assert!(!store.config().archive_enabled);
    assert!(!store.config().run_in_background);
    let mut input = update_input(&store);
    input.chat.base_url = " http://127.0.0.1:4010/v1/ ".into();
    input.chat.model = " local-chat ".into();
    input.archive_enabled = true;
    store.update(input).unwrap();
    store
        .set_archive_started_at(Some("2026-11-21T00:00:00Z".into()))
        .unwrap();
    let disk = fs::read_to_string(fixture.path().join("services.json")).unwrap();
    assert!(!disk.contains("apiKey"));
    let reopened = SettingsStore::load(fixture.path(), sessions).unwrap();
    assert_eq!(reopened.config().chat.base_url, "http://127.0.0.1:4010/v1");
    assert_eq!(reopened.config().chat.model, "local-chat");
    assert_eq!(
        reopened.config().archive_started_at.as_deref(),
        Some("2026-11-21T00:00:00Z")
    );
    assert!(reopened.config().archive_enabled);
}

#[test]
fn invalid_service_input_keeps_existing_settings() {
    let fixture = fixture();
    let mut store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let mut input = update_input(&store);
    input.chat.base_url = "file:///tmp/service".into();
    assert!(store.update(input).is_err());
    assert_eq!(store.config().chat.base_url, "https://api.openai.com/v1");
    assert!(!fixture.path().join("services.json").exists());
}

#[test]
fn connection_test_uses_unsaved_settings_without_persisting_them() {
    let fixture = fixture();
    let store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let credentials = store
        .test_credentials(
            ServiceKind::Chat,
            ServiceInput {
                provider: None,
                base_url: "http://localhost:1234/v1/".into(),
                model: "chat-local".into(),
                api_key: Some("test-only-key".into()),
            },
        )
        .unwrap();
    assert_eq!(credentials.base_url, "http://localhost:1234/v1");
    assert_eq!(credentials.api_key, "test-only-key");
    assert!(!store.config().chat.has_api_key);
    assert!(!fixture.path().join("services.json").exists());
}

#[test]
fn encrypted_key_roundtrip_keeps_secrets_out_of_json() {
    let fixture = fixture();
    let mut store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let mut input = update_input(&store);
    input.chat.api_key = Some("aimemory-local-test-secret".into());
    store.update(input).unwrap();
    let view = store.view().unwrap();
    let original = store
        .credentials(ServiceKind::Chat)
        .map(|credentials| credentials.api_key);
    let disk = fs::read_to_string(fixture.path().join("services.json")).unwrap();
    let mut preserved = update_input(&store);
    preserved.chat.model = "changed-model".into();
    let updated = store.update(preserved);
    let after_update = store
        .credentials(ServiceKind::Chat)
        .map(|credentials| credentials.api_key);
    let mut removal = update_input(&store);
    removal.chat.api_key = Some(String::new());
    store.update(removal).unwrap();
    assert!(!store.view().unwrap().chat.has_api_key);
    assert!(store.read_key(ServiceKind::Chat).unwrap().is_none());
    assert!(view.chat.has_api_key);
    assert_eq!(original.unwrap(), "aimemory-local-test-secret");
    assert!(updated.is_ok());
    assert_eq!(after_update.unwrap(), "aimemory-local-test-secret");
    assert!(!disk.contains("aimemory-local-test-secret"));
}

#[test]
fn legacy_settings_preserve_models_and_only_reuse_keys_at_the_saved_endpoint() {
    let fixture = fixture();
    fs::write(
        fixture.path().join("services.json"),
        r#"{
      "chat":{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat","hasApiKey":true},
      "embedding":{"baseUrl":"http://localhost:1234/v1","model":"local-embed"},
      "codexDirectory":"/sessions"
    }"#,
    )
    .unwrap();
    let store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    assert_eq!(store.config().chat.model, "deepseek-chat");
    assert!(store.config().chat.provider.is_none());
    let credentials = store
        .test_credentials(
            ServiceKind::Chat,
            ServiceInput {
                provider: Some(ServiceProvider::Custom),
                base_url: "http://localhost:1234/v1".into(),
                model: "local-chat".into(),
                api_key: None,
            },
        )
        .unwrap();
    assert!(credentials.api_key.is_empty());
    let credentials = store
        .models_credentials(ServiceKind::Chat, "http://localhost:1234/v1", None)
        .unwrap();
    assert!(credentials.api_key.is_empty());
}

#[test]
fn encrypted_key_provider_change_clears_previous_key() {
    let fixture = fixture();
    let mut store = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    let mut original = update_input(&store);
    original.chat.api_key = Some("aimemory-provider-test-key".into());
    store.update(original).unwrap();
    let mut changed = update_input(&store);
    changed.chat.provider = Some(ServiceProvider::Deepseek);
    changed.chat.base_url = "https://api.deepseek.com".into();
    changed.chat.model = "deepseek-flash".into();
    assert!(store
        .test_credentials(ServiceKind::Chat, changed.chat.clone())
        .unwrap()
        .api_key
        .is_empty());
    store.update(changed).unwrap();
    let reopened = SettingsStore::load(fixture.path(), fixture.path().join("sessions")).unwrap();
    assert_eq!(reopened.config().chat.model, "deepseek-flash");
    assert!(!reopened.config().chat.has_api_key);
    assert!(reopened
        .credentials(ServiceKind::Chat)
        .unwrap()
        .api_key
        .is_empty());
    assert!(reopened.read_key(ServiceKind::Chat).unwrap().is_none());
    assert!(matches!(
        reopened.config().chat.provider,
        Some(ServiceProvider::Deepseek)
    ));
}
