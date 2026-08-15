//! The film: many `.buzz` shots assembled into one movie.
//!
//! # Why a project file, not `Vec<Scene>` in one document
//!
//! The obvious design — several scenes inside one `.buzz` — buys in-app scene
//! switching and a single portable file, and costs a format break that makes
//! history, autosave, crash recovery and every panel scene-indexed: an enormous
//! change touching nearly everything, for a goal that is really **export
//! orchestration** — "render these shots as one film". A lightweight project
//! file solves that at a fraction of the size, with *no* change to the document
//! model. Shots stay independently editable, independently openable, and
//! independently versionable in git, and `Scene::extract`/`Scene::merge` already
//! move content between them.
//!
//! # Its own versioning
//!
//! A `.buzzproj` is a small JSON manifest that lists member shots. It versions
//! **independently** of the `.buzz` document format — the two evolve for
//! different reasons — so it carries its own [`PROJECT_VERSION`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The `.buzzproj` manifest version. Independent of the document format.
pub const PROJECT_VERSION: u32 = 1;

/// The file extension for a project.
pub const PROJECT_EXTENSION: &str = "buzzproj";

/// A film: an ordered list of shots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub shots: Vec<Shot>,
}

fn default_version() -> u32 {
    PROJECT_VERSION
}

/// One shot in the film: a member `.buzz`, at an optional angle and range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Path to the `.buzz`, **relative to the `.buzzproj`** so a project folder
    /// can be moved or shared whole.
    pub path: PathBuf,
    /// The inclusive frame range to render, or the whole document when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(u32, u32)>,
    /// A named camera angle to shoot the shot from — Wave 10b. The same staged
    /// `.buzz` can appear as several shots at several angles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<String>,
    /// A disabled shot stays in the list but is skipped when the film is built.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Shot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            range: None,
            angle: None,
            enabled: true,
        }
    }

    /// The shot's `.buzz` path, resolved against the directory the project file
    /// lives in.
    pub fn resolve(&self, project_dir: &Path) -> PathBuf {
        if self.path.is_absolute() {
            self.path.clone()
        } else {
            project_dir.join(&self.path)
        }
    }
}

/// Why a film cannot be assembled as it stands.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FilmError {
    #[error("the film has no enabled shots to render")]
    NoShots,
    #[error(
        "shot {shot} is {found} fps but the film is {expected} fps; \
         concatenation needs matching frame rates"
    )]
    FrameRateMismatch {
        shot: String,
        found: f64,
        expected: f64,
    },
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: PROJECT_VERSION,
            name: name.into(),
            shots: Vec::new(),
        }
    }

    /// Parse a `.buzzproj` from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialise to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a project always serialises")
    }

    /// The shots that will actually be rendered, in order.
    pub fn enabled_shots(&self) -> impl Iterator<Item = &Shot> {
        self.shots.iter().filter(|s| s.enabled)
    }

    /// Check that the film can be assembled with `-c copy` concatenation:
    /// there is at least one shot, and every shot shares the first one's frame
    /// rate. `frame_rate_of` looks a shot's document up — the caller has the
    /// loader — and is only called for enabled shots.
    ///
    /// Returned as a list so a UI can show every offending shot at once rather
    /// than one at a time.
    pub fn validate(
        &self,
        mut frame_rate_of: impl FnMut(&Shot) -> f64,
    ) -> Result<f64, Vec<FilmError>> {
        let mut shots = self.enabled_shots();
        let Some(first) = shots.next() else {
            return Err(vec![FilmError::NoShots]);
        };
        let expected = frame_rate_of(first);

        let mut errors = Vec::new();
        for shot in shots {
            let found = frame_rate_of(shot);
            if (found - expected).abs() > 1e-6 {
                errors.push(FilmError::FrameRateMismatch {
                    shot: shot.path.display().to_string(),
                    found,
                    expected,
                });
            }
        }
        if errors.is_empty() {
            Ok(expected)
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_round_trips_through_json() {
        let mut project = Project::new("My Film");
        project.shots.push(Shot::new("shots/wide.buzz"));
        let mut close = Shot::new("shots/scene.buzz");
        close.angle = Some("Close".into());
        close.range = Some((0, 47));
        project.shots.push(close);

        let back = Project::from_json(&project.to_json()).expect("parse");
        assert_eq!(back, project);
    }

    #[test]
    fn a_relative_shot_resolves_against_the_project_folder() {
        let shot = Shot::new("shots/wide.buzz");
        let resolved = shot.resolve(Path::new("/films/ep1"));
        assert_eq!(resolved, PathBuf::from("/films/ep1/shots/wide.buzz"));
    }

    #[test]
    fn an_absolute_shot_path_is_left_alone() {
        let abs = if cfg!(windows) {
            r"C:\art\wide.buzz"
        } else {
            "/art/wide.buzz"
        };
        let shot = Shot::new(abs);
        assert_eq!(shot.resolve(Path::new("/films/ep1")), PathBuf::from(abs));
    }

    #[test]
    fn disabled_shots_are_skipped() {
        let mut project = Project::new("Film");
        project.shots.push(Shot::new("a.buzz"));
        let mut off = Shot::new("b.buzz");
        off.enabled = false;
        project.shots.push(off);
        project.shots.push(Shot::new("c.buzz"));

        let names: Vec<_> = project
            .enabled_shots()
            .map(|s| s.path.display().to_string())
            .collect();
        assert_eq!(names, vec!["a.buzz", "c.buzz"]);
    }

    #[test]
    fn a_film_with_no_enabled_shots_is_rejected() {
        let mut project = Project::new("Empty");
        let mut off = Shot::new("a.buzz");
        off.enabled = false;
        project.shots.push(off);
        assert_eq!(project.validate(|_| 24.0), Err(vec![FilmError::NoShots]));
    }

    #[test]
    fn matching_frame_rates_validate_and_report_the_rate() {
        let mut project = Project::new("Film");
        project.shots.push(Shot::new("a.buzz"));
        project.shots.push(Shot::new("b.buzz"));
        assert_eq!(project.validate(|_| 24.0), Ok(24.0));
    }

    #[test]
    fn a_mismatched_frame_rate_is_reported_against_the_first_shot() {
        let mut project = Project::new("Film");
        project.shots.push(Shot::new("a.buzz")); // 24 fps, the reference
        project.shots.push(Shot::new("b.buzz")); // 30 fps, the offender
        let rate = |s: &Shot| if s.path.ends_with("b.buzz") { 30.0 } else { 24.0 };
        let errors = project.validate(rate).expect_err("should not validate");
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], FilmError::FrameRateMismatch { .. }));
    }

    #[test]
    fn an_older_manifest_without_version_still_loads() {
        // A hand-written project with no version field takes the default.
        let json = r#"{ "name": "Hand made", "shots": [] }"#;
        let project = Project::from_json(json).expect("parse");
        assert_eq!(project.version, PROJECT_VERSION);
    }
}
