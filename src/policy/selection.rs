//! Multi-selection for the clip library — file-manager semantics.
//!
//! Ported from `src/lib/selection.ts`, reducer and all, because the shift-range
//! is computed over the **full filtered order**, not the currently painted
//! window. That detail is the whole reason this is a reducer instead of a
//! `HashSet` in the view: shift-click across pages must select everything
//! between the anchor and the click, including rows not yet rendered.
//!
//! Selections are keyed by absolute clip path, which is stable across rescans
//! (unlike `id`, which is re-derived from a path hash + timestamp).

/// The last clicked path (shift-clicks extend the range from it) plus the
/// selected paths in click order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Option<String>,
    pub selected: Vec<String>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.anchor = None;
        self.selected.clear();
    }

    pub fn is_selected(&self, path: &str) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// Applies a click. `order` is the *visible* clip order.
    ///
    /// * plain click — replaces the selection with just this clip
    /// * shift — selects the range from the anchor, in visible order
    /// * ctrl — toggles just this clip
    /// * ctrl+shift — unions the range with the existing selection
    ///
    /// An anchor that is gone (filtered out, deleted) degrades to a plain
    /// single selection instead of a bogus range.
    pub fn click(&mut self, path: &str, order: &[String], ctrl: bool, shift: bool) {
        if !ctrl && !shift {
            self.anchor = Some(path.to_string());
            self.selected = vec![path.to_string()];
            return;
        }

        if shift {
            if let Some(anchor) = self.anchor.clone() {
                let a = order.iter().position(|p| p == &anchor);
                let b = order.iter().position(|p| p == path);
                if let (Some(a), Some(b)) = (a, b) {
                    let (from, to) = if a <= b { (a, b) } else { (b, a) };
                    let range = &order[from..=to];
                    if ctrl {
                        // Ctrl+Shift: union the range with the existing
                        // selection, keeping first-seen order.
                        let mut merged = self.selected.clone();
                        for p in range {
                            if !merged.iter().any(|x| x == p) {
                                merged.push(p.clone());
                            }
                        }
                        self.selected = merged;
                    } else {
                        self.selected = range.to_vec();
                    }
                    self.anchor = Some(path.to_string());
                    return;
                }
            }
            // No usable anchor -> plain single selection.
            self.anchor = Some(path.to_string());
            self.selected = vec![path.to_string()];
            return;
        }

        // Ctrl-only: toggle.
        self.anchor = Some(path.to_string());
        if let Some(i) = self.selected.iter().position(|p| p == path) {
            self.selected.remove(i);
        } else {
            self.selected.push(path.to_string());
        }
    }

    pub fn select_all(&mut self, order: &[String]) {
        self.anchor = order.first().cloned();
        self.selected = order.to_vec();
    }

    /// Drops paths that no longer exist (deleted, filtered away by a delete).
    /// Clears the anchor if it was one of them, so the next shift-click starts
    /// fresh instead of selecting a phantom range.
    pub fn remove(&mut self, paths: &[String]) {
        if let Some(anchor) = self.anchor.as_ref() {
            if paths.iter().any(|p| p == anchor) {
                self.anchor = None;
            }
        }
        self.selected.retain(|p| !paths.iter().any(|gone| gone == p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> Vec<String> {
        ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn plain_click_replaces_the_selection() {
        let mut s = Selection::default();
        s.click("a", &order(), false, false);
        s.click("c", &order(), false, false);
        assert_eq!(s.selected, vec!["c".to_string()]);
        assert_eq!(s.anchor.as_deref(), Some("c"));
    }

    #[test]
    fn ctrl_click_toggles_without_disturbing_the_rest() {
        let mut s = Selection::default();
        s.click("a", &order(), false, false);
        s.click("c", &order(), true, false);
        assert_eq!(s.selected, vec!["a".to_string(), "c".to_string()]);
        // Clicking it again removes only that one.
        s.click("c", &order(), true, false);
        assert_eq!(s.selected, vec!["a".to_string()]);
        assert_eq!(s.anchor.as_deref(), Some("c"));
    }

    #[test]
    fn shift_click_selects_the_range_in_visible_order() {
        let mut s = Selection::default();
        s.click("b", &order(), false, false);
        s.click("d", &order(), false, true);
        assert_eq!(
            s.selected,
            vec!["b".to_string(), "c".to_string(), "d".to_string()]
        );
        assert_eq!(s.anchor.as_deref(), Some("d"));
    }

    #[test]
    fn shift_click_works_backwards() {
        let mut s = Selection::default();
        s.click("d", &order(), false, false);
        s.click("b", &order(), false, true);
        assert_eq!(
            s.selected,
            vec!["b".to_string(), "c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn ctrl_shift_unions_the_range_with_the_existing_selection() {
        let mut s = Selection::default();
        s.click("a", &order(), false, false);
        s.click("e", &order(), true, false); // {a, e}
        // Anchor is now "e"; ctrl+shift-click "c" unions [c..e].
        s.click("c", &order(), true, true);
        assert_eq!(
            s.selected,
            vec![
                "a".to_string(),
                "e".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
    }

    #[test]
    fn ctrl_shift_does_not_duplicate_already_selected_paths() {
        let mut s = Selection::default();
        s.click("b", &order(), false, false);
        s.click("c", &order(), true, false); // {b, c}, anchor c
        s.click("d", &order(), true, true); // range [c..d]
        assert_eq!(
            s.selected,
            vec!["b".to_string(), "c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn shift_click_without_a_usable_anchor_degrades_to_a_single_selection() {
        let mut s = Selection::default();
        s.click("c", &order(), false, true);
        assert_eq!(s.selected, vec!["c".to_string()]);

        // Anchor that is no longer in the visible order (filtered out).
        let mut s = Selection::default();
        s.click("a", &order(), false, false);
        s.click("e", &["c".to_string(), "d".to_string(), "e".to_string()], false, true);
        assert_eq!(s.selected, vec!["e".to_string()]);
    }

    #[test]
    fn select_all_uses_the_full_filtered_order() {
        let mut s = Selection::default();
        s.select_all(&order());
        assert_eq!(s.selected, order());
        assert_eq!(s.anchor.as_deref(), Some("a"));
    }

    #[test]
    fn remove_drops_paths_and_clears_a_deleted_anchor() {
        let mut s = Selection::default();
        s.select_all(&order());
        s.remove(&["a".to_string(), "c".to_string()]);
        assert_eq!(
            s.selected,
            vec!["b".to_string(), "d".to_string(), "e".to_string()]
        );
        // Anchor "a" was removed -> gone, so the next shift-click is a plain
        // click.
        assert_eq!(s.anchor, None);
    }

    #[test]
    fn remove_keeps_an_unaffected_anchor() {
        let mut s = Selection::default();
        s.select_all(&order());
        s.remove(&["e".to_string()]);
        assert_eq!(s.anchor.as_deref(), Some("a"));
    }

    #[test]
    fn selecting_nothing_reports_zero() {
        let mut s = Selection::default();
        s.select_all(&order());
        s.clear();
        assert_eq!(s.count(), 0);
        assert!(!s.is_selected("a"));
        assert_eq!(s.anchor, None);
    }
}
