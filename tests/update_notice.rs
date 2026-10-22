//! Update-notice cache, opt-out, fetch failure, and footer paint.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::app::{board_frame, load_board_for_quick_capture, load_board_model};
use tsk_tui::domain::DomainState;
use tsk_tui::ui::queue::NavTab;
use tsk_tui::ui::{apply_intent, draw_board, BoardIntent, BoardModel};
use tsk_tui::update::{
    apply_fetch, is_stale, mark_check_started, notice_for, plan, read_cache,
    suppress_background_fetch, unix_now, write_cache, CheckPlan, UpdateCache, UpdateCheck,
    STALE_AFTER_SECS,
};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-update-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn seed_cache(dir: &Path, last_check_unix: u64, latest: &str) {
    write_cache(
        dir,
        &UpdateCache {
            last_check_unix,
            latest: latest.to_string(),
        },
    )
    .expect("write cache");
}

#[test]
fn twenty_four_hour_freshness_and_future_timestamp_are_eligible() {
    let _lock = env_lock();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("fresh");
    let now = 1_700_000_000u64;
    seed_cache(&dir, now - 60, "v9.0.0");
    let fresh = plan(&dir, "0.6.0", now);
    assert_eq!(
        fresh,
        CheckPlan {
            notice: Some("v9.0.0 available, run tsk update".into()),
            should_fetch: false,
        }
    );
    seed_cache(&dir, now - STALE_AFTER_SECS, "v9.0.0");
    let stale = plan(&dir, "0.6.0", now);
    assert!(stale.should_fetch);
    assert_eq!(
        stale.notice.as_deref(),
        Some("v9.0.0 available, run tsk update")
    );
    seed_cache(&dir, now + 3_600, "v9.0.0");
    let future = plan(&dir, "0.6.0", now);
    assert!(!future.should_fetch, "future last_check stays eligible");
    assert!(!is_stale(now + 3_600, now));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn opt_out_env_disables_notice_and_fetch() {
    let _lock = env_lock();
    let dir = temp_dir("opt-out");
    seed_cache(&dir, 1, "v9.0.0");
    std::env::set_var("TSK_NO_UPDATE_CHECK", "1");
    let planned = plan(&dir, "0.6.0", 1_700_000_000);
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    assert_eq!(
        planned,
        CheckPlan {
            notice: None,
            should_fetch: false,
        }
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn starting_a_check_stamps_last_check_and_keeps_the_cached_tag() {
    let _lock = env_lock();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("stamp");
    seed_cache(&dir, 1, "v9.0.0");
    mark_check_started(&dir, 99);
    assert_eq!(
        read_cache(&dir),
        Some(UpdateCache {
            last_check_unix: 99,
            latest: "v9.0.0".into(),
        })
    );
    let planned = plan(&dir, "0.6.0", 99);
    assert!(!planned.should_fetch);
    assert_eq!(
        planned.notice.as_deref(),
        Some("v9.0.0 available, run tsk update")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn curl_failure_leaves_cache_and_footer_untouched() {
    let _lock = env_lock();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("fail");
    let original = UpdateCache {
        last_check_unix: 42,
        latest: "v0.1.0".into(),
    };
    write_cache(&dir, &original).expect("seed");
    let before = fs::read_to_string(dir.join("update.json")).expect("read");
    apply_fetch(&dir, 99, || None);
    let after = fs::read_to_string(dir.join("update.json")).expect("reread");
    assert_eq!(after, before);
    assert_eq!(read_cache(&dir), Some(original));
    assert_eq!(notice_for("v0.1.0", "0.6.0"), None);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn footer_paints_dim_notice_from_a_seeded_cache_and_hides_for_a_message() {
    let mut model = BoardModel::from_domain(&DomainState::new(), None);
    let notice = notice_for("v9.9.9", "0.6.0").expect("newer");
    model.set_update_notice(Some(notice.clone()));

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let mut found = None;
    for y in 0..24 {
        let row: String = (0..80)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        if let Some(x) = row.find(&notice) {
            found = Some((x as u16, y));
            break;
        }
    }
    let (x, y) = found.expect("status row should paint the update notice");
    assert!(
        buffer[(x, y)].style().add_modifier.contains(Modifier::DIM),
        "notice must be dim"
    );
    assert!(
        !buffer[(x, y)].style().add_modifier.contains(Modifier::BOLD),
        "notice must not use the message style"
    );

    model.set_message("copied");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw message");
    let buffer = terminal.backend().buffer();
    let mut still = false;
    for y in 0..24 {
        let row: String = (0..80)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        if row.contains(&notice) {
            still = true;
        }
    }
    assert!(!still, "any other status message hides the update notice");

    let mut domain = DomainState::new();
    model.set_update_notice(Some(notice.clone()));
    model.clear_message();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).expect("open quick-add");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw quick-add");
    let buffer = terminal.backend().buffer();
    let mut on_input = false;
    for y in 0..24 {
        let row: String = (0..80)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        if row.contains(&notice) {
            on_input = true;
        }
    }
    assert!(!on_input, "a bottom input hides the update notice");

    apply_intent(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None)
        .expect("close quick-add");
    model.set_update_notice(Some(notice.clone()));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(NavTab::Projects),
        None,
    )
    .expect("open projects");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw projects");
    let buffer = terminal.backend().buffer();
    let mut on_projects = false;
    for y in 0..24 {
        let row: String = (0..80)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        if row.contains(&notice) {
            on_projects = true;
        }
    }
    assert!(
        on_projects,
        "Projects Overview idle status shows the update notice"
    );
}

#[test]
fn load_board_uses_seeded_update_json_in_state_dir() {
    let _lock = env_lock();
    suppress_background_fetch();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("load");
    seed_cache(&dir, unix_now(), "v9.9.9");
    let previous = std::env::var_os("TSK_STATE_DIR");
    std::env::set_var("TSK_STATE_DIR", &dir);
    let model = load_board_model().expect("load board");
    match previous {
        Some(value) => std::env::set_var("TSK_STATE_DIR", value),
        None => std::env::remove_var("TSK_STATE_DIR"),
    }
    assert_eq!(
        model.update_notice(),
        Some("v9.9.9 available, run tsk update")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn semver_notice_matches_public_compare() {
    let current = env!("CARGO_PKG_VERSION");
    assert_eq!(notice_for("v0.6.1", current), None);
    assert!(!tsk_tui::update::is_newer("v0.6.1", current));
    let newer = "v99.0.0";
    assert!(tsk_tui::update::is_newer(newer, current));
    assert_eq!(
        notice_for(newer, current).as_deref(),
        Some("v99.0.0 available, run tsk update")
    );
}

/// Polls `ready` until it holds or five seconds pass.
fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    ready()
}

fn with_state_dir<T>(dir: &Path, run: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os("TSK_STATE_DIR");
    std::env::set_var("TSK_STATE_DIR", dir);
    let result = run();
    match previous {
        Some(value) => std::env::set_var("TSK_STATE_DIR", value),
        None => std::env::remove_var("TSK_STATE_DIR"),
    }
    result
}

#[test]
fn a_finished_background_check_reaches_an_open_board_without_a_restart() {
    let _lock = env_lock();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("lands");
    seed_cache(&dir, unix_now() - STALE_AFTER_SECS, "v0.6.0");
    let mut model = BoardModel::from_domain(&DomainState::new(), None);
    model.set_update_check(Some(UpdateCheck::with_fetch(&dir, "0.6.0", || {
        Some("v99.0.0".into())
    })));
    let frame = |model: &mut BoardModel| {
        board_frame(model, |_| Ok(()), |_| Ok(false), false).expect("frame");
    };
    frame(&mut model);
    let landed = wait_until(|| {
        frame(&mut model);
        model.update_notice().is_some()
    });
    assert!(landed, "the open board never showed the finished check");
    assert_eq!(
        model.update_notice(),
        Some("v99.0.0 available, run tsk update")
    );
    let _ = fs::remove_dir_all(dir);
}

static DAILY_FETCHES: AtomicUsize = AtomicUsize::new(0);
static DAILY_RELEASE: AtomicBool = AtomicBool::new(false);

fn daily_fetch() -> Option<String> {
    DAILY_FETCHES.fetch_add(1, Ordering::SeqCst);
    while !DAILY_RELEASE.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(2));
    }
    Some("v99.0.0".into())
}

#[test]
fn an_open_board_checks_again_once_a_day_with_one_check_in_flight() {
    let _lock = env_lock();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("daily");
    let t = unix_now();
    seed_cache(&dir, t - STALE_AFTER_SECS, "v0.6.0");
    let check = UpdateCheck::with_fetch(&dir, "0.6.0", daily_fetch);
    let fetches = || DAILY_FETCHES.load(Ordering::SeqCst);

    assert_eq!(check.poll(t), Some(None), "open reads the cache");
    assert!(wait_until(|| fetches() == 1), "a stale open starts a check");
    assert_eq!(check.poll(t + 3 * STALE_AFTER_SECS), None);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fetches(), 1, "never a second check while one is in flight");

    DAILY_RELEASE.store(true, Ordering::SeqCst);
    let mut landed = None;
    assert!(wait_until(|| {
        landed = check.poll(t);
        landed.is_some()
    }));
    assert_eq!(
        landed,
        Some(Some("v99.0.0 available, run tsk update".into()))
    );
    assert_eq!(check.poll(t + STALE_AFTER_SECS - 1), None);
    assert_eq!(fetches(), 1, "no new check inside the day");

    check.poll(t + 3 * STALE_AFTER_SECS);
    assert!(
        wait_until(|| fetches() == 2),
        "a day later the open board checks again"
    );
    check.poll(t + 3 * STALE_AFTER_SECS);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fetches(), 2, "exactly one new check");
    let _ = fs::remove_dir_all(dir);
}

static OPT_OUT_FETCHES: AtomicUsize = AtomicUsize::new(0);

fn opt_out_fetch() -> Option<String> {
    OPT_OUT_FETCHES.fetch_add(1, Ordering::SeqCst);
    Some("v99.0.0".into())
}

#[test]
fn opt_out_suppresses_the_open_board_check_and_its_notice() {
    let _lock = env_lock();
    suppress_background_fetch();
    let dir = temp_dir("opt-out-open");
    let t = unix_now();
    seed_cache(&dir, 1, "v99.0.0");
    let before = fs::read_to_string(dir.join("update.json")).expect("read");
    std::env::set_var("TSK_NO_UPDATE_CHECK", "1");
    let check = UpdateCheck::with_fetch(&dir, "0.6.0", opt_out_fetch);
    let polls = [
        check.poll(t),
        check.poll(t + STALE_AFTER_SECS),
        check.poll(t + 2 * STALE_AFTER_SECS),
    ];
    let model = with_state_dir(&dir, || load_board_model().expect("load board"));
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(polls, [None, None, None]);
    assert_eq!(OPT_OUT_FETCHES.load(Ordering::SeqCst), 0);
    assert_eq!(model.update_notice(), None);
    let after = fs::read_to_string(dir.join("update.json")).expect("reread");
    assert_eq!(after, before, "opt-out never stamps a check");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn quick_capture_and_the_cli_never_check() {
    let _lock = env_lock();
    suppress_background_fetch();
    std::env::remove_var("TSK_NO_UPDATE_CHECK");
    let dir = temp_dir("capture");
    seed_cache(&dir, 1, "v99.0.0");
    let before = fs::read_to_string(dir.join("update.json")).expect("read");

    let (_store, _state, model) = with_state_dir(&dir, || {
        load_board_for_quick_capture().expect("capture open")
    });
    assert_eq!(model.update_notice(), None);
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["list", "--json"])
        .env("TSK_STATE_DIR", &dir)
        .env_remove("TSK_NO_UPDATE_CHECK")
        .output()
        .expect("run tsk list");
    assert!(output.status.success(), "{output:?}");

    let after = fs::read_to_string(dir.join("update.json")).expect("reread");
    assert_eq!(after, before, "a stale cache stays unstamped");
    let _ = fs::remove_dir_all(dir);
}
