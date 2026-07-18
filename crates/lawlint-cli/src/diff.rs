//! Line-level diff shared by the CLI `--diff` flag and the TUI `/fix` view.
//!
//! A plain LCS over lines; fix diffs are small (a handful of changed lines in
//! a document), so the quadratic table is bounded by trimming the common
//! prefix and suffix first.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Line present in both texts.
    Same(String),
    /// Line only in the original text.
    Removed(String),
    /// Line only in the fixed text.
    Added(String),
}

/// Diff `before` against `after` line by line, in order.
pub fn diff_lines(before: &str, after: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();

    // Trim the common prefix and suffix so the LCS table only covers the
    // changed middle.
    let mut start = 0;
    while start < old.len() && start < new.len() && old[start] == new[start] {
        start += 1;
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }

    let mut out: Vec<DiffLine> = old[..start]
        .iter()
        .map(|l| DiffLine::Same((*l).to_string()))
        .collect();
    out.extend(diff_middle(&old[start..old_end], &new[start..new_end]));
    out.extend(
        old[old_end..]
            .iter()
            .map(|l| DiffLine::Same((*l).to_string())),
    );
    out
}

/// LCS-based diff of the changed middle sections.
fn diff_middle(old: &[&str], new: &[&str]) -> Vec<DiffLine> {
    // lcs[i][j] = length of the LCS of old[i..] and new[j..].
    let mut lcs = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            out.push(DiffLine::Same(old[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine::Removed(old[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Added(new[j].to_string()));
            j += 1;
        }
    }
    out.extend(old[i..].iter().map(|l| DiffLine::Removed((*l).to_string())));
    out.extend(new[j..].iter().map(|l| DiffLine::Added((*l).to_string())));
    out
}

/// Collapse runs of unchanged lines, keeping `context` lines around each
/// change. Runs longer than that are replaced by a `None` separator entry.
/// Returns `(line, is_separator)` pairs ready for rendering.
pub fn with_context(lines: &[DiffLine], context: usize) -> Vec<Option<DiffLine>> {
    let changed: Vec<bool> = lines
        .iter()
        .map(|l| !matches!(l, DiffLine::Same(_)))
        .collect();
    let mut keep = vec![false; lines.len()];
    for (i, &is_changed) in changed.iter().enumerate() {
        if is_changed {
            let lo = i.saturating_sub(context);
            let hi = (i + context + 1).min(lines.len());
            for flag in &mut keep[lo..hi] {
                *flag = true;
            }
        }
    }

    let mut out = Vec::new();
    let mut in_gap = false;
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            out.push(Some(line.clone()));
            in_gap = false;
        } else if !in_gap {
            out.push(None);
            in_gap = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(s: &str) -> DiffLine {
        DiffLine::Same(s.into())
    }
    fn rem(s: &str) -> DiffLine {
        DiffLine::Removed(s.into())
    }
    fn add(s: &str) -> DiffLine {
        DiffLine::Added(s.into())
    }

    #[test]
    fn identical_texts_are_all_same() {
        assert_eq!(diff_lines("a\nb", "a\nb"), vec![same("a"), same("b")]);
    }

    #[test]
    fn single_changed_line() {
        assert_eq!(
            diff_lines("a\nold\nc", "a\nnew\nc"),
            vec![same("a"), rem("old"), add("new"), same("c")]
        );
    }

    #[test]
    fn insertion_and_deletion() {
        assert_eq!(
            diff_lines("a\nb\nc", "a\nc\nd"),
            vec![same("a"), rem("b"), same("c"), add("d")]
        );
    }

    #[test]
    fn empty_before_is_all_added() {
        assert_eq!(diff_lines("", "x\ny"), vec![add("x"), add("y")]);
    }

    #[test]
    fn context_collapses_distant_same_runs() {
        let lines = vec![
            same("1"),
            same("2"),
            same("3"),
            rem("old"),
            add("new"),
            same("4"),
            same("5"),
            same("6"),
        ];
        let out = with_context(&lines, 1);
        assert_eq!(
            out,
            vec![
                None,
                Some(same("3")),
                Some(rem("old")),
                Some(add("new")),
                Some(same("4")),
                None,
            ]
        );
    }
}
