//! Print the embedded agent skill.

use super::CliOutput;

/// Full `skills/tsk-cli/SKILL.md`, including YAML frontmatter.
pub const SKILL_MD: &str = include_str!("../../skills/tsk-cli/SKILL.md");

/// Full `skills/tsk-coordinator/SKILL.md`, installed beside the tsk skill.
pub const COORDINATOR_MD: &str = include_str!("../../skills/tsk-coordinator/SKILL.md");

/// Skill body after the closing `---` fence and one following blank line.
pub fn body() -> &'static str {
    strip_frontmatter(SKILL_MD)
}

pub fn strip_frontmatter(source: &str) -> &str {
    let Some(rest) = source.strip_prefix("---\n") else {
        return source;
    };
    let Some(close) = rest.find("\n---\n") else {
        return source;
    };
    let after = &rest[close + "\n---\n".len()..];
    after.strip_prefix('\n').unwrap_or(after)
}

pub fn run() -> CliOutput {
    CliOutput {
        stdout: body().to_string(),
        stderr: String::new(),
        code: 0,
    }
}
