//! Integration and verification tests for Chunk C11:
//! Directing through the local LLM.
//!
//! Validates:
//! - Strict schema generation for director verbs, settings, scenery, and beats.
//! - Natural language story brief parsing via mock LLM into structured `DirectorScript`.
//! - Conversion of script into canonical director prose.
//! - End-to-end execution creating staged actors, timeline keyframe performances, camera framing, and facial/gaze cues.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use buzz_scene::Scene;

use buzz_ai::director::{
    director_verbs_schema, execute_director_script, parse_story_to_script, script_to_story_text,
    DirectorBeat, DirectorScript, DirectorVerb,
};
use buzz_ai::llm::{Llm, LlmError};

/// Mock LLM for testing structured script generation from natural language prompts.
struct MockDirectorLlm {
    call_count: AtomicUsize,
    response: Mutex<serde_json::Value>,
}

impl MockDirectorLlm {
    fn new(response: serde_json::Value) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            response: Mutex::new(response),
        }
    }
}

impl Llm for MockDirectorLlm {
    fn chat_json<T: serde::de::DeserializeOwned>(
        &self,
        _model: &str,
        _system: &str,
        _user: &str,
        _images: &[&[u8]],
        _schema: serde_json::Value,
    ) -> Result<T, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let resp = self.response.lock().unwrap().clone();
        serde_json::from_value(resp).map_err(|e| LlmError::InvalidJson {
            raw: String::new(),
            error: e.to_string(),
        })
    }

    fn embed(&self, _model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
        Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
    }
}

#[test]
fn test_director_verbs_schema_structure() {
    let schema = director_verbs_schema();
    assert_eq!(schema["type"], "object");
    let props = &schema["properties"];
    assert!(props.get("setting").is_some());
    assert!(props.get("scenery").is_some());
    assert!(props.get("cast").is_some());
    assert!(props.get("beats").is_some());

    let verbs_enum = &props["beats"]["items"]["properties"]["verb"]["enum"]
        .as_array()
        .expect("verb enum array");
    let verb_strings: Vec<&str> = verbs_enum.iter().filter_map(|v| v.as_str()).collect();

    assert!(verb_strings.contains(&"enter_left"));
    assert!(verb_strings.contains(&"enter_right"));
    assert!(verb_strings.contains(&"walk_to"));
    assert!(verb_strings.contains(&"walk_across"));
    assert!(verb_strings.contains(&"talk"));
    assert!(verb_strings.contains(&"listen"));
    assert!(verb_strings.contains(&"idle"));
    assert!(verb_strings.contains(&"point"));
    assert!(verb_strings.contains(&"wave"));
    assert!(verb_strings.contains(&"look_at"));
}

#[test]
fn test_parse_story_to_script_with_mock_llm() {
    let mock_json = serde_json::json!({
        "setting": "night",
        "scenery": "village",
        "cast": ["Ramu", "Shyam"],
        "beats": [
            {
                "actor": "Ramu",
                "verb": "enter_left"
            },
            {
                "actor": "Ramu",
                "verb": "walk_across"
            },
            {
                "actor": "Ramu",
                "verb": "talk",
                "dialogue": "Good evening friend"
            },
            {
                "actor": "Shyam",
                "verb": "listen"
            },
            {
                "actor": "Ramu",
                "verb": "point",
                "gaze": [0.3, -0.7]
            },
            {
                "actor": "Ramu",
                "verb": "look_at",
                "target": "moon"
            }
        ]
    });

    let mock_llm = MockDirectorLlm::new(mock_json);
    let prompt = "Night in the village. Ramu walks in from the left and walks across. Ramu says 'Good evening friend' to Shyam, who listens. Ramu points and looks at the moon.";

    let script = parse_story_to_script(&mock_llm, "test-llm", prompt)
        .expect("parse_story_to_script should succeed");

    assert_eq!(mock_llm.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(script.setting, "night");
    assert_eq!(script.scenery, "village");
    assert_eq!(script.cast, vec!["Ramu", "Shyam"]);
    assert_eq!(script.beats.len(), 6);

    assert_eq!(script.beats[0].actor, "Ramu");
    assert_eq!(script.beats[0].verb, DirectorVerb::EnterLeft);

    assert_eq!(script.beats[2].verb, DirectorVerb::Talk);
    assert_eq!(script.beats[2].dialogue.as_deref(), Some("Good evening friend"));

    assert_eq!(script.beats[4].verb, DirectorVerb::Point);
    assert_eq!(script.beats[4].gaze, Some((0.3, -0.7)));

    assert_eq!(script.beats[5].verb, DirectorVerb::LookAt);
    assert_eq!(script.beats[5].target.as_deref(), Some("moon"));
}

#[test]
fn test_script_to_story_text_translation() {
    let script = DirectorScript {
        setting: "night".into(),
        scenery: "forest".into(),
        cast: vec!["Ana".into(), "Ben".into()],
        beats: vec![
            DirectorBeat {
                actor: "Ana".into(),
                verb: DirectorVerb::EnterLeft,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ana".into(),
                verb: DirectorVerb::Talk,
                target: Some("Ben".into()),
                dialogue: None,
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ben".into(),
                verb: DirectorVerb::Listen,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ben".into(),
                verb: DirectorVerb::WalkAway,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
        ],
    };

    let story_text = script_to_story_text(&script);
    assert!(story_text.contains("Night. Forest."));
    assert!(story_text.contains("Ana walks in from the left."));
    assert!(story_text.contains("Ana talks to Ben."));
    assert!(story_text.contains("Ben listens."));
    assert!(story_text.contains("Ben walks off."));
}

#[test]
fn test_execute_director_script_creates_valid_scene() {
    let mut scene = Scene::default();

    let script = DirectorScript {
        setting: "night".into(),
        scenery: "forest".into(),
        cast: vec!["Ana".into(), "Ben".into()],
        beats: vec![
            DirectorBeat {
                actor: "Ana".into(),
                verb: DirectorVerb::EnterLeft,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ana".into(),
                verb: DirectorVerb::Talk,
                target: Some("Ben".into()),
                dialogue: Some("Hello Ben".into()),
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ben".into(),
                verb: DirectorVerb::Listen,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
            DirectorBeat {
                actor: "Ben".into(),
                verb: DirectorVerb::WalkAway,
                target: None,
                dialogue: None,
                gaze: None,
                seconds: None,
            },
        ],
    };

    let directed = execute_director_script(&mut scene, &script)
        .expect("execute_director_script should succeed");

    assert_eq!(directed.names, vec!["Ana", "Ben"]);
    assert!(!directed.beats.is_empty(), "Should generate planned beats");
    assert!(directed.frames > 0, "Scene duration should be positive");

    // Cast members placed on stage
    assert_eq!(directed.staged.cast.len(), 2);

    // Verify camera tracking was automatically written
    assert!(scene.camera().enabled, "Camera track should be enabled");
}
