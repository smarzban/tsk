//! Shared project-path resolution for board and headless capture.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::context::InvocationSnapshot;
use crate::domain::{DomainState, TaskScope};

/// Whether two project-path spellings name the same directory.
///
/// Comparison only: stored scope identity is never rewritten through this. When both
/// sides exist on disk they are compared canonically, so the same repository reached
/// as `/tmp/repo` and `/private/tmp/repo` (macOS) still matches. A missing or
/// nonexistent side falls back to platform lexical equality (case-insensitive on Windows),
/// so a stored path whose directory has not been created yet stays addressable.
pub fn paths_equivalent(a: &str, b: &str) -> bool {
    if lexical_path_eq(trim(a), trim(b)) {
        return true;
    }
    match (
        std::fs::canonicalize(Path::new(a)),
        std::fs::canonicalize(Path::new(b)),
    ) {
        (Ok(ca), Ok(cb)) => lexical_path_eq(&ca.to_string_lossy(), &cb.to_string_lossy()),
        _ => false,
    }
}

/// Whether a stored project-path set contains an equivalent spelling.
pub fn archived_path_contains(paths: &BTreeSet<String>, path: &str) -> bool {
    paths.iter().any(|stored| paths_equivalent(stored, path))
}

/// Bounded path identity memo used by one board query. It is intentionally owned by
/// the operation, never global, so aliases are refreshed on the next query and a
/// missing path cannot become permanently stale.
#[derive(Debug, Default)]
pub(crate) struct PathIdentityCache {
    canonical: RefCell<BTreeMap<String, Option<PathBuf>>>,
}

impl PathIdentityCache {
    pub(crate) fn equivalent(&self, a: &str, b: &str) -> bool {
        if lexical_path_eq(trim(a), trim(b)) {
            return true;
        }
        match (self.canonical_path(a), self.canonical_path(b)) {
            (Some(a), Some(b)) => lexical_path_eq(&a.to_string_lossy(), &b.to_string_lossy()),
            _ => false,
        }
    }

    fn canonical_path(&self, path: &str) -> Option<PathBuf> {
        let key = trim(path).to_string();
        if let Some(value) = self.canonical.borrow().get(&key) {
            return value.clone();
        }
        let value = std::fs::canonicalize(Path::new(&key)).ok();
        self.canonical.borrow_mut().insert(key, value.clone());
        value
    }

    pub(crate) fn contains(&self, paths: &BTreeSet<String>, path: &str) -> bool {
        paths.iter().any(|stored| self.equivalent(stored, path))
    }
}

#[cfg(not(windows))]
fn lexical_path_eq(a: &str, b: &str) -> bool {
    a == b
}

#[cfg(windows)]
fn lexical_path_eq(a: &str, b: &str) -> bool {
    use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};

    let a = a.encode_utf16().collect::<Vec<_>>();
    let b = b.encode_utf16().collect::<Vec<_>>();
    let (Ok(a_len), Ok(b_len)) = (i32::try_from(a.len()), i32::try_from(b.len())) else {
        return false;
    };
    // SAFETY: the UTF-16 buffers remain valid for the call and their explicit lengths keep the
    // API from reading beyond them. CompareStringOrdinal retains no pointers.
    unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) == CSTR_EQUAL }
}

fn is_path_separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

fn has_path_separator(path: &str) -> bool {
    path.chars().any(is_path_separator)
}

fn trim(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_path_separator);
    if trimmed.is_empty() && path.chars().next().is_some_and(is_path_separator) {
        return &path[..1];
    }
    // Keep a Windows drive root absolute. Trimming `C:\\` to `C:` changes it into a
    // drive-relative path with different semantics.
    #[cfg(windows)]
    if trimmed.len() == 2
        && trimmed.as_bytes()[1] == b':'
        && path
            .as_bytes()
            .get(2)
            .is_some_and(|byte| matches!(byte, b'/' | b'\\'))
    {
        return path;
    }
    trimmed
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn path_identity_cache_preserves_root_and_refreshes_per_query() {
        let root = std::env::temp_dir().join(format!(
            "tsk-path-identities-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let first = root.join("first");
        let second = root.join("second");
        let alias = root.join("alias");
        fs::create_dir_all(&first).expect("first directory");
        fs::create_dir(&second).expect("second directory");
        let root_alias = root.join("root-alias");
        symlink("/", &root_alias).expect("root alias");
        symlink(&first, &alias).expect("first alias");

        let cache = PathIdentityCache::default();
        assert!(cache.equivalent("/", &root_alias.to_string_lossy()));
        assert!(!cache.equivalent(
            &root.join("missing").to_string_lossy(),
            &first.to_string_lossy()
        ));
        assert!(cache.equivalent(&alias.to_string_lossy(), &first.to_string_lossy()));

        fs::remove_file(&alias).expect("remove old alias");
        symlink(&second, &alias).expect("second alias");
        assert!(
            !cache.equivalent(&alias.to_string_lossy(), &second.to_string_lossy()),
            "one query keeps its original identity snapshot"
        );
        assert!(
            PathIdentityCache::default()
                .equivalent(&alias.to_string_lossy(), &second.to_string_lossy()),
            "a fresh query must observe the new alias target"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bare_project_names_resolve_once_or_refuse_unknown_and_ambiguous() {
        let mut domain = DomainState::new();
        for path in ["/a/Atlas", "/b/atlas"] {
            domain
                .create(
                    "fixture",
                    None,
                    TaskScope::Project { path: path.into() },
                    crate::domain::ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create project fixture");
        }

        assert_eq!(
            resolve_project_path("missing", &domain, None),
            Err(ProjectResolveError::Unknown)
        );
        assert_eq!(
            resolve_project_path("ATLAS", &domain, None),
            Err(ProjectResolveError::Ambiguous(vec![
                "/a/Atlas".into(),
                "/b/atlas".into()
            ]))
        );
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: Some(PathBuf::from("/0/atlas")),
            title_prefill: None,
            provenance: crate::domain::ProvenanceOrigin::Capture,
        };
        assert_eq!(
            resolve_project_path("atlas", &domain, Some(&snapshot)),
            Err(ProjectResolveError::Ambiguous(vec![
                "/0/atlas".into(),
                "/a/Atlas".into(),
                "/b/atlas".into()
            ])),
            "ambiguous candidates stay sorted across stored and invocation sources"
        );
    }

    #[test]
    fn path_tokens_require_an_absolute_existing_directory() {
        let root = std::env::temp_dir().join(format!(
            "tsk-project-resolution-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("project");
        fs::create_dir_all(&project).expect("project directory");
        let missing = root.join("missing").to_string_lossy().into_owned();
        let domain = DomainState::new();

        assert_eq!(
            resolve_project_path("relative/project", &domain, None),
            Err(ProjectResolveError::NoDirectory("relative/project".into()))
        );
        assert_eq!(
            resolve_project_path(&missing, &domain, None),
            Err(ProjectResolveError::NoDirectory(missing))
        );
        assert_eq!(
            resolve_project_path(&project.to_string_lossy(), &domain, None),
            Ok(project.to_string_lossy().into_owned())
        );
        assert_eq!(
            expand_home_from("~/repo", Some(std::ffi::OsStr::new("/home/example"))),
            "/home/example/repo"
        );
        assert_eq!(
            expand_home_from("~//repo", Some(std::ffi::OsStr::new("/home/example"))),
            "/home/example/repo",
            "redundant separators must not replace the HOME prefix"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn equivalent_absolute_path_keeps_the_stored_project_spelling() {
        let root = std::env::temp_dir().join(format!(
            "tsk-project-alias-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("project");
        let stored_alias = root.join("z-alias");
        let invocation_alias = root.join("a-alias");
        fs::create_dir_all(&project).expect("project directory");
        symlink(&project, &stored_alias).expect("stored project alias");
        symlink(&project, &invocation_alias).expect("invocation project alias");
        let stored = stored_alias.to_string_lossy().into_owned();
        let invocation = invocation_alias.to_string_lossy().into_owned();
        let mut domain = DomainState::new();
        domain
            .create(
                "stored fixture",
                None,
                TaskScope::Project {
                    path: stored.clone(),
                },
                crate::domain::ProvenanceOrigin::Manual,
                None,
            )
            .expect("create stored project fixture");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: Some(invocation_alias),
            title_prefill: None,
            provenance: crate::domain::ProvenanceOrigin::Capture,
        };

        assert_eq!(
            resolve_project_path(&project.to_string_lossy(), &domain, Some(&snapshot)),
            Ok(stored.clone()),
            "a stored identity wins over an equivalent invocation candidate"
        );

        domain
            .create(
                "second stored fixture",
                None,
                TaskScope::Project { path: invocation },
                crate::domain::ProvenanceOrigin::Manual,
                None,
            )
            .expect("create second stored project fixture");
        assert_eq!(
            resolve_project_path(&stored, &domain, Some(&snapshot)),
            Ok(stored),
            "an exact stored spelling wins among equivalent stored aliases"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bare_project_name_dedupes_equivalent_stored_and_invocation_aliases() {
        let root = std::env::temp_dir().join(format!(
            "tsk-project-bare-alias-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("project");
        let stored_parent = root.join("stored");
        let invocation_parent = root.join("invocation");
        fs::create_dir_all(&project).expect("project directory");
        fs::create_dir(&stored_parent).expect("stored parent");
        fs::create_dir(&invocation_parent).expect("invocation parent");
        let stored_alias = stored_parent.join("widget");
        let invocation_alias = invocation_parent.join("widget");
        symlink(&project, &stored_alias).expect("stored project alias");
        symlink(&project, &invocation_alias).expect("invocation project alias");
        let stored = stored_alias.to_string_lossy().into_owned();
        let mut domain = DomainState::new();
        domain
            .create(
                "stored fixture",
                None,
                TaskScope::Project {
                    path: stored.clone(),
                },
                crate::domain::ProvenanceOrigin::Manual,
                None,
            )
            .expect("create stored project fixture");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: Some(invocation_alias),
            title_prefill: None,
            provenance: crate::domain::ProvenanceOrigin::Capture,
        };

        assert_eq!(
            resolve_project_path("widget", &domain, Some(&snapshot)),
            Ok(stored),
            "equivalent aliases are one project identity"
        );
        let _ = fs::remove_dir_all(root);
    }
}

/// Why a user-supplied project token cannot identify a capture destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectResolveError {
    /// A bare basename matches no known project.
    Unknown,
    /// A bare basename matches more than one known project, in sorted path order.
    Ambiguous(Vec<String>),
    /// A path token is relative or does not name an existing directory.
    NoDirectory(String),
}

impl ProjectResolveError {
    /// Human-facing refusal text. CLI callers add the stable `unknown-project` code.
    pub fn message(&self, token: &str) -> String {
        match self {
            Self::Unknown => format!("project {token} is not on the board"),
            Self::Ambiguous(paths) => {
                format!("project {token} is ambiguous: {}", paths.join(", "))
            }
            Self::NoDirectory(path) => format!("no directory at {path}"),
        }
    }
}

/// Resolve command-line project/global scope flags with the same default and basename
/// rules used by headless add.
pub fn resolve_flag_scope(
    project: Option<&str>,
    global: bool,
    domain: &DomainState,
    snapshot: &InvocationSnapshot,
) -> Result<TaskScope, ProjectResolveError> {
    match project {
        Some(project) => resolve_project_path(project, domain, Some(snapshot))
            .map(|path| TaskScope::Project { path }),
        None if global => Ok(TaskScope::Global),
        None => Ok(snapshot.default_scope.clone()),
    }
}

/// Resolve a project token against task, registered-project, and invocation paths.
///
/// Bare tokens must have one ASCII-case-insensitive basename match. Path tokens must be
/// absolute existing directories; `~/…` expands through `HOME`. An equivalent stored path
/// wins over a new spelling so capture never rewrites an existing project identity.
pub fn resolve_project_path(
    token: &str,
    domain: &DomainState,
    snapshot: Option<&InvocationSnapshot>,
) -> Result<String, ProjectResolveError> {
    let candidates = project_candidates(domain, snapshot);
    if has_path_separator(token) || token == "~" {
        let expanded = expand_home(token);
        let path = Path::new(&expanded);
        if !path.is_absolute() || !path.is_dir() {
            return Err(ProjectResolveError::NoDirectory(expanded));
        }
        return Ok(candidates
            .preferred_equivalent(&expanded)
            .unwrap_or(expanded));
    }

    let matches = candidates.basename_matches(token);
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(ProjectResolveError::Unknown),
        _ => Err(ProjectResolveError::Ambiguous(matches)),
    }
}

/// Resolve with the pre-T82 permissive rules used by read and project archive verbs.
/// Slash paths stay literal, and missing or ambiguous bare names stay literal.
pub(crate) fn resolve_permissive_project_path(
    token: &str,
    domain: &DomainState,
    snapshot: Option<&InvocationSnapshot>,
) -> String {
    if has_path_separator(token) {
        return token.to_string();
    }
    let mut candidates = stored_project_paths(domain);
    if let Some(snapshot) = snapshot {
        // Outside Git, the invocation directory was not historically a basename alias.
        if let TaskScope::Project { path } = &snapshot.default_scope {
            candidates.insert(path.clone());
            if let Some(this_repo) = snapshot.this_repo.as_deref() {
                candidates.insert(this_repo.to_string_lossy().into_owned());
            }
        }
    }
    let mut matches = candidates
        .into_iter()
        .filter(|path| basename_matches(path, token));
    match (matches.next(), matches.next()) {
        (Some(path), None) => path,
        _ => token.to_string(),
    }
}

struct ProjectCandidates {
    stored: BTreeSet<String>,
    invocation: BTreeSet<String>,
}

impl ProjectCandidates {
    fn preferred_equivalent(&self, path: &str) -> Option<String> {
        // Existing persisted identity wins over transient invocation spellings. Within each
        // source, honor an exact spelling before considering canonical aliases.
        for candidates in [&self.stored, &self.invocation] {
            if let Some(exact) = candidates
                .iter()
                .find(|candidate| trim(candidate) == trim(path))
            {
                return Some(exact.clone());
            }
            if let Some(equivalent) = candidates
                .iter()
                .find(|candidate| paths_equivalent(candidate, path))
            {
                return Some(equivalent.clone());
            }
        }
        None
    }

    fn basename_matches(&self, token: &str) -> Vec<String> {
        let mut matches = Vec::<String>::new();
        for candidate in self.stored.iter().chain(&self.invocation) {
            if basename_matches(candidate, token)
                && !matches
                    .iter()
                    .any(|matched| paths_equivalent(matched, candidate))
            {
                matches.push(candidate.clone());
            }
        }
        matches.sort();
        matches
    }
}

fn project_candidates(
    domain: &DomainState,
    snapshot: Option<&InvocationSnapshot>,
) -> ProjectCandidates {
    let stored = stored_project_paths(domain);
    let mut invocation = BTreeSet::new();
    if let Some(snapshot) = snapshot {
        if let TaskScope::Project { path } = &snapshot.default_scope {
            invocation.insert(path.clone());
        }
        if let Some(this_repo) = snapshot.this_repo.as_deref() {
            invocation.insert(this_repo.to_string_lossy().into_owned());
        }
    }
    ProjectCandidates { stored, invocation }
}

fn stored_project_paths(domain: &DomainState) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for task in domain.tasks() {
        if let TaskScope::Project { path } = &task.scope {
            paths.insert(path.clone());
        }
    }
    // Archived projects keep resolvable names even when every task of theirs is hidden.
    paths.extend(domain.projects().keys().cloned());
    paths
}

fn basename_matches(path: &str, token: &str) -> bool {
    path.trim_end_matches(is_path_separator)
        .rsplit(is_path_separator)
        .find(|component| !component.is_empty())
        .is_some_and(|basename| basename.eq_ignore_ascii_case(token))
}

fn expand_home(token: &str) -> String {
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()));
    expand_home_from(token, home.as_deref())
}

fn expand_home_from(token: &str, home: Option<&std::ffi::OsStr>) -> String {
    let remainder = if token == "~" {
        Some("")
    } else {
        token.strip_prefix("~/")
    };
    #[cfg(windows)]
    let remainder = remainder.or_else(|| token.strip_prefix("~\\"));
    let Some(remainder) = remainder else {
        return token.to_string();
    };
    let Some(home) = home else {
        return token.to_string();
    };
    let mut expanded = PathBuf::from(home);
    let remainder = remainder.trim_start_matches(is_path_separator);
    if !remainder.is_empty() {
        expanded.push(remainder);
    }
    expanded.to_string_lossy().into_owned()
}

#[cfg(test)]
mod portable_path_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_home_tokens_expand_with_either_separator() {
        let home = std::ffi::OsStr::new(r"C:\Users\example");
        assert_eq!(
            expand_home_from(r"~\code\app", Some(home)),
            PathBuf::from(home).join(r"code\app").to_string_lossy()
        );
        assert_eq!(
            expand_home_from("~/code/app", Some(home)),
            PathBuf::from(home).join("code/app").to_string_lossy()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_root_is_not_changed_to_a_drive_relative_path() {
        assert_eq!(trim(r"C:\"), r"C:\");
        assert!(basename_matches(r"C:\work\tsk\", "tsk"));
    }

    #[cfg(windows)]
    #[test]
    fn missing_windows_paths_compare_case_insensitively() {
        assert!(paths_equivalent(
            r"C:\Missing\RéPo\App",
            r"c:\missing\répo\app"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn permissive_resolution_keeps_a_windows_path_literal() {
        let domain = DomainState::new();
        assert_eq!(
            resolve_permissive_project_path(r"C:\work\tsk", &domain, None),
            r"C:\work\tsk"
        );
    }

    #[cfg(unix)]
    #[test]
    fn backslash_remains_a_literal_unix_filename_character() {
        assert_eq!(trim(r"/tmp/foo\"), r"/tmp/foo\");
        assert!(basename_matches(r"/tmp/foo\bar", r"foo\bar"));
        assert!(!has_path_separator(r"foo\bar"));
    }
}
