//! Optional GitHub release check: a cached tag and a dim footer notice.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

/// A full board's release check: the cached notice at open, then one background check
/// whenever the last one is a day old. The fetch thread only writes the cache and flags the
/// landing; the board's frame tick picks the notice up, so the UI thread never waits on it.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    dir: PathBuf,
    current: String,
    fetch: fn() -> Option<String>,
    /// The real release check honours `suppress_background_fetch`; a stub fetch always runs.
    live: bool,
    state: Arc<Mutex<CheckState>>,
}

#[derive(Debug, Default)]
struct CheckState {
    /// Unix time the cache is next worth reading: a day after the last check started.
    due_at: u64,
    in_flight: bool,
    landed: bool,
}

impl UpdateCheck {
    pub fn new(dir: &Path, current: &str) -> Self {
        Self::build(dir, current, fetch_latest, true)
    }

    /// The same check with the network call stood in for, for tests.
    pub fn with_fetch(dir: &Path, current: &str, fetch: fn() -> Option<String>) -> Self {
        Self::build(dir, current, fetch, false)
    }

    fn build(dir: &Path, current: &str, fetch: fn() -> Option<String>, live: bool) -> Self {
        Self {
            dir: dir.to_path_buf(),
            current: current.to_string(),
            fetch,
            live,
            state: Arc::default(),
        }
    }

    /// `Some` when the notice should be (re)set: a check landed, or the cache came due and
    /// was read again. Starts at most one background check, and only once a day has passed
    /// since the last one, here or in another tsk process sharing the cache.
    pub fn poll(&self, now: u64) -> Option<Option<String>> {
        if opt_out() {
            return None;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut notice = None;
        if std::mem::take(&mut state.landed) {
            notice = Some(self.cached_notice());
        }
        if state.in_flight || now < state.due_at {
            return notice;
        }
        let planned = plan(&self.dir, &self.current, now);
        if planned.should_fetch {
            mark_check_started(&self.dir, now);
            state.due_at = now.saturating_add(STALE_AFTER_SECS);
        } else {
            let last = read_cache(&self.dir).map_or(now, |cache| cache.last_check_unix);
            state.due_at = last.saturating_add(STALE_AFTER_SECS);
        }
        // A spawn that fails drops its `Landing` on this thread, which takes the lock.
        drop(state);
        if planned.should_fetch {
            self.spawn_fetch();
        }
        Some(planned.notice)
    }

    fn cached_notice(&self) -> Option<String> {
        read_cache(&self.dir).and_then(|cache| notice_for(&cache.latest, &self.current))
    }

    fn spawn_fetch(&self) {
        if self.live && (cfg!(test) || !BACKGROUND_FETCH.load(Ordering::SeqCst)) {
            return;
        }
        silence_update_check_panics();
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .in_flight = true;
        let dir = self.dir.clone();
        let fetch = self.fetch;
        let landing = Landing(Arc::clone(&self.state));
        let _ = std::thread::Builder::new()
            .name(UPDATE_CHECK_THREAD.to_string())
            .spawn(move || {
                let _landing = landing;
                apply_fetch(&dir, unix_now(), fetch);
            });
    }
}

/// Flags the check as landed when its thread ends, a panic included, so the board stops
/// waiting on it and reads the cache again.
struct Landing(Arc<Mutex<CheckState>>);

impl Drop for Landing {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.in_flight = false;
        state.landed = true;
    }
}

const UPDATE_CHECK_THREAD: &str = "tsk-update-check";

/// The check is best effort and runs beside the TUI: a panic on its thread must not print
/// over the board. Panics on every other thread still reach the previous hook.
fn silence_update_check_panics() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| quiet_panic_hook(UPDATE_CHECK_THREAD));
}

fn quiet_panic_hook(thread_name: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some(thread_name) {
            previous(info);
        }
    }));
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
    fn quiet_panic_hook_silences_only_the_named_thread() {
        use std::sync::atomic::AtomicUsize;
        static REPORTED: AtomicUsize = AtomicUsize::new(0);
        const QUIET: &str = "tsk-quiet-hook-probe";
        const LOUD: &str = "tsk-loud-hook-probe";

        let outer = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(LOUD) {
                REPORTED.fetch_add(1, Ordering::SeqCst);
            } else if std::thread::current().name() != Some(QUIET) {
                outer(info);
            }
        }));
        quiet_panic_hook(QUIET);

        let quiet = std::thread::Builder::new()
            .name(QUIET.to_string())
            .spawn(|| panic!("quiet probe"))
            .expect("spawn quiet probe");
        assert!(quiet.join().is_err());
        assert_eq!(REPORTED.load(Ordering::SeqCst), 0);
        let loud = std::thread::Builder::new()
            .name(LOUD.to_string())
            .spawn(|| panic!("loud probe"))
            .expect("spawn loud probe");
        assert!(loud.join().is_err());
        assert_eq!(REPORTED.load(Ordering::SeqCst), 1);
    }

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
        crate::test_stub::write_stub(
            &curl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '{{\"tag_name\": \"v9.9.9\"}}'\n",
                argv.display()
            ),
            0o755,
        );
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
