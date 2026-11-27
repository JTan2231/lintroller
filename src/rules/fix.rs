use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use tree_sitter::Point;

use super::{Directives, Finding};

#[derive(Clone)]
struct Edit {
    range: Range<usize>,
    replacement: String,
    copied_from: Option<Range<usize>>,
}

/// All edits in a fix must be applied together.
#[derive(Clone, Default)]
pub(super) struct Fix(Arc<Vec<Edit>>);

impl Fix {
    pub(super) fn replace(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self::default().and_replace(range, replacement)
    }

    pub(super) fn and_replace(
        mut self,
        range: Range<usize>,
        replacement: impl Into<String>,
    ) -> Self {
        Arc::make_mut(&mut self.0).push(Edit {
            range,
            replacement: replacement.into(),
            copied_from: None,
        });
        self
    }

    pub(super) fn copy(range: Range<usize>, from: Range<usize>, source: &str) -> Self {
        Self::default().and_copy(range, from, source)
    }

    pub(super) fn and_copy(
        mut self,
        range: Range<usize>,
        from: Range<usize>,
        source: &str,
    ) -> Self {
        Arc::make_mut(&mut self.0).push(Edit {
            range,
            replacement: source[from.clone()].into(),
            copied_from: Some(from),
        });
        self
    }
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start == right.start || left.end > right.start && right.end > left.start
}

struct Lines(Vec<usize>);

impl Lines {
    fn new(source: &str) -> Self {
        Self(
            std::iter::once(0)
                .chain(source.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
        )
    }

    fn point(&self, offset: usize) -> Point {
        let row = self.0.partition_point(|&start| start <= offset) - 1;
        Point::new(row, offset - self.0[row])
    }

    fn offset(&self, point: Point) -> usize {
        self.0[point.row] + point.column
    }
}

pub struct FixPlan<'a> {
    edits: BTreeMap<usize, &'a Edit>,
}

impl<'a> FixPlan<'a> {
    pub fn new(source: &str, directives: &Directives, findings: &'a [Finding]) -> Self {
        let mut candidates: Vec<_> = findings
            .iter()
            .filter(|finding| finding.fix.is_some())
            .collect();
        candidates.sort_by_key(|finding| {
            let priority = match finding.rule {
                "unused_import" => 0, // This fix uses the original compiler index.
                "superfluous_disable_command" => 1,
                "file_types_order" => 3, // Allow fixes inside the declaration to run first.
                _ => 2,
            };
            (priority, finding.point, finding.rule)
        });
        let lines = Lines::new(source);
        let scopes = directives.scoped_rows();
        let mut edits = BTreeMap::new();
        let mut seen = HashSet::new();
        for candidate in candidates {
            let fix = candidate.fix.as_ref().unwrap();
            if !seen.insert(Arc::as_ptr(&fix.0)) {
                continue;
            }
            let mut group: Vec<_> = fix.0.iter().collect();
            group.sort_unstable_by_key(|edit| edit.range.start);
            if group
                .windows(2)
                .any(|pair| overlaps(&pair[0].range, &pair[1].range))
            {
                continue;
            }
            let available = group.iter().copied().all(|edit| {
                let Some(original) = source.get(edit.range.clone()) else {
                    return false;
                };
                if !scopes.is_empty()
                    && (edit.copied_from.is_some()
                        || original.contains('\n')
                        || edit.replacement.contains('\n'))
                {
                    let start = lines.point(edit.range.start);
                    let end = lines.point(edit.range.end);
                    let whole_lines = start.row < end.row
                        && start.column == 0
                        && end.column == 0
                        && edit.replacement.is_empty();
                    let last = end.row - usize::from(whole_lines);
                    if scopes.range(start.row..=last).next().is_some() {
                        return false;
                    }
                }
                let before = edits.range(..=edit.range.start).next_back();
                let after = edits.range(edit.range.start..).next();
                ![before, after]
                    .into_iter()
                    .flatten()
                    .any(|(_, old): (_, &&Edit)| overlaps(&old.range, &edit.range))
            });
            if available
                && group
                    .iter()
                    .any(|edit| source[edit.range.clone()] != edit.replacement)
            {
                edits.extend(group.into_iter().map(|edit| (edit.range.start, edit)));
            }
        }
        Self { edits }
    }

    pub fn apply(&self, source: &str, indexed: &mut Vec<Finding>) -> Option<String> {
        if self.edits.is_empty() {
            return None;
        }
        let mut result = String::with_capacity(source.len());
        let mut end = 0;
        let mut positions = Vec::with_capacity(self.edits.len());
        for edit in self.edits.values() {
            result.push_str(&source[end..edit.range.start]);
            positions.push((*edit, result.len()));
            result.push_str(&edit.replacement);
            end = edit.range.end;
        }
        result.push_str(&source[end..]);

        // Indexed findings track original tokens, including those moved by a
        // file-order fix. A finding whose token was removed has been fixed.
        let old_lines = Lines::new(source);
        let new_lines = Lines::new(&result);
        let mut moved: Vec<_> = positions
            .iter()
            .filter_map(|(edit, start)| Some((edit.copied_from.as_ref()?, *start)))
            .collect();
        moved.sort_unstable_by_key(|(range, _)| range.start);
        indexed.retain_mut(|finding| {
            let previous = old_lines.offset(finding.point);
            let preceding = moved.partition_point(|(from, _)| from.start <= previous);
            let destination = preceding.checked_sub(1).and_then(|i| {
                let (from, start) = moved[i];
                from.contains(&previous)
                    .then(|| start + previous - from.start)
            });
            let point = destination.or_else(|| {
                let before = positions.partition_point(|(edit, _)| edit.range.start <= previous);
                let Some((edit, start)) = before.checked_sub(1).map(|i| positions[i]) else {
                    return Some(previous);
                };
                (!edit.range.contains(&previous))
                    .then(|| start + edit.replacement.len() + previous - edit.range.end)
            });
            if let Some(offset) = point {
                finding.point = new_lines.point(offset);
                // Import fixes have first choice on the original snapshot.
                // Other indexed diagnostics are carried only for reporting.
                finding.fix = None;
            }
            point.is_some()
        });
        Some(result)
    }
}
