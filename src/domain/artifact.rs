//! Named references attached to a task: an http(s) URL or an absolute file path.
//!
//! tsk stores the reference, never the thing it points at. Targets are not fetched or
//! checked, and every artifact records who attached it and which review round it came with.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// One reference attached to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// An http(s) URL or an absolute file path, as typed.
    pub target: String,
    /// Optional short name shown in place of `target`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `you`, or the agent profile that attached it.
    pub by: String,
    /// The review round it came with; zero outside a round.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What an artifact refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    /// Not an http(s) URL or an absolute path, or containing a terminal control character.
    Invalid,
    /// A remove named no artifact at that position.
    Unknown,
    /// An empty label was given.
    EmptyLabel,
    /// A label longer than [`super::BLOCK_TEXT_MAX`] bytes was given.
    LabelTooLong,
}

impl ArtifactError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid-artifact",
            Self::Unknown => "unknown-artifact",
            Self::EmptyLabel => "empty-artifact-label",
            Self::LabelTooLong => "artifact-label-too-long",
        }
    }
}

/// Validate a target and return its trimmed form. Nothing is fetched and no file is opened:
/// a URL needs an http(s) scheme and a non-empty rest, a path needs to be absolute. Control
/// characters are refused so a stored target can never inject terminal sequences when painted.
pub fn validate_target(input: &str) -> Result<String, ArtifactError> {
    let target = input.trim();
    if target.is_empty() || target.chars().any(char::is_control) {
        return Err(ArtifactError::Invalid);
    }
    let lower = target.to_ascii_lowercase();
    let url = ["http://", "https://"]
        .into_iter()
        .find(|scheme| lower.starts_with(scheme))
        .is_some_and(|scheme| target.len() > scheme.len());
    if url || Path::new(target).is_absolute() {
        Ok(target.to_string())
    } else {
        Err(ArtifactError::Invalid)
    }
}

/// Validate an optional label, trimming it and dropping empty labels.
pub fn validate_label(input: Option<&str>) -> Result<Option<String>, ArtifactError> {
    let Some(label) = input.map(str::trim) else {
        return Ok(None);
    };
    if label.is_empty() {
        return Err(ArtifactError::EmptyLabel);
    }
    if label.chars().any(char::is_control) {
        return Err(ArtifactError::Invalid);
    }
    if label.len() > super::BLOCK_TEXT_MAX {
        return Err(ArtifactError::LabelTooLong);
    }
    Ok(Some(label.to_string()))
}

impl Artifact {
    /// The name shown on the board and in `tsk list`: the label when set, else the target.
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_accept_urls_and_absolute_paths_only() {
        assert_eq!(
            validate_target(" https://example.com/pr/42 ").unwrap(),
            "https://example.com/pr/42"
        );
        assert!(validate_target("http://host").is_ok());
        assert!(validate_target("/tmp/shot.png").is_ok());
        if cfg!(windows) {
            assert!(validate_target("C:\\Users\\me\\shot.png").is_ok());
        }
        for bad in [
            "",
            " relative/path",
            "ftp://host",
            "https://",
            "http://a\nb",
        ] {
            assert_eq!(validate_target(bad), Err(ArtifactError::Invalid), "{bad}");
        }
    }

    #[test]
    fn labels_are_trimmed_and_empty_labels_are_refused() {
        assert_eq!(
            validate_label(Some("  PR #42 ")).unwrap().as_deref(),
            Some("PR #42")
        );
        assert_eq!(validate_label(None).unwrap(), None);
        assert_eq!(validate_label(Some("  ")), Err(ArtifactError::EmptyLabel));
    }
}
