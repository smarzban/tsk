//! Tier Layout Resolver: map terminal size to standard/compact frame geometry.

use ratatui::layout::Rect;

/// Layout tier for the queue board: Standard or Compact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Standard,
    Compact,
}

/// Surface that retains focus while responsive presentation changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedSurface {
    Board,
    Task,
}

/// Session-only wide-slider position. Its focus owner is derived from the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WideStage {
    #[default]
    FullBoard,
    Split,
    Rail,
    FullTask,
}

impl WideStage {
    pub const fn focused_surface(self) -> FocusedSurface {
        match self {
            Self::FullBoard | Self::Split => FocusedSurface::Board,
            Self::Rail | Self::FullTask => FocusedSurface::Task,
        }
    }
}

/// Responsive presentation selected for the usable frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsivePresentation {
    SingleBoard,
    SingleTask,
    WideSplit,
}

/// Bounded surface allocations and shared internal density for one usable frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsiveGeometry {
    pub presentation: ResponsivePresentation,
    pub board: Rect,
    /// The one-column separator between board/rail and task, empty outside split stages.
    pub rule: Rect,
    pub task: Rect,
    pub density: Tier,
}

impl ResponsiveGeometry {
    /// Task renderer area, with the one-column split pad removed on its left.
    pub fn task_content(self) -> Rect {
        if self.rule.width > 0 {
            Rect::new(
                self.task.x.saturating_add(u16::from(self.task.width > 0)),
                self.task.y,
                self.task.width.saturating_sub(1),
                self.task.height,
            )
        } else {
            self.task
        }
    }
}

/// Minimum usable width for the wide split view.
pub const WIDE_SPLIT_MIN_WIDTH: u16 = 110;

/// Width of the stage G rail column, including its leading space.
pub const RAIL_WIDTH: u16 = 32;

/// Pure frame geometry for one terminal size.
///
/// Chrome row indices are `None` when that row does not fit. Viewport height is
/// zero when there is no room between selector and bottom chrome. Renderers can
/// place rows from this struct at any size ≥1×1 without panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierGeometry {
    pub tier: Tier,
    pub width: u16,
    pub height: u16,
    /// Selector row (view segments + project chip). Always row 0 when height ≥ 1.
    pub selector_row: Option<u16>,
    /// First list viewport row (below selector).
    pub viewport_top: u16,
    /// List viewport height in rows.
    pub viewport_height: u16,
    /// Dim rule above the status line.
    pub rule_row: Option<u16>,
    /// Status line (counts).
    pub status_row: Option<u16>,
    /// Verb bar (bottom).
    pub verb_row: Option<u16>,
    /// Full content width for a painted row (terminal width).
    pub row_width: u16,
    /// Title cells available after the meta reserve (and 0-width when width is 0).
    pub title_width: u16,
    /// Trailing meta column budget; 0 in compact.
    pub meta_column_width: u16,
    /// Project chip truncation ceiling (cells).
    pub selector_chip_max: u16,
    /// How many verb-bar entries may be shown.
    pub verb_bar_entry_budget: u16,
}

impl TierGeometry {
    /// Whether this geometry paints its own rule, status and verb rows. A wide column does
    /// not: the shared footer owns those rows for the whole frame.
    pub fn owns_footer(&self) -> bool {
        self.rule_row.is_some() || self.status_row.is_some() || self.verb_row.is_some()
    }

    /// Rebuild title/meta budgets for a narrower content width (scrollbar gap + track).
    ///
    /// Only shrinking `row_width` left title+meta summing past the row and clipped
    /// every task into `…`.
    pub fn with_row_width(self, row_width: u16) -> Self {
        // Preserve the resolved meta budget (the wide board column sizes it to its content)
        // and clamp it to the narrower row, so title+meta never overrun the row.
        let meta_column_width = self.meta_column_width.min(row_width);
        let title_width = row_width.saturating_sub(meta_column_width);
        Self {
            row_width,
            title_width,
            meta_column_width,
            ..self
        }
    }
}

/// Project chip truncation ceiling shared by both tiers.
pub const SELECTOR_CHIP_MAX_CELLS: u16 = 24;

/// Compact verb bar keeps at most this many entries.
pub const COMPACT_VERB_BAR_ENTRY_BUDGET: u16 = 5;

/// Full standard verb-bar set: ctrl+s · enter · ctrl+d · ctrl+b · : · ? · +.
pub const STANDARD_VERB_BAR_ENTRY_BUDGET: u16 = 7;

/// Minimum width for the standard board tier. This is the default Herdr split amendment.
const STANDARD_MIN_WIDTH: u16 = 78;

/// Reserved trailing meta cells in the standard tier for full project/thread attribution.
const STANDARD_META_COLUMN_WIDTH: u16 = 36;

/// Map terminal dimensions to a tier and frame geometry.
///
/// Standard when width ≥ 78 and height ≥ 24; otherwise compact. Width ≥ 120 still
/// reports Standard; there is no wide tier here. Any size yields a geometry;
/// callers may pass values below 1×1 and still receive a non-panicking result.
pub fn resolve(width: u16, height: u16) -> TierGeometry {
    let tier = if width >= STANDARD_MIN_WIDTH && height >= 24 {
        Tier::Standard
    } else {
        Tier::Compact
    };
    resolve_density(width, height, tier)
}

/// Build frame geometry at an already-resolved responsive density.
///
/// Row positions still follow this surface's own height; title, metadata, and verb
/// budgets follow the shared density decision.
pub(crate) fn resolve_density(width: u16, height: u16, tier: Tier) -> TierGeometry {
    let (selector_row, viewport_top, viewport_height, rule_row, status_row, verb_row) =
        chrome_rows(height);

    let meta_column_width = match tier {
        Tier::Standard => STANDARD_META_COLUMN_WIDTH.min(width),
        Tier::Compact => 0,
    };
    let title_width = width.saturating_sub(meta_column_width);
    let verb_bar_entry_budget = match tier {
        Tier::Standard => STANDARD_VERB_BAR_ENTRY_BUDGET,
        Tier::Compact => COMPACT_VERB_BAR_ENTRY_BUDGET,
    };

    TierGeometry {
        tier,
        width,
        height,
        selector_row,
        viewport_top,
        viewport_height,
        rule_row,
        status_row,
        verb_row,
        row_width: width,
        title_width,
        meta_column_width,
        selector_chip_max: SELECTOR_CHIP_MAX_CELLS,
        verb_bar_entry_budget,
    }
}

/// Map usable dimensions and the slider stage to one or two bounded surfaces.
///
/// Wide stages A and G divide the frame into a left column, a one-column rule and a task
/// column whose first cell is a pad. Stages 0 and F use the whole frame. Both columns share
/// the frame's density.
pub fn resolve_responsive(width: u16, height: u16, stage: WideStage) -> ResponsiveGeometry {
    let frame = Rect::new(0, 0, width, height);
    if width < WIDE_SPLIT_MIN_WIDTH {
        return match stage.focused_surface() {
            FocusedSurface::Board => ResponsiveGeometry {
                presentation: ResponsivePresentation::SingleBoard,
                board: frame,
                rule: Rect::default(),
                task: Rect::default(),
                density: resolve(width, height).tier,
            },
            FocusedSurface::Task => ResponsiveGeometry {
                presentation: ResponsivePresentation::SingleTask,
                board: Rect::default(),
                rule: Rect::default(),
                task: frame,
                density: resolve(width, height).tier,
            },
        };
    }

    let (board, rule, task) = match stage {
        WideStage::FullBoard => (frame, Rect::default(), Rect::default()),
        WideStage::FullTask => (Rect::default(), Rect::default(), frame),
        WideStage::Split => {
            let board_width = width.saturating_mul(2) / 5;
            let rule = Rect::new(board_width, 0, 1, height);
            let task_x = board_width.saturating_add(rule.width);
            (
                Rect::new(0, 0, board_width, height),
                rule,
                Rect::new(task_x, 0, width.saturating_sub(task_x), height),
            )
        }
        WideStage::Rail => {
            // Only reached at width >= WIDE_SPLIT_MIN_WIDTH, so the rail, rule and pad
            // always fit.
            let rule = Rect::new(RAIL_WIDTH, 0, 1, height);
            let task_x = RAIL_WIDTH.saturating_add(rule.width);
            (
                Rect::new(0, 0, RAIL_WIDTH, height),
                rule,
                Rect::new(task_x, 0, width.saturating_sub(task_x), height),
            )
        }
    };
    // Density follows the frame, not the narrower column: a split at 130×24 keeps the
    // standard row rhythm and the board's meta column, and a full-width stage is exactly the
    // standard board. Only a short frame (height < 24) drops to compact.
    ResponsiveGeometry {
        presentation: ResponsivePresentation::WideSplit,
        board,
        rule,
        task,
        density: resolve(width, height).tier,
    }
}

/// Geometry for one wide column that paints no footer of its own.
///
/// The shared footer owns the rule, status and verb rows for the whole frame; a column keeps
/// the frame's row rhythm (blank row, selector row, viewport) and stops at `height`.
pub fn resolve_column(width: u16, height: u16, frame_height: u16, tier: Tier) -> TierGeometry {
    let mut geometry = resolve_density(width, frame_height, tier);
    geometry.height = height;
    geometry.rule_row = None;
    geometry.status_row = None;
    geometry.verb_row = None;
    geometry.viewport_height = height.saturating_sub(geometry.viewport_top);
    geometry
}

/// Place chrome from the outside in so indices never overlap.
///
/// height ≥ 4: blank · selector · viewport · rule · status · verb.
/// height == 3: selector · status · verb.
/// height == 2: selector · verb.
/// height ≤ 1: selector only (or nothing when height == 0).
fn chrome_rows(height: u16) -> (Option<u16>, u16, u16, Option<u16>, Option<u16>, Option<u16>) {
    match height {
        0 => (None, 0, 0, None, None, None),
        1 => (Some(0), 0, 0, None, None, None),
        2 => (Some(0), 0, 0, None, None, Some(1)),
        3 => (Some(0), 0, 0, None, Some(1), Some(2)),
        4 => (Some(0), 1, 0, Some(1), Some(2), Some(3)),
        h => {
            // row 0 blank; row 1 selector; rows 2..h-4 list; h-3 rule; h-2 status; h-1 verbs
            (
                Some(1),
                2,
                h.saturating_sub(5),
                Some(h - 3),
                Some(h - 2),
                Some(h - 1),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_stage_geometry_has_exact_allocations_and_preserves_narrow_mapping() {
        for (width, split_board, split_task, rail_task) in [(110, 44, 65, 77), (130, 52, 77, 97)] {
            let split = resolve_responsive(width, 24, WideStage::Split);
            assert_eq!(split.board, Rect::new(0, 0, split_board, 24));
            assert_eq!(split.rule, Rect::new(split_board, 0, 1, 24));
            assert_eq!(split.task, Rect::new(split_board + 1, 0, split_task, 24));
            let rail = resolve_responsive(width, 24, WideStage::Rail);
            assert_eq!(rail.board, Rect::new(0, 0, 32, 24));
            assert_eq!(rail.rule, Rect::new(32, 0, 1, 24));
            assert_eq!(rail.task, Rect::new(33, 0, rail_task, 24));
            assert_eq!(
                resolve_responsive(width, 24, WideStage::FullBoard).board,
                Rect::new(0, 0, width, 24)
            );
            assert_eq!(
                resolve_responsive(width, 24, WideStage::FullTask).task,
                Rect::new(0, 0, width, 24)
            );
        }
        assert_eq!(
            resolve_responsive(109, 24, WideStage::Split).presentation,
            ResponsivePresentation::SingleBoard
        );
        assert_eq!(
            resolve_responsive(109, 24, WideStage::Rail).presentation,
            ResponsivePresentation::SingleTask
        );
    }

    #[test]
    fn wide_stage_geometry_is_bounded_and_exhausts_every_column() {
        for width in WIDE_SPLIT_MIN_WIDTH..=250 {
            for height in 10..=60 {
                for stage in [
                    WideStage::FullBoard,
                    WideStage::Split,
                    WideStage::Rail,
                    WideStage::FullTask,
                ] {
                    let geometry = resolve_responsive(width, height, stage);
                    assert_eq!(
                        geometry.board.width + geometry.rule.width + geometry.task.width,
                        width
                    );
                    for rect in [
                        geometry.board,
                        geometry.rule,
                        geometry.task,
                        geometry.task_content(),
                    ] {
                        assert!(u32::from(rect.x) + u32::from(rect.width) <= u32::from(width));
                        assert!(u32::from(rect.y) + u32::from(rect.height) <= u32::from(height));
                    }
                }
            }
        }
    }

    #[test]
    fn at_78x24_and_above_until_120_cols_tier_is_standard_with_selector_viewport_rule_status_verb_rows(
    ) {
        for &(w, h) in &[
            (78, 24),
            (78, 30),
            (100, 24),
            (119, 40),
            (120, 24),
            (200, 50),
        ] {
            let g = resolve(w, h);
            assert_eq!(g.tier, Tier::Standard, "{w}x{h}");
            assert_eq!(g.selector_row, Some(1), "{w}x{h}");
            assert_eq!(g.viewport_top, 2, "{w}x{h}");
            assert_eq!(g.viewport_height, h.saturating_sub(5), "{w}x{h}");
            assert_eq!(g.rule_row, Some(h - 3), "{w}x{h}");
            assert_eq!(g.status_row, Some(h - 2), "{w}x{h}");
            assert_eq!(g.verb_row, Some(h - 1), "{w}x{h}");
            assert_eq!(g.row_width, w, "{w}x{h}");
            assert!(g.meta_column_width > 0, "{w}x{h} standard reserves meta");
            assert_eq!(
                g.title_width,
                w.saturating_sub(g.meta_column_width),
                "{w}x{h}"
            );
            assert_eq!(g.selector_chip_max, SELECTOR_CHIP_MAX_CELLS, "{w}x{h}");
            assert_eq!(
                g.verb_bar_entry_budget, STANDARD_VERB_BAR_ENTRY_BUDGET,
                "{w}x{h}"
            );
            if (w, h) == (STANDARD_MIN_WIDTH, 24) {
                assert_eq!(g.meta_column_width, STANDARD_META_COLUMN_WIDTH);
                assert_eq!(
                    g.title_width, 42,
                    "78 columns retain room for full thread metadata"
                );
            }
            let narrowed = g.with_row_width(w.saturating_sub(2));
            assert_eq!(narrowed.row_width, w.saturating_sub(2), "{w}x{h}");
            assert_eq!(
                narrowed
                    .title_width
                    .saturating_add(narrowed.meta_column_width),
                narrowed.row_width,
                "{w}x{h} scrollbar shrink must rebalance title+meta"
            );
            // Width ≥ 120 still reports Standard; there is no wide tier here.
            if w >= 120 {
                assert_eq!(g.tier, Tier::Standard, "wide not returned in M1 at {w}x{h}");
            }
        }
    }

    #[test]
    fn below_78_or_below_24_tier_is_compact_with_one_line_rows_and_verb_bar_budget_leq_5() {
        for &(w, h) in &[
            (77, 24),
            (78, 23),
            (77, 23),
            (48, 18),
            (40, 10),
            (50, 30),
            (100, 20),
        ] {
            let g = resolve(w, h);
            assert_eq!(g.tier, Tier::Compact, "{w}x{h}");
            assert_eq!(g.meta_column_width, 0, "{w}x{h} one-line rows: no meta");
            assert_eq!(g.title_width, w, "{w}x{h}");
            assert_eq!(g.row_width, w, "{w}x{h}");
            assert!(
                g.verb_bar_entry_budget <= 5,
                "{w}x{h} verb budget {} > 5",
                g.verb_bar_entry_budget
            );
            assert_eq!(
                g.verb_bar_entry_budget, COMPACT_VERB_BAR_ENTRY_BUDGET,
                "{w}x{h}"
            );
            assert_eq!(g.selector_chip_max, SELECTOR_CHIP_MAX_CELLS, "{w}x{h}");
            if h >= 4 {
                assert_eq!(g.selector_row, Some(1), "{w}x{h}");
                assert_eq!(g.viewport_top, 2, "{w}x{h}");
                assert_eq!(g.viewport_height, h.saturating_sub(5), "{w}x{h}");
                assert_eq!(g.rule_row, Some(h - 3), "{w}x{h}");
                assert_eq!(g.status_row, Some(h - 2), "{w}x{h}");
                assert_eq!(g.verb_row, Some(h - 1), "{w}x{h}");
            }
        }
    }

    #[test]
    fn every_size_from_1x1_through_40x10_returns_a_geometry_without_panic() {
        for h in 1u16..=10 {
            for w in 1u16..=40 {
                let g = resolve(w, h);
                assert_eq!(g.width, w);
                assert_eq!(g.height, h);
                assert_eq!(g.row_width, w);
                assert!(
                    g.title_width.saturating_add(g.meta_column_width) <= g.row_width,
                    "{w}x{h}: title+meta exceed row"
                );
                assert_eq!(g.selector_chip_max, SELECTOR_CHIP_MAX_CELLS);
                // Compact at these sizes (all below standard breakpoint).
                assert_eq!(g.tier, Tier::Compact, "{w}x{h}");
                assert_eq!(g.meta_column_width, 0, "{w}x{h}");
                assert!(g.verb_bar_entry_budget <= 5, "{w}x{h}");
                // Chrome indices that exist stay inside the frame and do not collide.
                let mut used = vec![false; h as usize];
                for row in [g.selector_row, g.rule_row, g.status_row, g.verb_row]
                    .into_iter()
                    .flatten()
                {
                    assert!(row < h, "{w}x{h}: chrome row {row} out of bounds");
                    assert!(
                        !used[row as usize],
                        "{w}x{h}: chrome row {row} occupied twice"
                    );
                    used[row as usize] = true;
                }
                if g.viewport_height > 0 {
                    let end = g.viewport_top as u32 + g.viewport_height as u32;
                    assert!(end <= h as u32, "{w}x{h}: viewport overflows");
                    for row in g.viewport_top..g.viewport_top + g.viewport_height {
                        assert!(
                            !used[row as usize],
                            "{w}x{h}: viewport overlaps chrome at {row}"
                        );
                    }
                }
            }
        }
    }
}
