//! Optional GitHub release check: a cached tag and a dim footer notice.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fsperm;

const UPDATE_FILE: &str = "update.json";
pub const UPDATE_TEMP_PREFIX: &str = ".update.json.tmp.";
pub const STALE_AFTER_SECS: u64 = 24 * 60 * 60;
const RELEASES_URL: &str = "https://api.github.com/repos/smarzban/tsk/releases/latest";
static BACKGROUND_FETCH: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCache {
    pub last_check_unix: u64,
    pub latest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckPlan {
    pub notice: Option<String>,
    pub should_fetch: bool,
}

pub fn opt_out() -> bool {
    std::env::var_os("TSK_NO_UPDATE_CHECK").is_some()
}

/// Integration tests link the library without `cfg(test)`, so `spawn_fetch` cannot
/// use that cfg. Tests that load the board call this before `load_board*`.
pub fn suppress_background_fetch() {
    BACKGROUND_FETCH.store(false, Ordering::SeqCst);
}

pub fn mark_check_started(dir: &Path, now: u64) {
    let latest = read_cache(dir)
        .map(|cache| cache.latest)
        .unwrap_or_default();
    let _ = write_cache(
        dir,
        &UpdateCache {
            last_check_unix: now,
            latest,
        },
    );
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn is_stale(last_check_unix: u64, now: u64) -> bool {
    now.saturating_sub(last_check_unix) >= STALE_AFTER_SECS
}

pub fn parse_semver(value: &str) -> Option<(u64, u64, u64)> {
    let trimmed = value.trim();
    let rest = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let core = rest.split(['-', '+']).next().unwrap_or(rest);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

pub fn notice_for(latest: &str, current: &str) -> Option<String> {
    if !is_newer(latest, current) {
        return None;
    }
    let body = latest.trim();
    let shown = if body.starts_with('v') || body.starts_with('V') {
        body.to_string()
    } else {
        format!("v{body}")
    };
    Some(format!("{shown} available, run tsk update"))
}

pub fn read_cache(dir: &Path) -> Option<UpdateCache> {
    let path = dir.join(UPDATE_FILE);
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn write_cache(dir: &Path, cache: &UpdateCache) -> io::Result<()> {
    fsperm::ensure_private_dir(dir)?;
    let document = dir.join(UPDATE_FILE);
    let tmp = unique_tmp_path(dir);
    let data = serde_json::to_string(cache).map_err(io::Error::other)?;
    let write_result = (|| -> io::Result<()> {
        let mut temp_file = fsperm::create_private_file(&tmp)?;
        temp_file.write_all(data.as_bytes())?;
        temp_file.sync_all()?;
        drop(temp_file);
        crate::fsperm::replace_file(&tmp, &document)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

pub fn plan(dir: &Path, current: &str, now: u64) -> CheckPlan {
    if opt_out() {
        return CheckPlan {
            notice: None,
            should_fetch: false,
        };
    }
    let cache = read_cache(dir);
    let notice = cache
        .as_ref()
        .and_then(|cache| notice_for(&cache.latest, current));
    let should_fetch = cache
        .as_ref()
        .map(|cache| is_stale(cache.last_check_unix, now))
        .unwrap_or(true);
    CheckPlan {
        notice,
        should_fetch,
    }
}

pub fn apply_fetch(dir: &Path, now: u64, fetch: impl FnOnce() -> Option<String>) {
    let Some(latest) = fetch() else {
        return;
    };
    if latest.trim().is_empty() {
        return;
    }
    let _ = write_cache(
        dir,
        &UpdateCache {
            last_check_unix: now,
            latest: latest.trim().to_string(),
        },
    );
}

pub fn startup(state_dir: &Path, current: &str) -> Option<String> {
    let now = unix_now();
    let planned = plan(state_dir, current, now);
    if planned.should_fetch {
        mark_check_started(state_dir, now);
        spawn_fetch(state_dir.to_path_buf());
    }
    planned.notice
}

fn spawn_fetch(dir: PathBuf) {
    if cfg!(test) || !BACKGROUND_FETCH.load(Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        apply_fetch(&dir, unix_now(), fetch_latest);
    });
}

#[cfg(unix)]
fn fetch_latest() -> Option<String> {
    let curl = crate::cli::update::curl_path().ok()?;
    fetch_latest_with(&curl)
}

#[cfg(windows)]
fn fetch_latest() -> Option<String> {
    let bytes = crate::cli::update::download_https(RELEASES_URL, 64 * 1024, 15).ok()?;
    parse_latest_release(&bytes)
}

fn parse_latest_release(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let tag = value.get("tag_name")?.as_str()?.trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

/// The Unix release check's one network call, with curl injected so a test can stand one
/// in: same hardened policy as `tsk update`, 15 s budget.
#[cfg(unix)]
pub fn fetch_latest_with(curl: &Path) -> Option<String> {
    let output = crate::cli::update::hardened_curl(curl, 15)
        .arg(RELEASES_URL)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_latest_release(&output.stdout)
}

fn unique_tmp_path(dir: &Path) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.join(format!("{UPDATE_TEMP_PREFIX}{pid}.{nanos}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare_orders_core_versions_and_v_prefix() {
        assert!(is_newer("v0.6.1", "0.6.0"));
        assert!(is_newer("0.7.0", "0.6.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.6.0", "0.6.0"));
        assert!(!is_newer("v0.6.0", "0.6.0"));
        assert!(!is_newer("0.5.9", "0.6.0"));
        assert!(!is_newer("not-a-version", "0.6.0"));
        assert!(!is_newer("0.6.0", "nope"));
        assert_eq!(
            notice_for("v0.7.0", "0.6.0").as_deref(),
            Some("v0.7.0 available, run tsk update")
        );
        assert_eq!(
            notice_for("0.7.0", "0.6.0").as_deref(),
            Some("v0.7.0 available, run tsk update")
        );
        assert_eq!(notice_for("0.6.0", "0.6.0"), None);
    }

    #[test]
    fn cache_is_stale_after_24h_and_future_timestamp_is_fresh() {
        let now = 1_700_000_000;
        assert!(!is_stale(now, now));
        assert!(!is_stale(now - STALE_AFTER_SECS + 1, now));
        assert!(is_stale(now - STALE_AFTER_SECS, now));
        assert!(is_stale(now - STALE_AFTER_SECS - 1, now));
        assert!(
            !is_stale(now + 60, now),
            "a future last_check stays eligible"
        );
    }

    #[cfg(unix)]
    #[test]
    fn release_check_uses_the_shared_hardened_curl_policy() {
        let dir = std::env::temp_dir().join(format!(
            "tsk-release-check-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let argv = dir.join("argv");
        let curl = dir.join("curl");
        std::fs::write(
            &curl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '{{\"tag_name\": \"v9.9.9\"}}'\n",
                argv.display()
            ),
        )
        .expect("fake curl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        assert_eq!(fetch_latest_with(&curl), Some("v9.9.9".to_string()));
        let argv = std::fs::read_to_string(&argv).expect("argv");
        let args: Vec<&str> = argv.lines().collect();
        assert_eq!(
            args,
            vec![
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                "--max-time",
                "15",
                RELEASES_URL
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
