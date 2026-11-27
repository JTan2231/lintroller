use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::{Context, Findings, Fix};
use crate::index::ImportUsage;

struct Import<'a> {
    node: Node<'a>,
    module: &'a str,
    protected: bool,
}

fn import<'a>(cx: &Context<'a>, node: Node<'a>) -> Option<Import<'a>> {
    let mut cursor = node.walk();
    let path = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "identifier")?;
    let module = path.named_child(0)?;
    let scoped = path.named_child_count() > 1;
    let reexports = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "modifiers")
        .any(|mods| {
            let mut cursor = mods.walk();
            mods.named_children(&mut cursor).any(|m| {
                m.kind() == "attribute"
                    && m.named_child(0).is_some_and(|n| cx.text(n) == "_exported")
            })
        });
    Some(Import {
        node,
        module: cx.text(module).trim_matches('`'),
        protected: scoped || reexports,
    })
}

fn location(node: Node<'_>) -> ((u32, u32), (u32, u32)) {
    let start = node.start_position();
    let end = node.end_position();
    (
        (start.row as u32 + 1, start.column as u32 + 1),
        (end.row as u32 + 1, end.column as u32 + 1),
    )
}

fn implicit_module(module: &str) -> bool {
    matches!(
        module,
        "Builtin"
            | "Swift"
            | "SwiftShims"
            | "SwiftOnoneSupport"
            | "_Concurrency"
            | "_StringProcessing"
            | "_SwiftConcurrencyShims"
    )
}

fn facade(module: &str) -> Option<&'static str> {
    // Public modules whose APIs have been split into implementation modules.
    match module {
        "FoundationEssentials" | "FoundationInternationalization" => Some("Foundation"),
        "SwiftUICore" => Some("SwiftUI"),
        _ => None,
    }
}

fn remove_plain_import(cx: &Context<'_>, item: &Import<'_>) -> Option<Fix> {
    let node = item.node;
    let row = node.start_position().row;
    let line_start = cx.line_starts[row];
    let before = &cx.source[line_start..node.start_byte()];
    let name = cx.text(node).strip_prefix("import")?.trim();
    if !before.trim().is_empty()
        || row != node.end_position().row
        || name.trim_matches('`') != item.module
    {
        return None;
    }
    let line_end = line_start + cx.lines[row].len();
    let after = &cx.source[node.end_byte()..line_end];
    let suffix = after.trim_start();
    let suffix_start = line_end - suffix.len();
    if suffix.starts_with("//") {
        return Some(Fix::replace(node.start_byte()..suffix_start, ""));
    }
    if suffix.starts_with("/*") {
        let mut end = node.end_byte();
        for comment in
            std::iter::successors(node.next_named_sibling(), |node| node.next_named_sibling())
        {
            if !matches!(comment.kind(), "comment" | "multiline_comment")
                || comment.start_position().row != row
                || comment.end_position().row != row
                || !cx.source[end..comment.start_byte()].trim().is_empty()
            {
                break;
            }
            end = comment.end_byte() - usize::from(cx.text(comment).ends_with('\r'));
        }
        if end < suffix_start || !cx.source[end..line_end].trim().is_empty() {
            return None;
        }
        return Some(Fix::replace(node.start_byte()..suffix_start, ""));
    }
    if !suffix.is_empty() && suffix != ";" {
        return None;
    }
    // Keep a line in place when a nearby scoped directive may target it.
    let nearby_directive = (row.saturating_sub(1)..=row + 1).any(|line| {
        cx.lines
            .get(line)
            .is_some_and(|text| text.contains("swiftlint:"))
    });
    let end = if nearby_directive {
        line_end
    } else {
        cx.line_starts
            .get(row + 1)
            .copied()
            .unwrap_or(cx.source.len())
    };
    Some(Fix::replace(line_start..end, ""))
}

pub(super) fn check(cx: &Context<'_>, usage: &ImportUsage, out: &mut Findings) {
    let mut imports = Vec::new();
    let mut requires_foundation = false;
    for &node in &cx.nodes {
        match node.kind() {
            "import_declaration" => {
                if let Some(item) = import(cx, node) {
                    imports.push(item);
                }
            }
            "attribute" if !requires_foundation => {
                requires_foundation = node.named_child(0).is_some_and(|n| {
                    matches!(
                        cx.text(n),
                        "objc" | "objcMembers" | "_objcNonLazyRealization"
                    )
                });
            }
            _ => {}
        }
    }
    if imports.is_empty() {
        return;
    }

    let mut active = Vec::new();
    let mut ranges: HashMap<&str, Vec<_>> = HashMap::new();
    for item in imports {
        let (begin, end) = location(item.node);
        if usage
            .module_locations
            .get(item.module)
            .is_some_and(|points| points.range(begin..end).next().is_some())
        {
            ranges.entry(item.module).or_default().push((begin, end));
            active.push(item);
        }
    }
    // References at an import's own location establish which #if branch was
    // compiled. A reference elsewhere (Foo.member) actually uses the module.
    let mut used = HashSet::new();
    for (module, ranges) in &ranges {
        if let Some(points) = usage.module_locations.get(*module) {
            let mut remaining = ranges.iter().peekable();
            for point in points {
                while remaining.peek().is_some_and(|(_, end)| end <= point) {
                    remaining.next();
                }
                if remaining.peek().is_none_or(|(begin, _)| point < begin) {
                    used.insert(*module);
                    break;
                }
            }
        }
    }

    let mut uncertain = usage.unknown_owner;
    for module in &usage.referenced_modules {
        if ranges.contains_key(module.as_str()) {
            used.insert(module.as_str());
        } else if let Some(public) = facade(module).filter(|public| ranges.contains_key(public)) {
            used.insert(public);
        } else if !implicit_module(module) && !usage.current_modules.contains(module) {
            // A referenced external module with no direct import may have
            // reached this file through a reexport. The index omits that edge.
            uncertain = true;
        }
    }
    for item in active {
        if item.protected || item.module == "Foundation" && requires_foundation {
            continue;
        }
        let always_available = matches!(item.module, "Swift" | "SwiftShims");
        if always_available || !uncertain && !used.contains(item.module) {
            let fix = out
                .fixing()
                .then(|| remove_plain_import(cx, &item))
                .flatten();
            out.at_with_fix(
                item.node,
                "unused_import",
                format!("Unused import of '{}'", item.module),
                fix,
            );
        }
    }
}
