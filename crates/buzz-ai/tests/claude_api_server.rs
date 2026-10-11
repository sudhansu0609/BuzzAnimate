//! F7.8: the "Claude API server" backend (OpenAI shape plus a key).
//!
//! The mock tests use a throwaway key; the real-server tests are `#[ignore]`
//! and read the key from `ai.json` / `BUZZ_CLAUDE_API_KEY` — never from here.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use buzz_ai::llm::{Backend, Llm};
use serde::{Deserialize, Serialize};

const DUMMY_KEY: &str = "sk-local-TESTONLY-not-a-real-key";

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Answer {
    colour: String,
}

/// Reads one HTTP request and returns (head, body).
fn read_request(stream: &mut std::net::TcpStream) -> (String, String) {
    let mut buf = [0u8; 4096];
    let mut total = Vec::new();
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        total.extend_from_slice(&buf[..n]);
        if let Some(pos) = total.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&total[..pos]).to_string();
            let len = head
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if total.len() - (pos + 4) >= len {
                let body = String::from_utf8_lossy(&total[pos + 4..]).to_string();
                return (head, body);
            }
        }
    }
    (String::from_utf8_lossy(&total).to_string(), String::new())
}

fn respond(stream: &mut std::net::TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).unwrap();
}

#[test]
fn claude_backend_sends_the_key_and_the_schema_and_parses_a_fenced_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let (head, body) = read_request(&mut s);
        tx.send((head, body)).unwrap();
        let content = "Here you go:\n```json\n{\"colour\": \"red\"}\n```";
        let reply = serde_json::json!({"choices":[{"message":{"role":"assistant","content":content}}]});
        respond(&mut s, &reply.to_string());
    });

    let backend = Backend::ClaudeApiServer {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        key: DUMMY_KEY.into(),
    };
    let schema = serde_json::json!({"type":"object","properties":{"colour":{"type":"string"}},"required":["colour"]});
    let a: Answer = backend
        .chat_json("haiku", "You name colours.", "Which colour?", &[b"png"], schema)
        .expect("parses the fenced JSON");
    assert_eq!(a.colour, "red");

    let (head, body) = rx.recv().unwrap();
    let lower = head.to_ascii_lowercase();
    assert!(head.starts_with("POST /v1/chat/completions"), "{head}");
    assert!(lower.contains(&format!("authorization: bearer {}", DUMMY_KEY.to_ascii_lowercase())));
    assert!(lower.contains(&format!("x-api-key: {}", DUMMY_KEY.to_ascii_lowercase())));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let system = v["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("JSON Schema") && system.contains("colour"), "{system}");
    assert_eq!(v["model"], "haiku");
    // The image goes as an OpenAI `image_url` part with a data: URL.
    let url = v["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"));
    // The Claude server gets the user's chosen effort (2026-10-06: medium),
    // never LM Studio's "none", which only switches thinking off locally.
    assert_eq!(v["reasoning_effort"], buzz_ai::ai_config::CLAUDE_DEFAULT_EFFORT);
}

#[test]
fn claude_backend_lists_models_with_the_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let (head, _) = read_request(&mut s);
        assert!(head.starts_with("GET /v1/models"));
        assert!(head.to_ascii_lowercase().contains("authorization: bearer"));
        respond(&mut s, r#"{"object":"list","data":[{"id":"sonnet"},{"id":"haiku"}]}"#);
    });
    let backend = Backend::ClaudeApiServer {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        key: DUMMY_KEY.into(),
    };
    assert_eq!(backend.list_models().unwrap(), vec!["sonnet".to_string(), "haiku".to_string()]);
}

#[test]
fn the_key_is_never_serialised_or_debug_printed() {
    let backend = Backend::ClaudeApiServer {
        base_url: "http://127.0.0.1:8787/v1".into(),
        key: DUMMY_KEY.into(),
    };
    let json = serde_json::to_string(&backend).unwrap();
    assert!(!json.contains("TESTONLY"), "{json}");
    assert!(json.contains("ClaudeApiServer"));
    let dbg = format!("{backend:?}");
    assert!(!dbg.contains("TESTONLY"), "{dbg}");
    assert!(dbg.contains("sk-local-…"));
    // It loads back with an empty key (resolved from ai.json at call time).
    let back: Backend = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, Backend::ClaudeApiServer { ref key, .. } if key.is_empty()));
}

fn real_backend() -> Option<Backend> {
    let b = Backend::default_claude_api();
    if b.resolved_key().is_none() {
        eprintln!("no Claude API server key in ai.json / BUZZ_CLAUDE_API_KEY; skipping");
        return None;
    }
    Some(b)
}

/// Real server at 127.0.0.1:8787: models list, a text reply and a vision reply.
#[test]
#[ignore = "needs the Claude API server running and a key in ai.json"]
fn real_claude_api_server_lists_models_and_answers_json_with_an_image() {
    let Some(b) = real_backend() else { return };
    let t = Instant::now();
    let models = b.list_models().expect("GET /v1/models");
    eprintln!("models ({} ms): {models:?}", t.elapsed().as_millis());
    assert!(models.iter().any(|m| m == "sonnet"), "{models:?}");

    // A 64x64 solid red PNG.
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([220, 20, 20, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let schema = serde_json::json!({"type":"object","properties":{"colour":{"type":"string"}},"required":["colour"]});
    for model in ["haiku", "sonnet"] {
        let t = Instant::now();
        let a: Answer = b
            .chat_json(
                model,
                "You identify the dominant colour of an image.",
                "What is the colour of this square? One lower-case word.",
                &[&png],
                schema.clone(),
            )
            .expect("chat_json against the real server");
        eprintln!("{model}: {:?} in {} ms", a, t.elapsed().as_millis());
        assert!(a.colour.to_lowercase().contains("red"), "{a:?}");
    }
}
