//! Performance harness for the frame scheduler: cold start to first frame, and
//! keypress-to-repaint latency during navigation, both at the 80x24 standard tier with a
//! 100-task fixture.
//!
//! Both benches drive the real render path -- `BoardModel::from_domain` then
//! `draw_board` through a `ratatui::backend::TestBackend` -- so a regression in either
//! the model build or the renderer's hot path shows up here, not a warmed cache or a
//! no-op. Navigation timing covers selection-move + redraw only: `BoardIntent::SelectNext`
//! is not in `board_intent_may_persist`, so `apply_intent` never reaches Task Store on this
//! path (no store I/O is timed).
//!
//! These are timing tests on a shared host, so each one warms up first and reports every
//! measured sample; the assertions read the design's own numbers (worst-of-N cold starts,
//! p99 of many navigation frames), not a mean standing in for either.
//!
//! Both are `#[ignore]`d by default: an unoptimized debug build is not the profile
//! these bounds describe (a debug build of `keypress_to_repaint_...` measures a ~22ms p99 against
//! the 16ms bound -- noise from the missing optimizer, not a renderer regression -- observed
//! at 22.22ms locally and 22ms in the build report), so leaving them unignored would make the
//! ordinary debug `cargo test` green bar flaky for a reason this task has no business fixing.
//! Run them explicitly, in release, to get the numbers these bounds check:
//!
//! cargo test --release --test queue_board_bench -- --ignored --test-threads=1 --nocapture
//!
//! Released p99/cold-start numbers vary run to run on a shared host: across five release runs
//! measured for fix round 1, nav p99 ranged ~3.9-11.3ms and cold start worst ranged
//! ~2.6-19ms (both comfortably under bound, but the margin against the 16ms p99 bound is closer
//! to ~1.4x on a noisy run than a single lucky run suggests). Report a range across several
//! release runs, not one sample, when citing this bench as evidence.
//!
//! The 100-task fixture is a 100-task *model* fixture, not a 100-row *paint*: the done-drawer
//! defaults closed and one fifth of the fixture is `Done`, so `queue::query` still filters and
//! sorts all 100 tasks (the model-side cost measured here is honest) but only ~80 rows ever
//! reach `draw_board`'s viewport. Do not read the assertions below as having painted 100 rows.
//!
//! Scope note: this bench times `apply_intent` +
//! `draw_board` only, which is the contract's own stated boundary for this task -- not the
//! full `map_key` -> `resolve_board_surface` -> `board_intent_for_area` -> `resolve_board_command`
//! -> `handle_board_intent` -> `board_frame` path a real keypress runs. Treat this bench as a
//! lower bound on keypress-to-repaint latency, not a measurement of the whole path.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::{apply_intent, draw_board, BoardIntent, BoardModel};

const THIS_REPO: &str = "/repos/tsk";
const TASK_COUNT: usize = 100;
const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// A 100-task fixture spanning every status and several projects (plus Global), so the
/// queue query does the same section/grouping work a real board this size would.
fn build_domain_with_n_tasks(n: usize) -> DomainState {
    let statuses = [
        HumanStatus::Ready,
        HumanStatus::Started,
        HumanStatus::Blocked,
        HumanStatus::Review,
        HumanStatus::Done,
    ];
    let projects = [
        THIS_REPO.to_string(),
        "/repos/herdr".to_string(),
        "/repos/alpha".to_string(),
        "/repos/beta".to_string(),
    ];

    let mut domain = DomainState::new();
    for i in 0..n {
        let scope = if i % 11 == 0 {
            TaskScope::Global
        } else {
            TaskScope::Project {
                path: projects[i % projects.len()].clone(),
            }
        };
        let id = domain
            .create(
                format!("bench task {i}"),
                None,
                scope,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create bench fixture task");
        let status = statuses[i % statuses.len()];
        if status != HumanStatus::Ready {
            domain
                .set_status(id, status)
                .expect("set bench fixture status");
        }
    }
    domain
}

/// Every symbol painted on the backend's buffer, flattened into one string, for the
/// Imp-4 fixture/section-header assertions -- concatenation is enough since these
/// assertions only need `contains`, not row-aware layout.
fn buffer_plain(backend: &TestBackend) -> String {
    backend
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// The p99 of a sample: sorts ascending and reads the sample at the 99th percentile index
/// (nearest-rank method), clamped to the last index for small samples.
fn p99(samples: &mut [Duration]) -> Duration {
    assert!(!samples.is_empty(), "p99 needs at least one sample");
    samples.sort();
    let rank = ((samples.len() as f64) * 0.99).ceil() as usize;
    let index = rank.saturating_sub(1).min(samples.len() - 1);
    samples[index]
}

/// Cold start to first frame: `BoardModel::from_domain` + one `draw_board` through a fresh
/// `TestBackend`, at 80x24 with 100 tasks. The 100-task domain is built *before* the
/// timer starts each trial -- fixture construction is test setup, not part of the app's own
/// cold-start path (that path picks up an already-loaded `DomainState` from Task Store).
///
/// Reports the worst of several independent cold trials rather than a single sample, since a
/// lone measurement on a shared host is noisy in either direction; the bound is on the worst
/// case, not the mean.
#[test]
#[ignore = "timing bench: run in release, see module docs"]
fn cold_start_to_first_frame_under_200ms_at_80x24_with_100_tasks() {
    const TRIALS: usize = 9;
    const BOUND: Duration = Duration::from_millis(200);

    let mut elapsed_by_trial = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        let domain = build_domain_with_n_tasks(TASK_COUNT);

        let start = Instant::now();
        let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
        let mut terminal =
            Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("scratch terminal");
        let completed = terminal
            .draw(|frame| {
                let _ = draw_board(frame, &model);
            })
            .expect("first frame draw");
        std::hint::black_box(&completed);
        elapsed_by_trial.push(start.elapsed());

        // Outside the timed region: a regression that empties the list (selection
        // lost, query returning no sections, a viewport bug) would otherwise make this bench
        // *faster* and still green, while shipping as evidence the board paints fast.
        let plain = buffer_plain(terminal.backend());
        assert!(
            plain.contains("bench task"),
            "first frame must paint fixture tasks: {plain}"
        );
        assert!(
            plain.contains("IN MOTION"),
            "first frame must paint a section header: {plain}"
        );
    }

    let worst = *elapsed_by_trial.iter().max().expect("at least one trial");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[bench] cold_start_to_first_frame ({profile} profile): trials={elapsed_by_trial:?} worst={worst:?} bound={BOUND:?}"
    );

    assert!(
        worst < BOUND,
        "cold start to first frame must stay under {BOUND:?} at 80x24 with {TASK_COUNT} tasks; \
         worst of {TRIALS} trials was {worst:?} ({profile} profile): {elapsed_by_trial:?}"
    );
}

/// Keypress-to-repaint p99 during navigation: one `BoardIntent::SelectNext` through
/// `apply_intent`, then one `draw_board`, at 80x24 with 100 tasks. `SelectNext` wraps
/// modulo, so a 500-frame run sweeps every listed row several times over and covers the wrap
/// branch too. Selection moves are not in `board_intent_may_persist`, so this measures
/// selection-move + redraw only -- no Task Store I/O is on this path.
///
/// A short warmup run is discarded before the measured run, and the measured run is large
/// enough (500 frames) for its top 1% to be a handful of samples rather than one noisy
/// outlier standing in for a whole percentile.
#[test]
#[ignore = "timing bench: run in release, see module docs"]
fn keypress_to_repaint_p99_under_16ms_during_navigation_at_80x24_with_100_tasks() {
    const WARMUP_FRAMES: usize = 50;
    const MEASURED_FRAMES: usize = 500;
    const BOUND: Duration = Duration::from_millis(16);

    let mut domain = build_domain_with_n_tasks(TASK_COUNT);
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("scratch terminal");

    // BoardIntent::SelectNext unconditionally: select_next wraps modulo, so a
    // 500-frame run sweeps every listed row several times over -- and hits the wrap branch
    // for free -- at the same per-frame cost. Alternating SelectNext/SelectPrev instead
    // ping-pongs between the first two IN MOTION rows for the whole run: 78 of 80 listed rows
    // would never be selected and any position-dependent cost in selected_index()'s linear
    // scan would never be timed.
    let mut step = |domain: &mut DomainState, model: &mut BoardModel| {
        apply_intent(domain, model, BoardIntent::SelectNext, None).expect("navigation intent");
        let completed = terminal
            .draw(|frame| {
                let _ = draw_board(frame, model);
            })
            .expect("navigation frame draw");
        std::hint::black_box(&completed);
    };

    for _ in 0..WARMUP_FRAMES {
        step(&mut domain, &mut model);
    }

    let mut samples = Vec::with_capacity(MEASURED_FRAMES);
    for _ in 0..MEASURED_FRAMES {
        let start = Instant::now();
        step(&mut domain, &mut model);
        samples.push(start.elapsed());
    }

    // Outside the timed region: confirm the run actually painted fixture rows and a
    // section header rather than an empty list that would make this bench faster and still
    // green.
    let plain = buffer_plain(terminal.backend());
    assert!(
        plain.contains("bench task"),
        "navigation frames must paint fixture tasks: {plain}"
    );
    // Scroll-aware since G-2: the viewport now follows the selection, so navigating to the end
    // of a 100-task deck legitimately scrolls the IN MOTION header off the top. Assert that *a*
    // section header is painted, not that a specific one is -- pinning `IN MOTION` here asserted
    // the absence of follow-selection scrolling, which is the very defect G-2 fixed.
    let has_section = ["IN MOTION", "ON DECK", "DONE"]
        .iter()
        .any(|header| plain.contains(header))
        || ["tsk", "herdr", "alpha", "beta", "global"]
            .iter()
            .any(|name| plain.contains(&format!("{name} ─")));
    assert!(
        has_section,
        "navigation frames must paint a section header: {plain}"
    );

    let mut for_stats = samples.clone();
    let measured_p99 = p99(&mut for_stats);
    // `worst` is reported but deliberately not asserted: p99 is the statistic
    // this bench checks, and across four release runs the observed worst single frame breached the
    // 16ms bound in three of them while p99 stayed under it -- asserting on `worst` here
    // would make this suite permanently flaky on a shared host, not catch a real regression.
    let worst = *samples.iter().max().expect("at least one sample");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[bench] keypress_to_repaint ({profile} profile): frames={MEASURED_FRAMES} p99={measured_p99:?} worst={worst:?} bound={BOUND:?}"
    );

    assert!(
        measured_p99 < BOUND,
        "keypress-to-repaint p99 during navigation must stay under {BOUND:?} at 80x24 with \
         {TASK_COUNT} tasks; measured p99={measured_p99:?} (worst={worst:?}) over \
         {MEASURED_FRAMES} frames ({profile} profile)"
    );
}
