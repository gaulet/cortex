//! E2E tests avec un vrai mock HTTP server.
//!
//! On lance un tokio::net::TcpListener qui capture les requêtes reçues,
//! et on vérifie que le dispatcher envoie bien les bons payloads.

use cortex_webhooks::{WebhookConfig, WebhookDispatcher, WebhookEvent};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Mock HTTP server minimal : accepte 1 POST, capture la requête, répond 200 OK.
async fn start_mock_server() -> (String, Arc<Mutex<Option<MockRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{}/webhook", port);
    let captured: Arc<Mutex<Option<MockRequest>>> = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            // Read request (headers + body)
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                match stream.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            // Headers terminés, lire le body
                            if let Some(content_length) = parse_content_length(&buf) {
                                let header_end = find_header_end(&buf);
                                let body_so_far = buf.len() - header_end - 4;
                                if body_so_far >= content_length {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let raw = String::from_utf8_lossy(&buf).to_string();
            let (method, path, headers, body) = parse_request(&raw);
            *captured_clone.lock().await = Some(MockRequest {
                method,
                path,
                headers,
                body,
            });
            // Respond 200 OK
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(response).await;
            let _ = stream.shutdown().await;
        }
    });

    // Petit délai pour que le serveur soit prêt
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, captured)
}

#[derive(Debug, Clone)]
struct MockRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

fn parse_content_length(buf: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(buf).ok()?;
    for line in s.lines() {
        if let Some(rest) = line.to_lowercase().strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

fn find_header_end(buf: &[u8]) -> usize {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len())
}

fn parse_request(raw: &str) -> (String, String, Vec<(String, String)>, String) {
    let mut lines = raw.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = vec![];
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if in_body {
            body.push_str(line);
            body.push('\n');
        } else if line.is_empty() {
            in_body = true;
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    (method, path, headers, body.trim().to_string())
}

#[tokio::test]
async fn test_e2e_dispatch_plan_generated() {
    let (url, captured) = start_mock_server().await;
    let cfg = WebhookConfig::from_urls(vec![url]);
    let dispatcher = WebhookDispatcher::new(cfg).expect("enabled");

    let result = dispatcher
        .fire_blocking(
            WebhookEvent::PlanGenerated,
            Some("proj-e2e-1".to_string()),
            serde_json::json!({"themes_count": 3, "criticity_max": 4}),
        )
        .await;
    assert!(result.is_ok(), "fire_blocking should succeed: {:?}", result);

    let req = captured.lock().await.clone().expect("request was captured");
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/webhook");

    // Headers
    let event_header = req
        .headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "x-cortex-event")
        .map(|(_, v)| v.as_str());
    assert_eq!(event_header, Some("plan_generated"));

    let delivery_header = req
        .headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "x-cortex-delivery-id")
        .map(|(_, v)| v.as_str());
    assert!(delivery_header.is_some());
    assert!(delivery_header.unwrap().starts_with("wh_"));

    // Body
    let body: JsonValue = serde_json::from_str(&req.body).expect("body is JSON");
    assert_eq!(body["event"], "plan_generated");
    assert_eq!(body["project_id"], "proj-e2e-1");
    assert_eq!(body["data"]["themes_count"], 3);
    assert!(body["delivery_id"].as_str().unwrap().starts_with("wh_"));
    // Pas de signature (pas configurée)
    assert!(body.get("signature").is_none());
}

#[tokio::test]
async fn test_e2e_dispatch_with_hmac_signature() {
    let (url, captured) = start_mock_server().await;
    let mut cfg = WebhookConfig::from_urls(vec![url]);
    cfg.hmac_secret = Some("my-very-secret-key".to_string());
    let dispatcher = WebhookDispatcher::new(cfg).expect("enabled");
    assert!(dispatcher.has_hmac());

    dispatcher
        .fire_blocking(
            WebhookEvent::Abort,
            Some("p-2".into()),
            serde_json::json!({"reason": "manual"}),
        )
        .await
        .expect("send");

    let req = captured.lock().await.clone().expect("captured");
    let body: JsonValue = serde_json::from_str(&req.body).expect("json");
    let sig = body["signature"].as_str().expect("signature present");
    assert!(sig.starts_with("hmac_sha256="));
    assert_eq!(sig.len(), "hmac_sha256=".len() + 64); // 32 bytes hex
}

#[tokio::test]
async fn test_e2e_dispatch_multiple_urls() {
    // Lance 2 mock servers, vérifie qu'on hit les 2
    let (url1, cap1) = start_mock_server().await;
    let (url2, cap2) = start_mock_server().await;
    let cfg = WebhookConfig::from_urls(vec![url1.clone(), url2.clone()]);
    let dispatcher = WebhookDispatcher::new(cfg).expect("enabled");
    assert_eq!(dispatcher.url_count(), 2);

    dispatcher
        .fire_blocking(
            WebhookEvent::PreMortemEmitted,
            Some("p-3".into()),
            serde_json::json!({"guardrails": 5}),
        )
        .await
        .expect("send to both");

    assert!(cap1.lock().await.is_some(), "url1 should receive");
    assert!(cap2.lock().await.is_some(), "url2 should receive");
}

#[tokio::test]
async fn test_e2e_retry_on_5xx() {
    // Mock server qui répond 500 2 fois puis 200
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/webhook", port);
    let attempt_count = Arc::new(Mutex::new(0u32));
    let attempt_count_clone = attempt_count.clone();

    tokio::spawn(async move {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![];
                let mut tmp = [0u8; 4096];
                // Drain request
                loop {
                    match stream.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                if let Some(cl) = parse_content_length(&buf) {
                                    let h_end = find_header_end(&buf);
                                    if buf.len() - h_end - 4 >= cl {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                let mut count = attempt_count_clone.lock().await;
                *count += 1;
                let status = if *count < 3 { "500" } else { "200" };
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: 0\r\n\r\n",
                    status,
                    if *count < 3 {
                        "Internal Server Error"
                    } else {
                        "OK"
                    }
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Configure avec retries courts pour le test
    let mut cfg = WebhookConfig::from_urls(vec![url]);
    cfg.max_retries = 3;
    cfg.initial_backoff = std::time::Duration::from_millis(10); // rapide pour le test
    let dispatcher = WebhookDispatcher::new(cfg).expect("enabled");

    let result = dispatcher
        .fire_blocking(
            WebhookEvent::RecoveryTriggered,
            Some("p-retry".into()),
            serde_json::json!({"uncommitted": 2}),
        )
        .await;
    assert!(result.is_ok(), "should eventually succeed: {:?}", result);
    let count = *attempt_count.lock().await;
    assert_eq!(count, 3, "should have retried twice then succeeded");
}

#[tokio::test]
async fn test_e2e_no_dispatch_when_disabled() {
    let cfg = WebhookConfig::from_urls(vec![]);
    assert!(WebhookDispatcher::new(cfg).is_none());
}
