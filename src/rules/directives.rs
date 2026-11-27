use super::{Context, Findings, Fix};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use tree_sitter::Point;

struct Command {
    rule: String,
    location: Point,
    // Scoped commands affect the whole target line. A region begins at the
    // end of its comment, so its own rule names can still receive diagnostics.
    start: Point,
    scoped: bool,
    disable: bool,
    text: DirectiveText,
}

struct DirectiveText {
    marker: usize,
    args: Range<usize>,
    comment: Range<usize>,
}

impl DirectiveText {
    fn remove(&self, source: &str, rule: &str) -> Option<Fix> {
        let args = &source[self.args.clone()];
        let mut words = args.split_whitespace().skip(1);
        let rules: Vec<_> = words.by_ref().take_while(|&word| word != "-").collect();
        if rules.iter().all(|&word| word == rule) {
            let reason = words.next();
            let end = reason.map_or(self.args.end, |word| {
                self.args.start + (word.as_ptr() as usize - args.as_ptr() as usize)
            });
            if source[end..self.args.end].contains("swiftlint:") {
                return None;
            }
            if reason.is_none() && !source[self.comment.clone()].contains('\n') {
                let before = source[self.comment.start..self.marker].trim();
                let after = source[end..self.comment.end].trim();
                if (before == "//" && after.is_empty()) || (before == "/*" && after == "*/") {
                    let replacement = if before == "/*"
                        && self.comment.start > 0
                        && self.comment.end < source.len()
                        && !source.as_bytes()[self.comment.start - 1].is_ascii_whitespace()
                        && !source.as_bytes()[self.comment.end].is_ascii_whitespace()
                    {
                        " "
                    } else {
                        ""
                    };
                    return Some(Fix::replace(self.comment.clone(), replacement));
                }
            }
            return Some(Fix::replace(self.marker..end, ""));
        }

        let mut ranges = rules.into_iter().filter(|&word| word == rule).map(|word| {
            let start = word.as_ptr() as usize - args.as_ptr() as usize;
            let before = args[..start].trim_end().len();
            self.args.start + before..self.args.start + start + word.len()
        });
        let fix = Fix::replace(ranges.next()?, "");
        Some(ranges.fold(fix, |fix, range| fix.and_replace(range, "")))
    }
}

#[derive(Default)]
struct Timeline {
    regions: Vec<(Point, usize)>,
    lines: HashMap<usize, usize>,
}

impl Timeline {
    fn command(&self, at: Point) -> Option<usize> {
        if let Some(id) = self.lines.get(&at.row) {
            return Some(*id);
        }
        let end = self.regions.partition_point(|(start, _)| *start <= at);
        end.checked_sub(1).map(|i| self.regions[i].1)
    }
}

#[derive(Default)]
pub struct Directives {
    commands: Vec<Command>,
    timelines: HashMap<String, Timeline>,
}

impl Directives {
    pub(super) fn from_comments(cx: &Context<'_>) -> Self {
        let mut result = Self::default();
        if !cx.source.contains("swiftlint:") {
            return result;
        }
        for &node in &cx.nodes {
            if !matches!(node.kind(), "comment" | "multiline_comment") {
                continue;
            }
            for (offset, line) in cx.text(node).lines().enumerate() {
                let Some(marker) = line.find("swiftlint:") else {
                    continue;
                };
                let row = node.start_position().row + offset;
                let column = if offset == 0 {
                    node.start_position().column
                } else {
                    0
                };
                let end = Point::new(row, column + line.len());
                let args = line[marker + "swiftlint:".len()..].trim_end_matches("*/");
                let line_start = cx.line_starts[row] + column;
                let args_start = line_start + marker + "swiftlint:".len();
                let mut words = args.split_whitespace();
                let Some(verb) = words.next() else { continue };
                let (action, scope) = verb.split_once(':').unwrap_or((verb, ""));
                let disable = match action {
                    "disable" => true,
                    "enable" => false,
                    _ => continue,
                };
                let start = match scope {
                    "" => end,
                    "this" => Point::new(row, 0),
                    "next" => Point::new(row + 1, 0),
                    "previous" if row > 0 => Point::new(row - 1, 0),
                    _ => continue,
                };
                // split_whitespace returns slices of this same source line.
                let mut seen = HashSet::new();
                for rule in words.take_while(|&word| word != "-") {
                    if !seen.insert(rule) {
                        continue;
                    }
                    let rule_column = rule.as_ptr() as usize - line.as_ptr() as usize + column;
                    let command = Command {
                        rule: rule.into(),
                        location: Point::new(row, rule_column),
                        start,
                        scoped: !scope.is_empty(),
                        disable,
                        text: DirectiveText {
                            marker: line_start + marker,
                            args: args_start..args_start + args.len(),
                            comment: node.byte_range(),
                        },
                    };
                    let id = result.commands.len();
                    let timeline = result.timelines.entry(rule.into()).or_default();
                    if command.scoped {
                        timeline.lines.insert(start.row, id);
                    } else {
                        timeline.regions.push((start, id));
                    }
                    result.commands.push(command);
                }
            }
        }
        result
    }

    fn disabling(&self, rule: &str, at: Point) -> Option<usize> {
        let id = self.timelines.get(rule)?.command(at)?;
        self.commands[id].disable.then_some(id)
    }

    pub(super) fn scoped_rows(&self) -> BTreeSet<usize> {
        self.commands
            .iter()
            .filter(|cmd| cmd.scoped)
            .flat_map(|cmd| {
                let comment = cmd.location.row;
                let target = cmd.start.row;
                comment.min(target)..=comment.max(target)
            })
            .collect()
    }

    fn blanket(&self, out: &mut Findings) {
        let mut open = HashMap::new();
        for cmd in &self.commands {
            let previous = open.contains_key(cmd.rule.as_str());
            if previous == cmd.disable {
                let state = if previous { "disabled" } else { "enabled" };
                out.push(
                    cmd.location,
                    "blanket_disable_command",
                    format!("Rule '{}' was already {state}", cmd.rule),
                );
            }
            if !cmd.scoped {
                if cmd.disable {
                    open.insert(cmd.rule.as_str(), cmd);
                } else {
                    open.remove(cmd.rule.as_str());
                }
            }
        }
        for (rule, cmd) in open {
            if !matches!(
                rule,
                "file_length"
                    | "file_name"
                    | "file_header"
                    | "file_name_no_space"
                    | "single_test_class"
            ) {
                out.push(
                    cmd.location,
                    "blanket_disable_command",
                    format!("Limit the disable for '{rule}' to a line or re-enable it after use"),
                );
            }
        }
    }

    // Apply suppression after semantic and syntax findings have been combined.
    // A directive that hides an indexed diagnostic counts as used too.
    pub fn apply(&self, out: &mut Findings, fix_source: Option<&str>) {
        if self.commands.is_empty() {
            return;
        }
        if out.selection.enabled("blanket_disable_command") {
            self.blanket(out);
        }
        let mut used = HashSet::new();
        out.items.retain(|finding| {
            let one = self.disabling(finding.rule, finding.point);
            let all = self.disabling("all", finding.point);
            used.extend(one);
            one.is_none() && all.is_none()
        });
        if !out.selection.enabled("superfluous_disable_command") {
            return;
        }
        for (id, cmd) in self.commands.iter().enumerate() {
            if cmd.disable
                && out.selection.enabled(&cmd.rule)
                && !matches!(cmd.rule.as_str(), "all" | "superfluous_disable_command")
                && super::identifiers().any(|id| id == cmd.rule)
                && !used.contains(&id)
                && self.disabling("all", cmd.start).is_none()
                && self
                    .disabling("superfluous_disable_command", cmd.start)
                    .is_none()
            {
                out.push_with_fix(
                    cmd.location,
                    "superfluous_disable_command",
                    format!(
                        "Rule '{}' had no violations in the disabled region",
                        cmd.rule
                    ),
                    fix_source.and_then(|source| cmd.text.remove(source, &cmd.rule)),
                );
            }
        }
    }
}
