// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use dimidiumlabs_config::{Error, Format, load};
use garde::Validate;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Validate)]
struct Config {
    #[garde(length(min = 1))]
    name: String,
    #[garde(skip)]
    port: u16,
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dimidiumlabs-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn loads_json_toml_yaml_and_yml_with_normalized_metadata() {
    let directory = TestDirectory::new();
    let cases = [
        (
            "service.JSON",
            r#"{"name":"api","port":8080}"#,
            Format::Json,
        ),
        ("service.toml", "name = 'api'\nport = 8080", Format::Toml),
        ("service.yaml", "name: api\nport: 8080", Format::Yaml),
        ("service.YmL", "name: api\nport: 8080", Format::Yaml),
    ];

    for (name, contents, format) in cases {
        let path = directory.file(name, contents);
        let loaded = load::<Config>("example", &path).await.unwrap();
        assert_eq!(
            loaded.config(),
            &Config {
                name: "api".into(),
                port: 8080
            }
        );
        assert_eq!(loaded.metadata().name(), "example");
        assert_eq!(loaded.metadata().path(), Path::new(&path));
        assert_eq!(loaded.metadata().format(), format);
    }
}

#[tokio::test]
async fn validation_failure_preserves_service_path_and_format() {
    let directory = TestDirectory::new();
    let path = directory.file("service.json", r#"{"name":"","port":8080}"#);

    let error = load::<Config>("example", &path).await.unwrap_err();
    let message = error.to_string();
    match error {
        Error::Validation { source, metadata } => {
            assert_eq!(metadata.name(), "example");
            assert_eq!(metadata.path(), path);
            assert_eq!(metadata.format(), Format::Json);
            assert!(!source.is_empty());
            assert!(message.contains(source.to_string().trim()));
        }
        other => panic!("expected validation error, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_input_returns_parser_specific_error() {
    let directory = TestDirectory::new();
    let path = directory.file("service.toml", "name = [");

    let error = load::<Config>("example", &path).await.unwrap_err();
    let message = error.to_string();
    match error {
        Error::Toml { source, metadata } => {
            assert_eq!(metadata.name(), "example");
            assert_eq!(metadata.path(), path);
            assert!(message.contains(&source.to_string()));
        }
        other => panic!("expected TOML error, got {other:?}"),
    }
}

#[tokio::test]
async fn unsupported_and_extensionless_paths_are_rejected_without_reading() {
    for path in [PathBuf::from("missing.ini"), PathBuf::from("missing")] {
        let error = load::<Config>("example", &path).await.unwrap_err();
        match error {
            Error::UnsupportedFormat {
                name,
                path: error_path,
            } => {
                assert_eq!(name, "example");
                assert_eq!(error_path, path);
            }
            other => panic!("expected unsupported format error, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn invalid_utf8_returns_utf8_error_with_exact_path() {
    let directory = TestDirectory::new();
    let path = directory.0.join("service.json");
    fs::write(&path, [0xff]).unwrap();

    let error = load::<Config>("example", &path).await.unwrap_err();
    match error {
        Error::Utf8 { metadata, .. } => {
            assert_eq!(metadata.name(), "example");
            assert_eq!(metadata.path(), path);
        }
        other => panic!("expected UTF-8 error, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_file_returns_read_error_with_exact_path() {
    let directory = TestDirectory::new();
    let path = directory.0.join("missing.json");

    let error = load::<Config>("example", &path).await.unwrap_err();
    let message = error.to_string();
    match error {
        Error::Read { source, metadata } => {
            assert_eq!(metadata.name(), "example");
            assert_eq!(metadata.path(), path);
            assert_eq!(metadata.format(), Format::Json);
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            assert!(message.contains(&source.to_string()));
        }
        other => panic!("expected read error, got {other:?}"),
    }
}
