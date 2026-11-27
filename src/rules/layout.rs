use super::syntax::{ancestors, child, declaration_keyword, named_children};
use super::{Context, Findings};
use tree_sitter::{Node, Point};
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn check(cx: &Context<'_>, out: &mut Findings) {
    let measure_bodies = [
        "type_body_length",
        "function_body_length",
        "closure_body_length",
    ]
    .iter()
    .any(|rule| cx.enabled(rule));
    let comments: Vec<_> = if measure_bodies || cx.enabled("orphaned_doc_comment") {
        cx.nodes
            .iter()
            .copied()
            .filter(|n| is_comment(*n))
            .collect()
    } else {
        Vec::new()
    };
    let code_lines = if measure_bodies {
        code_line_prefix(cx, &comments)
    } else {
        Vec::new()
    };

    if cx.enabled("orphaned_doc_comment") {
        doc_comments(cx, &comments, out);
    }
    if cx.enabled("line_length") || cx.enabled("file_length") {
        file_metrics(cx, out);
    }
    let check_indent = cx.enabled("indentation_width");
    let mut condition_continuations = vec![false; if check_indent { cx.lines.len() } else { 0 }];
    for &node in &cx.nodes {
        match node.kind() {
            "class_declaration"
            | "protocol_declaration"
            | "typealias_declaration"
            | "associatedtype_declaration"
            | "function_declaration"
            | "protocol_function_declaration" => {
                if cx.enabled("nesting") {
                    nesting(node, out);
                }
            }
            _ => {}
        }
        match node.kind() {
            "class_declaration" if cx.enabled("type_body_length") => {
                if let (Some(body), Some(keyword)) = (
                    node.child_by_field_name("body"),
                    node.child_by_field_name("declaration_kind"),
                ) && keyword.kind() != "extension"
                {
                    body_length(
                        body,
                        keyword,
                        &code_lines,
                        "type_body_length",
                        "Type",
                        250,
                        out,
                    );
                }
            }
            "function_body" => {
                if let Some(owner) = node.parent() {
                    let keyword = declaration_keyword(owner);
                    if cx.enabled("function_body_length") {
                        body_length(
                            node,
                            keyword,
                            &code_lines,
                            "function_body_length",
                            "Function",
                            50,
                            out,
                        );
                    }
                    if cx.enabled("cyclomatic_complexity") && owner.kind() != "deinit_declaration" {
                        complexity(cx, node, keyword, out);
                    }
                }
            }
            "computed_property" | "computed_getter" | "computed_setter" | "computed_modify"
                if cx.enabled("function_body_length") =>
            {
                let property = if node.kind() == "computed_property" {
                    Some(node)
                } else {
                    node.parent()
                };
                if let Some(owner) = property.and_then(|p| p.parent())
                    && owner.kind() == "subscript_declaration"
                    && (node.kind() != "computed_property" || child(node, "statements").is_some())
                {
                    let keyword = if node.kind() == "computed_property" {
                        declaration_keyword(owner)
                    } else {
                        node
                    };
                    body_length(
                        node,
                        keyword,
                        &code_lines,
                        "function_body_length",
                        "Subscript accessor",
                        50,
                        out,
                    );
                }
            }
            "lambda_literal" => {
                if cx.enabled("closure_body_length") {
                    body_length(
                        node,
                        node,
                        &code_lines,
                        "closure_body_length",
                        "Closure",
                        30,
                        out,
                    );
                }
                if cx.enabled("closure_end_indentation") {
                    closure_indentation(cx, node, out);
                }
            }
            "lambda_parameter" | "capture_list_item"
                if cx.enabled("closure_parameter_position") =>
            {
                if let Some(closure) = ancestors(node).find(|n| n.kind() == "lambda_literal") {
                    let token = if node.kind() == "capture_list_item" {
                        node.child_by_field_name("name").unwrap_or(node)
                    } else {
                        node
                    };
                    if token.start_position().row != closure.start_position().row {
                        out.at(
                            token,
                            "closure_parameter_position",
                            "Place closure parameters and captures on the opening brace line",
                        );
                    }
                }
            }
            "if_statement" | "guard_statement" | "while_statement" => {
                if check_indent {
                    mark_condition_lines(node, &mut condition_continuations);
                }
                if cx.enabled("conditional_returns_on_newline") && node.kind() != "while_statement"
                {
                    conditional_return(node, out);
                }
            }
            "value_arguments"
                if cx.enabled("multiline_arguments") || cx.enabled("multiline_call_arguments") =>
            {
                call_arguments(cx, node, out);
            }
            "call_expression" if cx.enabled("multiline_function_chains") => {
                function_chain(cx, node, out)
            }
            "source_file" | "class_body" | "enum_class_body" | "protocol_body"
                if cx.enabled("let_var_whitespace") =>
            {
                binding_whitespace(cx, node, out)
            }
            "switch_entry" if cx.enabled("switch_case_on_newline") => {
                if let (Some(colon), Some(stmts)) = (child(node, ":"), child(node, "statements"))
                    && let Some(first) = first_item(stmts)
                    && first.start_position().row == colon.end_position().row
                {
                    out.at(
                        node,
                        "switch_case_on_newline",
                        "Start the switch case body on a new line",
                    );
                }
            }
            "integer_literal" | "real_literal" if cx.enabled("number_separator") => {
                number_separator(cx, node, out);
            }
            "array_literal" | "dictionary_literal"
                if cx.enabled("multiline_literal_brackets") || cx.enabled("trailing_comma") =>
            {
                collection_literal(cx, node, out);
            }
            _ => {}
        }
        if cx.enabled("opening_brace") && has_braced_body(node) {
            opening_braces(cx, node, out);
        }
    }
    if check_indent {
        indentation(cx, &condition_continuations, out);
    }
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "comment" | "multiline_comment")
}

fn first_item(node: Node<'_>) -> Option<Node<'_>> {
    named_children(node).find(|n| !is_comment(*n))
}

fn nesting(node: Node<'_>, out: &mut Findings) {
    let function = matches!(
        node.kind(),
        "function_declaration" | "protocol_function_declaration"
    );
    let limit = if function { 2 } else { 1 };
    let count = ancestors(node)
        .filter(|n| {
            if function {
                matches!(
                    n.kind(),
                    "function_declaration" | "protocol_function_declaration"
                )
            } else {
                matches!(n.kind(), "class_declaration" | "protocol_declaration")
            }
        })
        .take(limit + 1)
        .count();
    if count > limit {
        let message = if function {
            "Nest functions at most two levels deep"
        } else {
            "Nest types at most one level deep"
        };
        out.at(declaration_keyword(node), "nesting", message);
    }
}

// Count source lines once so overlapping type, method and closure bodies cost O(1) each.
fn code_line_prefix(cx: &Context<'_>, comments: &[Node<'_>]) -> Vec<usize> {
    let mut prefix = Vec::with_capacity(cx.lines.len() + 1);
    // A blank physical line inside a multiline string is still part of a token.
    let mut string_lines = vec![false; cx.lines.len()];
    for node in cx.nodes.iter().filter(|n| {
        matches!(
            n.kind(),
            "multi_line_str_text" | "raw_str_part" | "raw_str_end_part"
        )
    }) {
        let start = node.start_position().row;
        let end = (node.end_position().row + 1).min(string_lines.len());
        if let Some(lines) = string_lines.get_mut(start..end) {
            lines.fill(true);
        }
    }
    let mut comment_index = 0;
    prefix.push(0);
    for (row, line) in cx.lines.iter().enumerate() {
        let start = cx.line_starts[row];
        let end = start + line.len();
        let mut position = start;
        let mut code = string_lines[row];
        while let Some(comment) = comments.get(comment_index) {
            if comment.start_byte() >= end {
                break;
            }
            if comment.end_byte() <= start {
                comment_index += 1;
                continue;
            }
            if position < comment.start_byte() {
                code |= !cx.source[position..comment.start_byte()].trim().is_empty();
            }
            position = position.max(comment.end_byte().min(end));
            if comment.end_byte() > end {
                break;
            }
            comment_index += 1;
        }
        code |= !cx.source[position..end].trim().is_empty();
        prefix.push(prefix[row] + usize::from(code));
    }
    prefix
}

fn body_length(
    body: Node<'_>,
    keyword: Node<'_>,
    prefix: &[usize],
    rule: &'static str,
    name: &str,
    limit: usize,
    out: &mut Findings,
) {
    if body
        .end_position()
        .row
        .saturating_sub(body.start_position().row)
        <= limit
    {
        return;
    }
    let Some(open) = child(body, "{") else { return };
    let Some(close) = child(body, "}") else {
        return;
    };
    let start = open.start_position().row + 1;
    let end = close.start_position().row;
    if end <= start || end >= prefix.len() {
        return;
    }
    let lines = prefix[end] - prefix[start];
    if lines > limit {
        out.at(
            keyword,
            rule,
            format!("{name} body spans {lines} lines of code (limit {limit})"),
        );
    }
}

fn character_count(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.graphemes(true).count()
    }
}

fn file_metrics(cx: &Context<'_>, out: &mut Findings) {
    // A final newline terminates the last physical line.
    let count = cx.lines.len();
    if count > 400 && cx.enabled("file_length") {
        out.push(
            Point::new(count - 1, 0),
            "file_length",
            format!("File spans {count} lines (limit 400)"),
        );
    }
    if cx.enabled("line_length") {
        for (row, line) in cx.lines.iter().enumerate().filter(|(_, l)| l.len() > 120) {
            let mut length = character_count(line);
            for delimiter in ["#colorLiteral(", "#imageLiteral("] {
                let mut remaining = *line;
                while let Some((_, rest)) = remaining.split_once(delimiter) {
                    let Some((arguments, tail)) = rest.split_once(')') else {
                        break;
                    };
                    length = length.saturating_sub(delimiter.len() + character_count(arguments));
                    remaining = tail;
                }
            }
            if length > 120 {
                out.push(
                    Point::new(row, 0),
                    "line_length",
                    format!("Line has {length} characters (limit 120)"),
                );
            }
        }
    }
}

fn mark_condition_lines(node: Node<'_>, skip: &mut [bool]) {
    let mut cursor = node.walk();
    let mut multiple_conditions = false;
    let mut last = node.start_position().row;
    for token in node
        .children(&mut cursor)
        .take_while(|n| !matches!(n.kind(), "{" | "else"))
    {
        multiple_conditions |= token.kind() == ",";
        if !is_comment(token) {
            last = token.end_position().row;
        }
    }
    let start = node.start_position().row + 1;
    if multiple_conditions && let Some(lines) = skip.get_mut(start..=last) {
        lines.fill(true);
    }
}

fn indentation(cx: &Context<'_>, skip: &[bool], out: &mut Findings) {
    let mut prior = Vec::new();
    for (row, &line) in cx.lines.iter().enumerate() {
        if skip[row] || line.trim().is_empty() {
            continue;
        }
        let mut spaces = 0;
        let mut tabs = 0;
        for byte in line.bytes() {
            match byte {
                b' ' => spaces += 1,
                b'\t' => tabs += 1,
                _ => break,
            }
        }
        let width: usize = spaces + tabs * 4;
        let valid = |&previous: &usize| {
            width == previous + 4 || (width <= previous && (previous - width).is_multiple_of(4))
        };
        let message = if tabs != 0 && spaces != 0 {
            Some("Use either tabs or spaces for indentation on a line")
        } else if prior.is_empty() && width != 0 {
            Some("Leave the first line unindented")
        } else if !prior.is_empty() && !prior.iter().any(valid) {
            Some("Indent by one tab or four spaces and unindent in matching increments")
        } else {
            None
        };
        if let Some(message) = message {
            out.push(Point::new(row, 0), "indentation_width", message);
        }
        if prior.first().is_some_and(valid) {
            prior.clear();
        }
        prior.push(width);
    }
}

fn number_separator(cx: &Context<'_>, node: Node<'_>, out: &mut Findings) {
    if node
        .parent()
        .is_some_and(|p| matches!(p.kind(), "navigation_suffix" | "type_arguments"))
    {
        return;
    }
    let text = cx.text(node);
    if text.starts_with("0x") || text.starts_with("0X") {
        return;
    }
    let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut groups = integer.split('_');
    let first = groups.next().unwrap_or("");
    if first.is_empty() || first.len() > 3 || groups.any(|g| g.len() != 3) || fraction.contains('_')
    {
        out.at(
            node,
            "number_separator",
            "Group the integral digits in threes using underscores",
        );
    }
}

fn complexity(cx: &Context<'_>, body: Node<'_>, keyword: Node<'_>, out: &mut Findings) {
    let mut complexity = 0i32;
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "function_declaration" | "init_declaration" => continue,
            "for_statement"
            | "if_statement"
            | "guard_statement"
            | "repeat_while_statement"
            | "while_statement"
            | "catch_block" => complexity += 1,
            "simple_identifier"
                if cx.text(node) == "fallthrough"
                    && node.parent().is_some_and(|n| n.kind() == "statements") =>
            {
                complexity -= 1
            }
            "switch_entry" => {
                if child(node, "fallthrough").is_none() {
                    complexity += 1;
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    if complexity > 10 {
        let anchor = body
            .parent()
            .and_then(|parent| child(parent, "modifiers"))
            .and_then(|mods| {
                let mut cursor = mods.walk();
                mods.named_children(&mut cursor).find(|modifier| {
                    modifier.kind() == "property_modifier"
                        && (child(*modifier, "static").is_some()
                            || child(*modifier, "class").is_some())
                })
            })
            .unwrap_or(keyword);
        out.at(
            anchor,
            "cyclomatic_complexity",
            format!("Function has cyclomatic complexity {complexity} (limit 10)"),
        );
    }
}

fn conditional_return(node: Node<'_>, out: &mut Findings) {
    let mut keyword_row = node.start_position().row;
    let mut cursor = node.walk();
    for part in node.children(&mut cursor) {
        if node.kind() == "if_statement" && part.kind() == "else" {
            keyword_row = part.start_position().row;
        }
        if part.kind() != "statements" {
            continue;
        }
        let mut items = part.walk();
        let last = part
            .named_children(&mut items)
            .filter(|n| !is_comment(*n))
            .last();
        if let Some(last) = last
            && last.kind() == "control_transfer_statement"
            && child(last, "return").is_some()
            && last.start_position().row == keyword_row
        {
            out.at(
                node,
                "conditional_returns_on_newline",
                "Put a conditional return on a new line",
            );
            break;
        }
    }
}

fn binding_whitespace(cx: &Context<'_>, node: Node<'_>, out: &mut Findings) {
    let variable = |n: Node<'_>| {
        matches!(
            n.kind(),
            "property_declaration" | "protocol_property_declaration"
        )
    };
    let mut previous: Option<Node<'_>> = None;
    let mut cursor = node.walk();
    for item in node.named_children(&mut cursor).filter(|n| !is_comment(*n)) {
        if let Some(prev) = previous
            && variable(prev) != variable(item)
            && !matches!(prev.kind(), "macro_invocation" | "shebang_line")
            && !matches!(item.kind(), "macro_invocation" | "shebang_line")
            && cx.source[prev.end_byte()..item.start_byte()]
                .bytes()
                .filter(|&b| b == b'\n')
                .take(2)
                .count()
                < 2
        {
            out.at(
                item,
                "let_var_whitespace",
                "Separate variables from other declarations with a blank line",
            );
        }
        previous = Some(item);
    }
}

fn collection_literal(cx: &Context<'_>, node: Node<'_>, out: &mut Findings) {
    let Some(close) = child(node, "]") else {
        return;
    };
    if cx.enabled("trailing_comma") {
        let previous = std::iter::successors(close.prev_sibling(), |n| n.prev_sibling())
            .find(|n| !is_comment(*n));
        if let Some(comma) = previous.filter(|n| n.kind() == ",") {
            out.at(
                comma,
                "trailing_comma",
                "Remove the trailing comma from the collection literal",
            );
        }
    }
    if !cx.enabled("multiline_literal_brackets")
        || node.start_position().row == close.start_position().row
    {
        return;
    }
    let mut cursor = node.walk();
    let mut elements = node.named_children(&mut cursor).filter(|n| !is_comment(*n));
    let Some(first) = elements.next() else { return };
    let last = elements.last().unwrap_or(first);
    if node.start_position().row == first.start_position().row {
        out.at(
            first,
            "multiline_literal_brackets",
            "Start multiline literal elements on a new line",
        );
    }
    if last.end_position().row == close.start_position().row {
        out.at(
            close,
            "multiline_literal_brackets",
            "Put the closing bracket of a multiline literal on a new line",
        );
    }
}

fn has_braced_body(node: Node<'_>) -> bool {
    match node.kind() {
        "class_body"
        | "enum_class_body"
        | "protocol_body"
        | "if_statement"
        | "guard_statement"
        | "for_statement"
        | "while_statement"
        | "repeat_while_statement"
        | "do_statement"
        | "catch_block"
        | "switch_statement"
        | "computed_getter"
        | "computed_setter"
        | "computed_modify"
        | "willset_didset_block"
        | "willset_clause"
        | "didset_clause"
        | "protocol_property_requirements"
        | "precedence_group_declaration" => true,
        "function_body" => node
            .parent()
            .is_some_and(|p| p.kind() != "deinit_declaration"),
        "computed_property" => node
            .parent()
            .is_some_and(|p| p.kind() == "property_declaration"),
        "lambda_literal" => node
            .parent()
            .is_some_and(|p| matches!(p.kind(), "call_suffix" | "constructor_suffix")),
        _ => false,
    }
}

fn opening_braces(cx: &Context<'_>, node: Node<'_>, out: &mut Findings) {
    let mut cursor = node.walk();
    for brace in node
        .children(&mut cursor)
        .filter(|n| n.kind() == "{" && !n.is_missing())
    {
        // The brace can be the first child of a body node. In that case its predecessor
        // belongs to the enclosing declaration or call.
        let mut previous = brace;
        let mut last_comment = None;
        let prev_token = loop {
            if let Some(sibling) = previous.prev_sibling() {
                previous = sibling;
                while let Some(last) = previous
                    .child_count()
                    .checked_sub(1)
                    .and_then(|i| previous.child(i as u32))
                {
                    previous = last;
                }
                if !is_comment(previous) {
                    break Some(previous);
                }
                last_comment.get_or_insert(previous);
            } else if let Some(parent) = previous.parent() {
                previous = parent;
            } else {
                break None;
            }
        };
        if let Some(prev) = prev_token {
            let before = last_comment.unwrap_or(prev);
            if prev.end_position().row != brace.start_position().row
                || &cx.source[before.end_byte()..brace.start_byte()] != " "
            {
                out.at(brace, "opening_brace", "Place the opening brace on the preceding token’s line with one space before it");
            }
        }
    }
}

fn doc_comments(cx: &Context<'_>, comments: &[Node<'_>], out: &mut Findings) {
    let mut pending = None;
    let mut previous_end = 0;
    let mut previous_row = 0;
    let report = |location: Node<'_>, out: &mut Findings| {
        out.at(
            location,
            "orphaned_doc_comment",
            "Attach the documentation comment to its declaration",
        );
    };
    for &comment in comments {
        if comment.start_byte() < previous_end {
            continue;
        }
        if !cx.source[previous_end..comment.start_byte()]
            .trim()
            .is_empty()
        {
            pending = None;
        }
        previous_end = comment.end_byte();
        let content = cx.text(comment);
        if content.starts_with("////") || content.starts_with("/***") {
            continue;
        }
        let doc = content.starts_with("///") || content.starts_with("/**");
        let row = comment.start_position().row;
        if (!doc || row > previous_row + 1)
            && let Some(orphan) = pending.take()
        {
            report(orphan, out);
        }
        if doc
            && let Some(&start) = cx.line_starts.get(row)
            && cx.source[start..comment.start_byte()].trim().is_empty()
        {
            pending.get_or_insert(comment);
        }
        previous_row = comment.end_position().row;
    }
    if let Some(orphan) = pending
        && cx.source[previous_end..].trim().is_empty()
    {
        report(orphan, out);
    }
}

fn call_arguments(cx: &Context<'_>, node: Node<'_>, out: &mut Findings) {
    if child(node, "(").is_none()
        || !node
            .parent()
            .is_some_and(|p| matches!(p.kind(), "call_suffix" | "constructor_suffix"))
    {
        return;
    }
    let mut cursor = node.walk();
    let arguments: Vec<_> = node
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "value_argument")
        .collect();
    if arguments.len() < 2
        || arguments.first().unwrap().start_position().row
            == arguments.last().unwrap().start_position().row
    {
        return;
    }
    let call_rule = cx.enabled("multiline_call_arguments")
        && !ancestors(node)
            .take_while(|n| {
                !matches!(n.kind(), "statements") && !n.kind().ends_with("_declaration")
            })
            .any(|n| matches!(n.kind(), "switch_pattern" | "pattern"));
    for pair in arguments.windows(2) {
        let previous = pair[0];
        let next = pair[1];
        let same_line = previous.start_position().row == next.start_position().row;
        if same_line && cx.enabled("multiline_arguments") {
            out.at(
                next,
                "multiline_arguments",
                "Place each argument of a multiline call on its own line",
            );
        }
        if call_rule {
            let comma_on_next_line = !cx.text(previous).ends_with(['}', ']'])
                && std::iter::successors(previous.next_sibling(), |n| n.next_sibling())
                    .take_while(|n| n.start_byte() < next.start_byte())
                    .any(|n| {
                        n.kind() == "," && n.start_position().row == next.start_position().row
                    });
            if same_line || comma_on_next_line {
                out.at(
                    next,
                    "multiline_call_arguments",
                    "Begin the next argument on a new line after the comma",
                );
            }
        }
    }
}

fn receiver(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "call_expression" | "constructor_expression" => first_item(node),
        "navigation_expression" | "postfix_expression" => node.child_by_field_name("target"),
        _ => None,
    }
}

fn function_chain(cx: &Context<'_>, call: Node<'_>, out: &mut Findings) {
    // Each receiver is also traversed by the file-wide walk. Inspect only the outer call.
    let mut current = call;
    while let Some(parent) = current.parent() {
        if receiver(parent) != Some(current) {
            break;
        }
        if parent.kind() == "call_expression" {
            return;
        }
        current = parent;
    }
    let mut methods = Vec::new();
    let mut current = call;
    while let Some(base) = receiver(current) {
        if current.kind() == "call_expression"
            && base.kind() == "navigation_expression"
            && let Some(dot) = base.child_by_field_name("suffix")
        {
            methods.push(dot);
        }
        current = base;
    }
    let Some(first) = methods.pop() else { return };
    if methods
        .iter()
        .all(|n| n.start_position().row == first.start_position().row)
    {
        return;
    }
    for dot in methods {
        let row = dot.start_position().row;
        if let Some(&start) = cx.line_starts.get(row)
            && !cx.source[start..dot.start_byte()].trim().is_empty()
        {
            out.at(
                dot,
                "multiline_function_chains",
                "Put each chained call on its own line in a multiline chain",
            );
        }
    }
}

fn closure_indentation(cx: &Context<'_>, closure: Node<'_>, out: &mut Findings) {
    let Some(close) = child(closure, "}") else {
        return;
    };
    if close.start_position().row == closure.start_position().row {
        return;
    }
    let Some(parent) = closure.parent() else {
        return;
    };
    let anchor = match parent.kind() {
        "value_argument" => {
            if let Some(arguments) = parent.parent()
                && arguments.kind() == "value_arguments"
                && let Some(first) = child(arguments, "value_argument")
                && first.start_position().row == arguments.start_position().row
            {
                return;
            }
            parent
        }
        "call_suffix" | "constructor_suffix" => {
            if let Some(colon) = closure.prev_sibling().filter(|n| n.kind() == ":") {
                colon.prev_named_sibling().unwrap_or(closure)
            } else {
                let mut current = parent.parent().unwrap_or(closure);
                while let Some(base) = receiver(current) {
                    if current.kind() == "navigation_expression"
                        && let Some(suffix) = current.child_by_field_name("suffix")
                        && suffix.start_position().row > base.end_position().row
                    {
                        current = suffix;
                        break;
                    }
                    current = base;
                }
                current
            }
        }
        _ => closure,
    };
    if let Some(line) = cx.lines.get(anchor.start_position().row) {
        let expected = line
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        if expected != close.start_position().column {
            out.at(
                close,
                "closure_end_indentation",
                format!("Align the closing brace at column {}", expected + 1),
            );
        }
    }
}
