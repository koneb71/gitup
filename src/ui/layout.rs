//! Where the splits sit before the user moves them.
//!
//! Pure arithmetic, kept out of the drawing code so it can be checked at sizes
//! nobody is going to open the app at by hand. The default split is easy to get
//! subtly wrong in a way that only shows up on one screen size, and the way it
//! was wrong before was invisible to every test in the project: all the widgets
//! were present and correct, there was simply nowhere to read the diff.

/// How tall the detail pane should be, given the height of the centre.
///
/// The naive answer — a fixed share of the centre — is wrong, and wrong in a
/// direction that hurts small windows most. The detail pane pays for its two
/// header bands and the commit box out of its *own* share, and none of that
/// furniture shrinks with the window. Measured: a plain 66% share leaves the
/// diff 301px at 1280x820 but only 129px at the 880x560 minimum, because the
/// furniture takes the same fixed cut out of a much smaller allowance.
///
/// So the furniture is added on top of the share rather than taken out of it:
/// `share` is the fraction of the *usable* centre — what is left once both
/// panes' fixed costs are paid — that goes to the diff.
///
/// History keeps a floor, so a short window cannot squeeze the commit graph
/// down to a row or two of navigation that is no use to anyone.
pub fn detail_height(centre: f32, furniture: f32, share: f32) -> f32 {
    let usable = (centre - furniture).max(0.0);
    let wanted = furniture + usable * share.clamp(0.0, 1.0);
    // Never take so much that history has nowhere to be, and never claim more
    // than exists.
    wanted.min(centre - HISTORY_FLOOR).max(0.0).min(centre)
}

/// The share of the usable centre implied by a detail pane of `height`.
///
/// The inverse of [`detail_height`], so that a size the user dragged to at one
/// window size is restored as the same *proportion* at another. Storing the
/// pixels instead would mean a split chosen on a laptop reopened wrong on a
/// monitor.
pub fn share_of(centre: f32, furniture: f32, height: f32) -> f32 {
    let usable = centre - furniture;
    if usable <= 1.0 {
        return DETAIL_SHARE;
    }
    ((height - furniture) / usable).clamp(0.0, 1.0)
}

/// Rows of commit graph that must survive whatever the diff wants.
///
/// Five rows: enough to see where you are and to reach the working-tree row
/// at the top without scrolling.
const HISTORY_FLOOR: f32 = 5.0 * 26.0;

/// The share of the usable centre the diff gets by default.
///
/// Chosen so the default window keeps the ~300px the diff needs to be worth
/// reading, rather than by taste.
pub const DETAIL_SHARE: f32 = 0.55;

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed cost inside the detail pane: two header bands and the commit
    /// box at its default height.
    ///
    /// Taken from the real constants rather than written down, so that raising
    /// the commit box or adding a header band fails these tests instead of
    /// quietly eating the diff again.
    const FURNITURE: f32 = crate::ui::metrics::DETAIL_HEADERS + crate::ui::metrics::COMMIT_BOX;

    /// Centre height for a window of `height`, past the toolbar, tab bar and
    /// status bar.
    fn centre(height: f32) -> f32 {
        height - 72.0 - 26.0
    }

    fn diff_lines(window_height: f32) -> f32 {
        let c = centre(window_height);
        let detail = detail_height(c, FURNITURE, DETAIL_SHARE);
        (detail - FURNITURE) / 17.0
    }

    #[test]
    fn the_default_window_gets_a_readable_diff() {
        // The number that started all this: the old layout left ten lines.
        assert!(
            diff_lines(820.0) > 16.0,
            "only {:.1} lines at 1280x820",
            diff_lines(820.0)
        );
    }

    #[test]
    fn the_smallest_window_is_still_usable() {
        // A plain share of the centre gave 7.6 lines here, because the
        // furniture takes its fixed cut whatever the window does. This is the
        // size at which the two halves genuinely compete, so the bar is lower —
        // history holds a floor of five rows out of the same 462px.
        let lines = diff_lines(560.0);
        assert!(lines > 8.0, "only {lines:.1} lines at 880x560");
    }

    #[test]
    fn a_large_window_gives_its_extra_space_to_the_diff() {
        // The reason the split is a share and not a height: a fixed height
        // would hand every extra pixel to the graph, which is navigation.
        assert!(
            diff_lines(1440.0) > 35.0,
            "only {:.1} lines at 2560x1440",
            diff_lines(1440.0)
        );
    }

    #[test]
    fn history_keeps_its_floor() {
        // Even asking for everything cannot leave the graph with nothing.
        let c = centre(560.0);
        let greedy = detail_height(c, FURNITURE, 1.0);
        assert!(
            c - greedy >= HISTORY_FLOOR - 0.5,
            "history left with {:.0}px",
            c - greedy
        );
    }

    #[test]
    fn a_share_survives_the_round_trip() {
        let c = centre(820.0);
        let height = detail_height(c, FURNITURE, DETAIL_SHARE);
        let back = share_of(c, FURNITURE, height);
        assert!(
            (back - DETAIL_SHARE).abs() < 0.001,
            "{back} != {DETAIL_SHARE}"
        );
    }

    #[test]
    fn a_split_chosen_on_one_screen_reopens_in_proportion_on_another() {
        // Someone drags the split on a laptop; the same share should give a
        // proportionally larger diff on a monitor, not the same pixel height.
        let laptop = centre(820.0);
        let monitor = centre(1440.0);

        let chosen = detail_height(laptop, FURNITURE, DETAIL_SHARE) + 60.0;
        let share = share_of(laptop, FURNITURE, chosen);
        let restored = detail_height(monitor, FURNITURE, share);

        assert!(
            restored > chosen,
            "the bigger screen gave the diff no more room: {restored:.0} vs {chosen:.0}"
        );
    }

    #[test]
    fn absurd_inputs_do_not_produce_absurd_layouts() {
        for centre in [0.0, 1.0, 50.0, 200.0] {
            let h = detail_height(centre, FURNITURE, DETAIL_SHARE);
            assert!(h >= 0.0 && h <= centre.max(0.0), "{h} out of {centre}");
        }
        // A share outside 0..1 is a bug elsewhere, but must not corrupt this.
        for share in [-1.0, 2.0, f32::NAN] {
            let h = detail_height(600.0, FURNITURE, share);
            assert!(h.is_finite() || share.is_nan(), "{h} from share {share}");
        }
    }
}
