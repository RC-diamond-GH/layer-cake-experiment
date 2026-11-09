use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

// Every child runs outside the repository with a synthetic .env. The user's
// .env is never opened, and the tests do not mutate global process variables.
struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("layer-cake-env-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).expect("fixture directory");
        Self { directory }
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.directory.join(name), content).expect("fixture file");
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_layer-cake"));
        command
            .current_dir(&self.directory)
            .args(arguments)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("OPENAI_MODEL")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn serve_once(
    status: &str,
    body: &str,
) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP listener");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let status = status.to_owned();
    let body = body.to_owned();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let (mut stream, _) = listener.accept().await.expect("request");
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await.expect("read request");
                assert!(count > 0, "request ended before its body");
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .expect("content length");
                    if bytes.len() >= end + 4 + length {
                        break (end, length);
                    }
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers");
            let request: Value =
                serde_json::from_slice(&bytes[header_end + 4..header_end + 4 + length])
                    .expect("request JSON");
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response");
            (headers, request)
        })
        .await
        .expect("HTTP request deadline")
    });
    (base, server)
}

const SUCCESS: &str = r#"{"choices":[{"message":{"role":"assistant","content":"configured model answered"},"finish_reason":"stop"}]}"#;
const RUN: &[&str] = &["run", "--config", "config.json", "--output", "run.json"];

#[tokio::test]
async fn dotenv_base_paths_model_selection_and_credential_free_replay() {
    for prefix in ["", "/", "/v1", "/gateway/v1/"] {
        let (base, server) = serve_once("200 OK", SUCCESS).await;
        let fixture = Fixture::new();
        fixture.write(".env", &format!("OPENAI_API_KEY=fixture-secret\nOPENAI_BASE_URL={base}{prefix}\nOPENAI_MODEL=fixture-model\n"));
        fixture.write(
            "config.json",
            r#"{"model":{"kind":"openai"},"generation":{"temperature":0.0,"max_tokens":64}}"#,
        );
        let result = fixture.command(RUN).output().await.expect("run CLI");
        let (headers, request) = server.await.expect("server");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(headers.starts_with(&format!(
            "POST {}/chat/completions HTTP/1.1",
            prefix.trim_end_matches('/')
        )));
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer fixture-secret")
        );
        assert_eq!(request["model"], "fixture-model");
        let saved = std::fs::read_to_string(fixture.directory.join("run.json")).expect("saved run");
        assert!(!saved.contains("fixture-secret"));
        let data: Value = serde_json::from_str(&saved).expect("run JSON");
        assert_eq!(
            data["config"]["model"]["endpoint"],
            format!("{base}{}/chat/completions", prefix.trim_end_matches('/'))
        );
        assert_eq!(data["config"]["model"]["model"], "fixture-model");
        assert_eq!(data["summary"]["status"], "finished");
        std::fs::remove_file(fixture.directory.join(".env")).expect("remove fixture credentials");
        let replay = fixture
            .command(&["replay", "run.json"])
            .output()
            .await
            .expect("replay CLI");
        assert!(
            replay.status.success(),
            "{}",
            String::from_utf8_lossy(&replay.stderr)
        );
        let summary: Value = serde_json::from_slice(&replay.stdout).expect("replay summary");
        assert_eq!(summary["model_calls"], 1);
    }
}

#[tokio::test]
async fn process_environment_precedes_dotenv_and_explicit_config_precedes_environment() {
    let (base, server) = serve_once("200 OK", SUCCESS).await;
    let fixture = Fixture::new();
    fixture.write(".env", "OPENAI_API_KEY=unused-file-secret\nOPENAI_BASE_URL=http://127.0.0.1:1/unused\nOPENAI_MODEL=unused-file-model\n");
    fixture.write("config.json", &json!({"model":{"kind":"openai","endpoint":format!("{base}/custom/chat/completions"),"model":"explicit-model"}}).to_string());
    let result = fixture
        .command(RUN)
        .env("OPENAI_API_KEY", "process-secret")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:2/unused")
        .env("OPENAI_MODEL", "process-model")
        .output()
        .await
        .expect("run CLI");
    let (headers, request) = server.await.expect("server");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(headers.starts_with("POST /custom/chat/completions HTTP/1.1"));
    assert!(
        headers
            .to_lowercase()
            .contains("authorization: bearer process-secret")
    );
    assert_eq!(request["model"], "explicit-model");
}

#[tokio::test]
async fn empty_404_and_other_http_failures_retain_status_and_redact_credentials() {
    for (status, body, kind, detail) in [
        ("404 Not Found", "", "http_404", "empty response body"),
        (
            "502 Bad Gateway",
            "upstream unavailable",
            "http_502",
            "non-JSON response body",
        ),
        (
            "401 Unauthorized",
            r#"{"error":{"message":"rejected fixture-secret"}}"#,
            "http_401",
            "rejected [REDACTED]",
        ),
        ("200 OK", "", "response_format", "empty response body"),
    ] {
        let (base, server) = serve_once(status, body).await;
        let fixture = Fixture::new();
        fixture.write(".env", &format!("OPENAI_API_KEY=fixture-secret\nOPENAI_BASE_URL={base}/v1\nOPENAI_MODEL=fixture-model\n"));
        fixture.write("config.json", r#"{"model":{"kind":"openai"}}"#);
        let result = fixture.command(RUN).output().await.expect("run CLI");
        server.await.expect("server");
        let error = String::from_utf8(result.stderr).expect("error text");
        assert!(!result.status.success());
        assert!(
            error.contains(kind) && error.contains(status) && error.contains(detail),
            "{error}"
        );
        assert!(!error.contains("fixture-secret"));
        if kind == "http_404" {
            assert!(error.contains("/chat/completions"));
        }
        let saved = std::fs::read_to_string(fixture.directory.join("run.json")).expect("error run");
        assert!(!saved.contains("fixture-secret"));
        let data: Value = serde_json::from_str(&saved).expect("run JSON");
        let call = data["records"]
            .as_array()
            .expect("records")
            .iter()
            .find(|r| r["kind"] == "call")
            .expect("model call");
        assert_eq!(call["data"]["error"]["kind"], kind);
    }
}

#[tokio::test]
async fn invalid_dotenv_is_reported_without_echoing_its_contents() {
    let fixture = Fixture::new();
    fixture.write(".env", "OPENAI_API_KEY='fixture-secret\n");
    fixture.write("config.json", r#"{"model":{"kind":"openai"}}"#);
    let result = fixture.command(RUN).output().await.expect("run CLI");
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).expect("error text");
    assert!(error.contains("could not load .env"));
    assert!(!error.contains("fixture-secret"));
    assert!(!Path::new(&fixture.directory.join("run.json")).exists());
}
