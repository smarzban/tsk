//! Selection Anchor: keep the selected task id stable across section rebuilds.

use uuid::Uuid;

/// Re-pin selection after the visible row set changes.
///
/// - Empty `new_visible` → `None`.
/// - `previous` still in `new_visible` → keep it (order may have changed).
/// - `previous` left the set → scan outward from its `previous_visible` index
///   for the nearest surviving id, preferring the preceding id on ties.
/// - `previous` absent from `previous_visible` → first `new_visible` id.
/// - `previous` is `None` → `None` (BoardModel seeds on open).
pub fn reanchor(
    previous: Option<Uuid>,
    previous_visible: &[Uuid],
    new_visible: &[Uuid],
) -> Option<Uuid> {
    if new_visible.is_empty() {
        return None;
    }
    let previous = previous?;
    if new_visible.contains(&previous) {
        return Some(previous);
    }
    let Some(idx) = previous_visible.iter().position(|&id| id == previous) else {
        return Some(new_visible[0]);
    };

    // Scan by prior-order distance, preferring the preceding id on ties.
    for distance in 1..=idx.max(previous_visible.len() - idx - 1) {
        if let Some(&candidate) = idx
            .checked_sub(distance)
            .and_then(|index| previous_visible.get(index))
            .filter(|candidate| new_visible.contains(candidate))
        {
            return Some(candidate);
        }
        if let Some(&candidate) = previous_visible
            .get(idx + distance)
            .filter(|candidate| new_visible.contains(candidate))
        {
            return Some(candidate);
        }
    }

    // No old id survived, so preserve the old positional fallback.
    Some(new_visible[idx.min(new_visible.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::reanchor;
    use uuid::Uuid;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn anchor_keeps_same_id_when_still_visible_after_reorder() {
        let a = id(1);
        let b = id(2);
        let c = id(3);
        let previous_visible = [a, b, c];
        let new_visible = [c, a, b]; // reordered; b still present

        assert_eq!(
            reanchor(Some(b), &previous_visible, &new_visible),
            Some(b),
            "selection must stay on the same id across a pure reorder"
        );
    }

    #[test]
    fn anchor_falls_back_to_nearest_surviving_row_when_id_leaves_visible_set() {
        let a = id(1);
        let b = id(2);
        let c = id(3);
        let previous_visible = [a, b, c];

        // b left at index 1 → a and c tie, so prefer preceding a.
        let without_middle = [a, c];
        assert_eq!(
            reanchor(Some(b), &previous_visible, &without_middle),
            Some(a),
            "removed middle row should prefer the preceding equally-close survivor"
        );

        // a left; b is closer in prior order even though c is first after reorder.
        let reordered_survivors = [c, b];
        assert_eq!(
            reanchor(Some(a), &previous_visible, &reordered_survivors),
            Some(b),
            "nearest survivor must use prior order, not the new-list index"
        );

        // c left at index 2 → clamp past end → last survivor b
        let without_end = [a, b];
        assert_eq!(
            reanchor(Some(c), &previous_visible, &without_end),
            Some(b),
            "removed last row should clamp to the new last row"
        );

        // a left at index 0 → clamp to index 0 → b
        let without_start = [b, c];
        assert_eq!(
            reanchor(Some(a), &previous_visible, &without_start),
            Some(b),
            "removed first row should land on the new first row"
        );
    }

    #[test]
    fn empty_sections_yield_empty_anchor() {
        let a = id(1);
        let previous_visible = [a];

        assert_eq!(
            reanchor(Some(a), &previous_visible, &[]),
            None,
            "no visible rows means no selection"
        );
        assert_eq!(
            reanchor(None, &[], &[]),
            None,
            "empty previous and empty new stay empty"
        );
        assert_eq!(
            reanchor(None, &[], &[a]),
            None,
            "missing anchor defers initial selection to BoardModel"
        );
        assert_eq!(
            reanchor(Some(id(2)), &[], &[a, id(3)]),
            Some(a),
            "anchor absent from prior order falls back to the first new row"
        );
    }
}
