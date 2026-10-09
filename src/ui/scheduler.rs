//! Frame Scheduler: adaptive input-wait duration.

use std::time::Duration;

/// Default idle poll when no base is supplied by the caller.
pub const DEFAULT_BASE_TICK: Duration = Duration::from_millis(250);

/// Short-tick floor while an animation is active (never the idle path).
const SHORT_TICK_FLOOR: Duration = Duration::from_millis(25);

/// Prototype-like animation frame interval (~30 Hz, within 16–33 ms).
const SHORT_TICK: Duration = Duration::from_millis(33);

/// Quiet window after a terminal resize before the next forced layout paint.
///
/// Dragging a pane edge fires many `Event::Resize` reports; waiting this long
/// after the latest one lets the size settle so the board rebuilds once.
pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(50);

/// Hard cap on one coalesce call so a continuous resize stream cannot freeze the loop.
/// A long pane-edge drag still paints about once per idle tick.
pub const RESIZE_COALESCE_MAX: Duration = Duration::from_millis(250);

/// Next input-wait duration for one frame-loop cycle.
///
/// - Idle (`active_animations == false`): at least `base` and
///   [`DEFAULT_BASE_TICK`].
/// - Animating: the shorter of `base` or the short tick, floored at 25 ms.
pub fn next_wait(active_animations: bool, base: Duration) -> Duration {
    if active_animations {
        base.min(SHORT_TICK).max(SHORT_TICK_FLOOR)
    } else {
        base.max(DEFAULT_BASE_TICK)
    }
}

/// Drain a burst of resize events, returning the first real input (if any).
///
/// `poll` / `read` are injected so the policy is unit-testable without a tty.
/// Each resize resets the quiet window. Mouse motion/drag is noise from the
/// pane-edge pointer and does not reset or abort. Keys, clicks, wheel, and
/// paste interrupt and are returned so the caller can paint then dispatch.
/// The call always returns within [`RESIZE_COALESCE_MAX`].
pub fn coalesce_resizes<E>(
    poll: impl FnMut(Duration) -> Result<bool, E>,
    read: impl FnMut() -> Result<crossterm::event::Event, E>,
) -> Result<Option<crossterm::event::Event>, E> {
    coalesce_resizes_at(poll, read, std::time::Instant::now)
}

fn resize_burst_kind(event: &crossterm::event::Event) -> BurstKind {
    use crossterm::event::{Event, MouseEventKind};
    match event {
        Event::Resize(_, _) => BurstKind::Resize,
        Event::Mouse(mouse)
            if matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) =>
        {
            BurstKind::Noise
        }
        _ => BurstKind::Interrupt,
    }
}

enum BurstKind {
    Resize,
    Noise,
    Interrupt,
}

/// Same as [`coalesce_resizes`] with an injected clock for tests.
pub fn coalesce_resizes_at<E>(
    mut poll: impl FnMut(Duration) -> Result<bool, E>,
    mut read: impl FnMut() -> Result<crossterm::event::Event, E>,
    mut now: impl FnMut() -> std::time::Instant,
) -> Result<Option<crossterm::event::Event>, E> {
    let start = now();
    let mut quiet = start + RESIZE_DEBOUNCE;
    let hard = start + RESIZE_COALESCE_MAX;
    loop {
        let t = now();
        if t >= hard || t >= quiet {
            return Ok(None);
        }
        let wait = quiet.min(hard).saturating_duration_since(t);
        if !poll(wait)? {
            return Ok(None);
        }
        let event = read()?;
        match resize_burst_kind(&event) {
            BurstKind::Resize => {
                quiet = now() + RESIZE_DEBOUNCE;
            }
            BurstKind::Noise => {}
            BurstKind::Interrupt => return Ok(Some(event)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        coalesce_resizes, coalesce_resizes_at, next_wait, DEFAULT_BASE_TICK, RESIZE_COALESCE_MAX,
        RESIZE_DEBOUNCE,
    };
    use crossterm::event::Event;
    use std::time::Duration;

    #[test]
    fn idle_wait_is_at_least_250ms_with_no_active_animation() {
        let wait = next_wait(false, DEFAULT_BASE_TICK);
        assert!(
            wait >= Duration::from_millis(250),
            "idle wait must be ≥250 ms, got {wait:?}"
        );
        assert_eq!(wait, DEFAULT_BASE_TICK);

        let sub_floor_base = Duration::from_millis(5);
        assert_eq!(next_wait(false, sub_floor_base), DEFAULT_BASE_TICK);
    }

    #[test]
    fn active_animation_shortens_wait_below_base_tick_and_never_below_25ms_when_idle_returns() {
        let base = DEFAULT_BASE_TICK;
        let animating = next_wait(true, base);
        assert!(
            animating < base,
            "active animation must shorten below base, got {animating:?} vs base {base:?}"
        );
        assert!(
            animating >= Duration::from_millis(25),
            "short tick must not drop below 25 ms floor, got {animating:?}"
        );
        // Prototype-like cadence sits in the ~16–33 ms band after the floor.
        assert!(
            animating <= Duration::from_millis(33),
            "short tick should stay ≤33 ms, got {animating:?}"
        );

        let tiny_base = Duration::from_millis(20);
        assert_eq!(next_wait(true, tiny_base), Duration::from_millis(25));

        let idle_again = next_wait(false, base);
        assert!(
            idle_again >= Duration::from_millis(250),
            "returning to idle must restore ≥250 ms, got {idle_again:?}"
        );
    }

    #[test]
    fn no_sustained_sub_25ms_wait_while_animation_set_empty() {
        for base in [
            Duration::from_millis(5),
            Duration::from_millis(25),
            DEFAULT_BASE_TICK,
            Duration::from_millis(500),
        ] {
            let wait = next_wait(false, base);
            assert!(
                wait >= Duration::from_millis(25),
                "empty animation set produced sub-25 ms wait: {wait:?}"
            );
            assert!(
                wait >= Duration::from_millis(250),
                "empty animation set must stay at least 250 ms, got {wait:?}"
            );
            assert!(
                wait >= base,
                "idle wait must preserve base {base:?}, got {wait:?}"
            );
        }
    }

    fn mouse_moved() -> Event {
        use crossterm::event::{MouseEvent, MouseEventKind};
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 10,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    }

    fn key_j() -> Event {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
    }

    #[test]
    fn coalesce_resizes_returns_none_when_only_resizes_arrive() {
        use std::cell::Cell;
        let sizes = [(80u16, 24u16), (90, 30), (100, 40)];
        let idx = Cell::new(0usize);
        let out = coalesce_resizes(
            |_| Ok::<_, ()>(idx.get() < sizes.len()),
            || {
                let i = idx.get();
                let (w, h) = sizes[i];
                idx.set(i + 1);
                Ok(Event::Resize(w, h))
            },
        )
        .expect("coalesce");
        assert!(out.is_none());
        assert!(RESIZE_DEBOUNCE >= Duration::from_millis(16));
    }

    #[test]
    fn coalesce_resizes_surfaces_the_first_key_not_mouse_motion() {
        use std::cell::Cell;
        let events = [
            Event::Resize(80, 24),
            mouse_moved(),
            Event::Resize(90, 30),
            mouse_moved(),
            key_j(),
        ];
        let idx = Cell::new(0usize);
        let out = coalesce_resizes(
            |_| Ok::<_, ()>(idx.get() < events.len()),
            || {
                let i = idx.get();
                let event = events[i].clone();
                idx.set(i + 1);
                Ok(event)
            },
        )
        .expect("coalesce");
        assert!(matches!(out, Some(Event::Key(_))));
        assert_eq!(
            idx.get(),
            5,
            "must consume motion and resizes before the key"
        );
    }

    #[test]
    fn coalesce_poll_wait_is_the_remaining_quiet_window() {
        use std::cell::Cell;
        let start = std::time::Instant::now();
        let t = Cell::new(start);
        let waits = Cell::new(Vec::<Duration>::new());
        let idx = Cell::new(0usize);
        let events = [Event::Resize(80, 24)];
        let _ = coalesce_resizes_at(
            |wait| {
                waits.set({
                    let mut v = waits.take();
                    v.push(wait);
                    v
                });
                Ok::<_, ()>(idx.get() < events.len())
            },
            || {
                let i = idx.get();
                idx.set(i + 1);
                Ok(events[i].clone())
            },
            || t.get(),
        )
        .expect("coalesce");
        let recorded = waits.take();
        assert!(
            recorded.first().is_some_and(|w| *w == RESIZE_DEBOUNCE),
            "first poll must ask for the quiet window, got {recorded:?}"
        );
        assert!(
            recorded.iter().all(|w| *w <= RESIZE_COALESCE_MAX),
            "poll wait must stay inside the hard cap, got {recorded:?}"
        );
    }

    #[test]
    fn a_resize_restarts_the_quiet_window() {
        use std::cell::Cell;
        let start = std::time::Instant::now();
        let t = Cell::new(start);
        let idx = Cell::new(0usize);
        let waits = Cell::new(Vec::<Duration>::new());
        let events = [Event::Resize(80, 24), Event::Resize(90, 30)];
        let _ = coalesce_resizes_at(
            |wait| {
                waits.set({
                    let mut v = waits.take();
                    v.push(wait);
                    v
                });
                Ok::<_, ()>(idx.get() < events.len())
            },
            || {
                let i = idx.get();
                idx.set(i + 1);
                t.set(t.get() + Duration::from_millis(10));
                Ok(events[i].clone())
            },
            || t.get(),
        )
        .expect("coalesce");
        let recorded = waits.take();
        assert!(
            recorded.get(1).is_some_and(|w| *w == RESIZE_DEBOUNCE),
            "a resize must restart a full quiet window, got {recorded:?}"
        );
    }

    #[test]
    fn coalesce_returns_none_at_the_hard_cap_even_if_resizes_continue() {
        use std::cell::Cell;
        let start = std::time::Instant::now();
        let t = Cell::new(start);
        let idx = Cell::new(0usize);
        let out = coalesce_resizes_at(
            |_| Ok::<_, ()>(true),
            || {
                idx.set(idx.get() + 1);
                t.set(t.get() + RESIZE_COALESCE_MAX);
                Ok(Event::Resize(80, 24))
            },
            || t.get(),
        )
        .expect("coalesce");
        assert!(out.is_none());
        assert!(
            idx.get() <= 2,
            "hard cap must stop the loop, reads={}",
            idx.get()
        );
    }
}
