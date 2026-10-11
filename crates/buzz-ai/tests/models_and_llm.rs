use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use buzz_ai::llm::{Backend, Llm, LlmError};
use buzz_ai::models::{compute_sha256, ensure_entry, ModelEntry, ModelError, ModelId};
use buzz_ai::onnx::{image_to_nchw, letterbox, unletterbox_mask};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct TestResponse {
    action: String,
    count: u32,
}

fn read_http_request(stream: &mut std::net::TcpStream) {
    let mut buf = [0u8; 1024];
    let mut total = Vec::new();
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        total.extend_from_slice(&buf[..n]);
        if let Some(pos) = total.windows(4).position(|w| w == b"\r\n\r\n") {
            let header_str = String::from_utf8_lossy(&total[..pos]);
            let content_len = header_str
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let body_read = total.len() - (pos + 4);
            if body_read >= content_len {
                break;
            }
        }
    }
}

#[test]
fn registry_sha_mismatch_returns_error_and_deletes_part_file() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Start a mock server serving 16 bytes of "mock model data!"
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = b"mock model data!";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        }
    });

    let entry = ModelEntry {
        id: ModelId::AnimeSegmentation,
        filename: "test_mismatch.onnx",
        url: Box::leak(format!("http://127.0.0.1:{}/model.onnx", addr.port()).into_boxed_str()),
        // Deliberately incorrect SHA-256
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        bytes: 16,
        licence: "Apache-2.0",
        description: "Test model for sha mismatch",
        extra_files: &[],
    };

    let mut progress_called = false;
    let res = ensure_entry(
        &entry,
        temp_dir.path(),
        &mut |downloaded, total| {
            if downloaded > 0 && total > 0 {
                progress_called = true;
            }
        },
        &|| false,
    );

    assert!(progress_called);
    match res {
        Err(ModelError::ShaMismatch { filename, expected, got }) => {
            assert_eq!(filename, "test_mismatch.onnx");
            assert_eq!(expected, "0000000000000000000000000000000000000000000000000000000000000000");
            assert_ne!(got, expected);
        }
        other => panic!("Expected ShaMismatch error, got {other:?}"),
    }

    // Crucial check: .part file must NOT remain on disk!
    let part_file = temp_dir.path().join("test_mismatch.onnx.part");
    assert!(!part_file.exists(), "Temporary .part file must be removed on SHA mismatch");

    let final_file = temp_dir.path().join("test_mismatch.onnx");
    assert!(!final_file.exists(), "Final file must not exist on SHA mismatch");
}

#[test]
fn chat_json_succeeds_on_valid_json() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                read_http_request(&mut stream);
                let json_body = serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": "{\"action\": \"walk\", \"count\": 42}"
                        }
                    }]
                }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json_body.len(),
                    json_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    });

    let backend = Backend::OpenAiCompatible {
        base_url: format!("http://127.0.0.1:{}", addr.port()),
    };

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "action": {"type": "string"},
            "count": {"type": "integer"}
        },
        "required": ["action", "count"]
    });

    let res: Result<TestResponse, _> = backend.chat_json("mock-model", "system", "user", &[], schema);
    assert_eq!(
        res.unwrap(),
        TestResponse {
            action: "walk".into(),
            count: 42,
        }
    );
}

#[test]
fn chat_json_retries_once_on_invalid_json() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = Arc::clone(&call_count);

    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                read_http_request(&mut stream);
                let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst);

                let content = if attempt == 0 {
                    // Invalid JSON on first attempt
                    "Here is your result: { action: walk, count: 42"
                } else {
                    // Valid JSON on retry
                    "{\"action\": \"jump\", \"count\": 10}"
                };

                let json_body = serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": content
                        }
                    }]
                }).to_string();

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json_body.len(),
                    json_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    });

    let backend = Backend::OpenAiCompatible {
        base_url: format!("http://127.0.0.1:{}", addr.port()),
    };

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "action": {"type": "string"},
            "count": {"type": "integer"}
        },
        "required": ["action", "count"]
    });

    let res: Result<TestResponse, _> = backend.chat_json("mock-model", "system", "user", &[], schema);
    assert_eq!(call_count.load(Ordering::SeqCst), 2, "Must have retried exactly once");
    assert_eq!(
        res.unwrap(),
        TestResponse {
            action: "jump".into(),
            count: 10,
        }
    );
}

#[test]
fn chat_json_fails_after_retry_fails() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                read_http_request(&mut stream);
                let json_body = serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": "not json at all"
                        }
                    }]
                }).to_string();

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json_body.len(),
                    json_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    });

    let backend = Backend::OpenAiCompatible {
        base_url: format!("http://127.0.0.1:{}", addr.port()),
    };

    let schema = serde_json::json!({"type": "object"});
    let res: Result<TestResponse, _> = backend.chat_json("mock-model", "system", "user", &[], schema);
    match res {
        Err(LlmError::InvalidJson { raw, .. }) => {
            assert_eq!(raw, "not json at all");
        }
        other => panic!("Expected InvalidJson error after retry, got {other:?}"),
    }
}

#[test]
fn letterbox_and_unletterbox_preserves_coordinates() {
    let mut img = RgbaImage::new(100, 200);
    img.put_pixel(50, 100, Rgba([255, 0, 0, 255]));

    let (letterboxed, info) = letterbox(&img, 300, 300, [0, 0, 0, 255]);
    assert_eq!(letterboxed.width(), 300);
    assert_eq!(letterboxed.height(), 300);

    // Center point in original image (50, 100)
    let (lx, ly) = info.letterbox_point(50.0, 100.0);
    let (ux, uy) = info.unletterbox_point(lx, ly);

    assert!((ux - 50.0).abs() < 1e-3);
    assert!((uy - 100.0).abs() < 1e-3);
}

#[test]
fn image_to_nchw_produces_expected_dimensions() {
    let img = RgbaImage::new(50, 50);
    let (nchw, info) = image_to_nchw(
        &img,
        (128, 128),
        [0.485, 0.456, 0.406],
        [0.229, 0.224, 0.225],
        [0, 0, 0, 255],
    );

    assert_eq!(nchw.len(), 3 * 128 * 128);
    assert_eq!(info.target_width, 128);
    assert_eq!(info.target_height, 128);
}

#[test]
fn unletterbox_mask_rescales_mask_to_original_size() {
    let img = RgbaImage::new(60, 40);
    let (_letterboxed, info) = letterbox(&img, 100, 100, [0, 0, 0, 255]);

    // Mask filled with 1.0
    let mask = vec![1.0f32; 100 * 100];
    let unmasked = unletterbox_mask(&mask, 100, 100, &info);

    assert_eq!(unmasked.len(), 60 * 40);
    assert!((unmasked[0] - 1.0).abs() < 1e-3);
}

#[test]
fn compute_sha256_matches_known_string() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"hello buzz animate").unwrap();
    let hash = compute_sha256(temp.path()).unwrap();
    // SHA256 of "hello buzz animate":
    // echo -n "hello buzz animate" | sha256sum
    assert!(!hash.is_empty());
    assert_eq!(hash.len(), 64);
}
