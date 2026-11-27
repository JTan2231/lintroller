use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

use super::syntax::{ancestors, child, children, syntax_children};
use super::{Context, Findings};

fn owner(node: Node<'_>) -> Option<Node<'_>> {
    ancestors(node).find(|n| matches!(n.kind(), "class_declaration" | "protocol_declaration"))
}

fn field<'a>(cx: &'a Context<'_>, node: Node<'_>, name: &str) -> &'a str {
    node.child_by_field_name(name).map_or("", |n| cx.text(n))
}

fn has_modifier(cx: &Context<'_>, node: Node<'_>, name: &str) -> bool {
    child(node, "modifiers").is_some_and(|mods| syntax_children(mods).any(|m| cx.text(m) == name))
}

fn has_attribute(cx: &Context<'_>, node: Node<'_>, names: &[&str]) -> bool {
    let is_attribute = |n: Node<'_>| {
        n.kind() == "attribute"
            && child(n, "user_type").is_some_and(|name| names.contains(&cx.text(name)))
    };
    syntax_children(node).any(|n| {
        is_attribute(n) || (n.kind() == "modifiers" && syntax_children(n).any(is_attribute))
    })
}

fn inherits(cx: &Context<'_>, node: Node<'_>, names: &[&str]) -> bool {
    syntax_children(node).any(|n| {
        n.kind() == "inheritance_specifier" && names.contains(&field(cx, n, "inherits_from"))
    })
}

fn member_scope(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "class_body" | "enum_class_body" | "protocol_body"
    )
}

fn suffix(call: Node<'_>) -> Option<Node<'_>> {
    syntax_children(call).find(|n| matches!(n.kind(), "call_suffix" | "constructor_suffix"))
}

fn callee(call: Node<'_>) -> Option<Node<'_>> {
    call.child_by_field_name("constructed_type")
        .or_else(|| syntax_children(call).next())
}

fn base_call(call: Node<'_>) -> Option<Node<'_>> {
    let target = callee(call)?.child_by_field_name("target")?;
    is_call(target).then_some(target)
}

fn call_name<'a>(cx: &'a Context<'_>, call: Node<'_>) -> &'a str {
    let Some(expr) = callee(call) else { return "" };
    match expr.kind() {
        "user_type" | "simple_identifier" => cx.text(expr).rsplit('.').next().unwrap_or(""),
        "navigation_expression" => expr
            .child_by_field_name("suffix")
            .map_or("", |n| field(cx, n, "suffix")),
        _ => "",
    }
}

fn is_call(node: Node<'_>) -> bool {
    matches!(node.kind(), "call_expression" | "constructor_expression")
}

fn args(call: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    suffix(call)
        .and_then(|n| child(n, "value_arguments"))
        .into_iter()
        .flat_map(syntax_children)
        .filter(|n| n.kind() == "value_argument")
}

fn arg_value<'t>(cx: &Context<'_>, call: Node<'t>, label: &str) -> Option<Node<'t>> {
    args(call)
        .find(|n| field(cx, *n, "name") == label)?
        .child_by_field_name("value")
}

fn default_value(parameter: Node<'_>) -> Option<Node<'_>> {
    let equals = std::iter::successors(parameter.next_sibling(), |n| n.next_sibling())
        .find(|n| !n.is_extra())?;
    (equals.kind() == "=")
        .then(|| {
            std::iter::successors(equals.next_named_sibling(), |n| n.next_named_sibling())
                .find(|n| !n.is_extra())
        })
        .flatten()
}

pub(super) fn check(cx: &Context<'_>, out: &mut Findings) {
    let mut scopes = Scopes::default();
    let magic = cx.enabled("no_magic_numbers");
    let shadowing = cx.enabled("variable_shadowing");
    let self_rule = cx.enabled("prefer_self_in_static_references");
    let mut self_types = SelfTypes::default();
    let mut test_types = HashSet::new();
    if magic || self_rule {
        for &node in &cx.nodes {
            if node.kind() != "class_declaration" {
                continue;
            }
            if self_rule {
                self_types.add(cx, node);
            }
            if magic
                && (inherits(cx, node, &["XCTestCase", "QuickSpec"])
                    || has_attribute(cx, node, &["Suite"])
                    || node.child_by_field_name("body").is_some_and(|body| {
                        syntax_children(body).any(|n| {
                            n.kind() == "function_declaration" && has_attribute(cx, n, &["Test"])
                        })
                    }))
            {
                test_types.insert(field(cx, node, "name"));
            }
        }
    }
    for &node in &cx.nodes {
        if shadowing {
            scopes.visit(cx, out, node);
        }
        match node.kind() {
            "simple_identifier" | "type_identifier" => identifier(cx, out, node, &self_types),
            "integer_literal" | "real_literal" | "hex_literal" if magic => {
                magic_number(cx, out, node, &test_types)
            }
            "function_declaration" | "protocol_function_declaration" | "init_declaration" => {
                parameters(cx, out, node)
            }
            "call_expression" | "constructor_expression" => {
                closure_arguments(out, node);
                accessibility(cx, out, node);
            }
            "lambda_literal" => {
                if cx.enabled("prefer_key_path") {
                    key_path(cx, out, node);
                }
                empty_block(out, node);
            }
            "function_body"
            | "computed_getter"
            | "computed_setter"
            | "computed_modify"
            | "willset_clause"
            | "didset_clause"
            | "do_statement"
            | "catch_block"
            | "for_statement"
            | "repeat_while_statement" => empty_block(out, node),
            "if_statement" | "while_statement" | "guard_statement" => {
                if node.kind() != "guard_statement" {
                    empty_block(out, node);
                }
                conditions(out, node);
            }
            "statements" => implicit_return(out, node),
            "switch_pattern" => optional_case(cx, out, node),
            "property_declaration" => private_state(cx, out, node),
            "class_declaration"
                if cx.enabled("unneeded_synthesized_initializer")
                    && field(cx, node, "declaration_kind") == "struct" =>
            {
                synthesized_init(cx, out, node)
            }
            _ => {}
        }
    }
}

fn identifier(cx: &Context<'_>, out: &mut Findings, node: Node<'_>, self_types: &SelfTypes<'_>) {
    let name = cx.text(node);
    if name
        .strip_prefix('$')
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && ancestors(node)
            .find(|n| n.kind() == "lambda_literal")
            .is_some_and(|n| n.start_position().row != n.end_position().row)
    {
        out.at(
            node,
            "anonymous_argument_in_multiline_closure",
            "Name this multiline closure parameter",
        );
        return;
    }
    let Some(parent) = node.parent() else { return };
    if name.starts_with("NS")
        && matches!(
            name,
            "NSAffineTransform"
                | "NSArray"
                | "NSCalendar"
                | "NSCharacterSet"
                | "NSData"
                | "NSDateComponents"
                | "NSDateInterval"
                | "NSDate"
                | "NSDecimalNumber"
                | "NSDictionary"
                | "NSIndexPath"
                | "NSIndexSet"
                | "NSLocale"
                | "NSMeasurement"
                | "NSNotification"
                | "NSNumber"
                | "NSPersonNameComponents"
                | "NSSet"
                | "NSString"
                | "NSTimeZone"
                | "NSURL"
                | "NSURLComponents"
                | "NSURLQueryItem"
                | "NSURLRequest"
                | "NSUUID"
        )
        && reference(cx, node, parent)
    {
        out.at(
            node,
            "legacy_objc_type",
            format!("Prefer the Swift value type for {name}"),
        );
    }
    if self_types.first_components.contains(name) && reference(cx, node, parent) {
        prefer_self(cx, out, node, self_types);
    }
}

fn reference(cx: &Context<'_>, node: Node<'_>, parent: Node<'_>) -> bool {
    match parent.kind() {
        "user_type" => {
            let first = syntax_children(parent).next();
            first == Some(node)
                || first.is_some_and(|n| {
                    cx.text(n) == "Foundation" && n.next_named_sibling() == Some(node)
                })
        }
        "navigation_suffix" => parent
            .parent()
            .is_some_and(|p| field(cx, p, "target") == "Foundation"),
        "parameter"
        | "lambda_parameter"
        | "value_argument_label"
        | "pattern"
        | "identifier"
        | "type_parameter"
        | "attribute"
        | "value_binding_pattern"
        | "capture_list_item" => false,
        _ if parent.kind().ends_with("_declaration") || parent.kind() == "enum_entry" => {
            parent.child_by_field_name("value") == Some(node)
                || parent.child_by_field_name("default_value") == Some(node)
        }
        _ => true,
    }
}

#[derive(Default)]
struct SelfTypes<'a> {
    names: HashMap<usize, &'a str>,
    first_components: HashSet<&'a str>,
}

impl<'a> SelfTypes<'a> {
    fn add(&mut self, cx: &Context<'a>, decl: Node<'_>) {
        let Some(name_node) = decl.child_by_field_name("name") else {
            return;
        };
        let name = cx.text(name_node);
        if name.trim_matches('`') == "Self" || name.contains('<') {
            return;
        }
        if decl.child_by_field_name("body").is_some_and(|body| {
            syntax_children(body).any(|member| {
                matches!(
                    member.kind(),
                    "class_declaration" | "protocol_declaration" | "typealias_declaration"
                ) && field(cx, member, "name") == name
            })
        }) {
            return;
        }
        self.names.insert(decl.id(), name);
        self.first_components
            .insert(name.split('.').next().unwrap_or(name));
    }
}

fn prefer_self(cx: &Context<'_>, out: &mut Findings, node: Node<'_>, types: &SelfTypes<'_>) {
    let Some(decl) = owner(node) else { return };
    let Some(&name) = types.names.get(&decl.id()) else {
        return;
    };
    let qualified = name.contains('.');
    if qualified {
        if !ancestors(node)
            .take_while(|n| matches!(n.kind(), "user_type" | "navigation_expression"))
            .any(|n| {
                cx.text(n) == name
                    && !n
                        .parent()
                        .is_some_and(|p| is_call(p) && callee(p) == Some(n))
            })
        {
            return;
        }
    } else if cx.text(node) != name {
        return;
    }
    let Some(body) = decl.child_by_field_name("body") else {
        return;
    };
    if !body.byte_range().contains(&node.start_byte()) {
        return;
    }
    let value_type = matches!(field(cx, decl, "declaration_kind"), "struct" | "enum");
    let chain = || ancestors(node).take_while(|n| *n != body);
    if chain().any(|n| {
        matches!(
            n.kind(),
            "attribute" | "inheritance_specifier" | "type_parameters"
        ) && !value_type
            || n.kind() == "macro_invocation" && !cx.text(n).starts_with("#selector(")
    }) {
        return;
    }
    let Some(parent) = node.parent() else { return };
    if parent.kind() == "user_type" && child(parent, "type_arguments").is_some() {
        return;
    }
    if !value_type {
        if decl.kind() == "protocol_declaration"
            || field(cx, decl, "declaration_kind") == "actor"
            || !chain().any(|n| n.kind() == "statements")
            || chain().any(|n| {
                n.kind() == "property_declaration"
                    && n.parent() == Some(body)
                    && n.child_by_field_name("computed_value").is_none()
                    && n.child_by_field_name("value").is_some()
            })
        {
            return;
        }
        if chain().any(|n| {
            matches!(
                n.kind(),
                "type_annotation"
                    | "parameter"
                    | "typealias_declaration"
                    | "opaque_type"
                    | "existential_type"
                    | "protocol_composition_type"
                    | "as_expression"
                    | "check_expression"
                    | "type_arguments"
                    | "type_constraints"
            ) || n.kind() == "navigation_expression"
                && n.child_by_field_name("suffix")
                    .is_some_and(|s| field(cx, s, "suffix") == "self")
        }) {
            return;
        }
        if is_call(parent) && callee(parent) == Some(node)
            || parent.kind() == "user_type"
                && parent
                    .parent()
                    .is_some_and(|p| is_call(p) && callee(p) == Some(parent))
        {
            return;
        }
    } else if parent.kind() == "user_type"
        && parent.parent().is_some_and(|annotation| {
            annotation.kind() == "type_annotation"
                && annotation.parent().is_some_and(|p| {
                    p.parent() == Some(body) && p.child_by_field_name("computed_value").is_none()
                })
        })
    {
        return;
    }
    out.at(
        node,
        "prefer_self_in_static_references",
        format!("Refer to {name} as Self here"),
    );
}

fn magic_number(cx: &Context<'_>, out: &mut Findings, node: Node<'_>, tests: &HashSet<&str>) {
    let literal = cx.text(node).replace('_', "");
    let Ok(value) = literal.parse::<f64>() else {
        return;
    };
    if matches!(value, 0.0 | 1.0 | 100.0) {
        return;
    }
    let Some(parent) = node.parent() else { return };
    let initialized = |n: Node<'_>| {
        n.parent().is_some_and(|p| {
            ["value", "default_value", "raw_value"].iter().any(|f| {
                let mut cursor = p.walk();
                (p.kind().ends_with("_declaration") || p.kind() == "enum_entry")
                    && p.children_by_field_name(f, &mut cursor).any(|v| v == n)
            })
        })
    };
    if initialized(node)
        || matches!(
            parent.kind(),
            "prefix_expression"
                | "postfix_expression"
                | "range_expression"
                | "open_start_range_expression"
                | "open_end_range_expression"
        ) && initialized(parent)
    {
        return;
    }
    if parent.kind() == "tuple_expression" && initialized(parent) {
        let mut cursor = parent.walk();
        if parent
            .children_by_field_name("value", &mut cursor)
            .all(|v| {
                matches!(
                    v.kind(),
                    "integer_literal"
                        | "real_literal"
                        | "line_string_literal"
                        | "multi_line_string_literal"
                        | "boolean_literal"
                )
            })
        {
            return;
        }
    }
    if parent.kind() == "bitwise_operation"
        && matches!(field(cx, parent, "op"), "<<" | ">>")
        && (initialized(parent) || parent.parent().is_some_and(|p| p.kind() == "statements"))
    {
        return;
    }
    if parent.kind() == "value_argument"
        && matches!(
            field(cx, parent, "name"),
            "white"
                | "alpha"
                | "red"
                | "displayP3Red"
                | "green"
                | "blue"
                | "hue"
                | "saturation"
                | "brightness"
                | "cgColor"
                | "ciColor"
                | "resource"
                | "patternImage"
        )
        && ancestors(parent)
            .find(|n| is_call(*n) || n.kind() == "playground_literal")
            .is_some_and(|call| {
                call.kind() == "playground_literal"
                    || callee(call)
                        .is_some_and(|n| matches!(cx.text(n), "UIColor" | "UIColor.init"))
            })
    {
        return;
    }
    if ancestors(node).any(|n| match n.kind() {
        "availability_condition" | "availability_argument" | "directive" => true,
        "attribute" => child(n, "user_type").is_some_and(|t| cx.text(t) == "available"),
        "function_declaration" => has_attribute(cx, n, &["Test"]),
        "class_declaration" => tests.contains(field(cx, n, "name")),
        "macro_invocation" => {
            field(cx, n, "name") == "Preview" || cx.text(n).starts_with("#Preview")
        }
        _ => false,
    }) {
        return;
    }
    out.at(
        node,
        "no_magic_numbers",
        format!("Give {} a descriptive constant name", cx.text(node)),
    );
}

fn parameters(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    if has_modifier(cx, node, "override") {
        return;
    }
    let params: Vec<_> = syntax_children(node)
        .filter(|n| n.kind() == "parameter")
        .collect();
    let count = params
        .iter()
        .filter(|p| default_value(**p).is_none())
        .count();
    if node.kind() != "init_declaration" && count > 5 {
        let token = children(node).find(|n| n.kind() == "func").unwrap_or(node);
        out.at(
            token,
            "function_parameter_count",
            format!("Function has {count} required parameters (limit 5)"),
        );
    }
    let mut next_required = false;
    for (index, &param) in params.iter().enumerate().rev() {
        if child(param, "parameter_modifiers").is_some_and(|n| cx.text(n).contains("@escaping"))
            || param.child_by_field_name("type").is_some_and(closure_type)
        {
            continue;
        }
        let default = default_value(param);
        if next_required
            && default.is_some()
            && !(index == 0 && default.is_some_and(|v| cx.text(v) == "#isolation"))
        {
            out.at(
                param,
                "function_default_parameter_at_end",
                "Place defaulted parameters after required parameters",
            );
        }
        next_required = default.is_none();
    }
}

fn closure_type(node: Node<'_>) -> bool {
    match node.kind() {
        "function_type" => true,
        "optional_type" | "tuple_type_item" | "tuple_type" => {
            syntax_children(node).count() == 1
                && syntax_children(node).next().is_some_and(closure_type)
        }
        _ => false,
    }
}

fn inside_condition(node: Node<'_>) -> bool {
    ancestors(node).any(|p| {
        matches!(
            p.kind(),
            "if_statement" | "guard_statement" | "while_statement" | "for_statement"
        ) && children(p)
            .find(|n| n.kind() == "{")
            .is_some_and(|brace| node.start_byte() < brace.start_byte())
    })
}

fn closure_arguments(out: &mut Findings, node: Node<'_>) {
    let Some(suffix) = suffix(node) else { return };
    let Some(list) = child(suffix, "value_arguments") else {
        return;
    };
    if list.child(0).is_none_or(|n| n.kind() != "(") {
        return;
    }
    let mut closures = 0;
    let mut last = None;
    let mut previous_closure = false;
    for arg in syntax_children(list).filter(|n| n.kind() == "value_argument") {
        previous_closure = last.is_some();
        last = arg
            .child_by_field_name("value")
            .filter(|v| v.kind() == "lambda_literal");
        closures += usize::from(last.is_some());
    }
    if let Some(trailing) = child(suffix, "lambda_literal") {
        if closures > 0 {
            out.at(
                trailing,
                "multiple_closures_with_trailing_closure",
                "Keep closure arguments together inside the parentheses",
            );
        }
    } else if let Some(last) = last
        && !previous_closure
        && !inside_condition(node)
    {
        out.at(
            last,
            "trailing_closure",
            "Use trailing closure syntax for the final closure",
        );
    }
}

fn empty_block(out: &mut Findings, node: Node<'_>) {
    let mut opening = None;
    let mut empty = false;
    for token in children(node) {
        match token.kind() {
            "{" | "^{" => {
                opening = Some(token);
                empty = true;
            }
            "}" => {
                if let Some(brace) = opening.take()
                    && empty
                {
                    out.at(
                        brace,
                        "no_empty_block",
                        "Add an implementation or a comment explaining the empty block",
                    );
                }
            }
            "in" if node.kind() == "lambda_literal" => {
                empty = true;
            }
            _ if opening.is_some() => {
                empty = false;
            }
            _ => {}
        }
    }
}

fn implicit_return(out: &mut Findings, node: Node<'_>) {
    let mut items = syntax_children(node);
    let Some(statement) = items.next() else {
        return;
    };
    if items.next().is_some() || statement.kind() != "control_transfer_statement" {
        return;
    }
    let Some(keyword) = statement.child(0).filter(|n| n.kind() == "return") else {
        return;
    };
    let Some(parent) = node.parent() else { return };
    let eligible = match parent.kind() {
        "computed_getter" | "lambda_literal" | "computed_property" => true,
        "function_body" => parent
            .parent()
            .is_some_and(|p| matches!(p.kind(), "function_declaration" | "init_declaration")),
        _ => false,
    };
    if eligible && !ancestors(node).any(|n| n.kind() == "protocol_declaration") {
        out.at(
            keyword,
            "implicit_return",
            "Omit return from this single-statement body",
        );
    }
}

fn key_path(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let Some(body) = child(node, "statements") else {
        return;
    };
    let mut items = syntax_children(body);
    let Some(mut expr) = items.next() else { return };
    if items.next().is_some() {
        return;
    }
    while expr.kind() == "navigation_expression" {
        let Some(base) = expr.child_by_field_name("target") else {
            return;
        };
        if children(expr).any(|n| matches!(n.kind(), "?" | "!")) {
            return;
        }
        expr = base;
    }
    if expr.kind() != "simple_identifier" {
        return;
    }
    let param = node
        .child_by_field_name("type")
        .and_then(|t| child(t, "lambda_function_type_parameters"));
    let expected = if let Some(params) = param {
        let mut params = syntax_children(params).filter(|n| n.kind() == "lambda_parameter");
        let Some(p) = params.next() else { return };
        if params.next().is_some() {
            return;
        }
        field(cx, p, "name")
    } else {
        "$0"
    };
    if cx.text(expr) != expected {
        return;
    }
    let Some(parent) = node.parent() else { return };
    if is_call(parent)
        || parent.kind() == "macro_invocation"
        || parent.kind() == "nil_coalescing_expression"
    {
        return;
    }
    if matches!(
        parent.kind(),
        "value_argument" | "call_suffix" | "constructor_suffix"
    ) {
        let Some(call) = ancestors(node).find(|n| is_call(*n) || n.kind() == "macro_invocation")
        else {
            return;
        };
        if !matches!(
            call_name(cx, call),
            "allSatisfy"
                | "contains"
                | "compactMap"
                | "drop"
                | "filter"
                | "first"
                | "flatMap"
                | "map"
                | "partition"
                | "prefix"
        ) {
            return;
        }
        let trailing = suffix(call).map_or(0, |s| {
            syntax_children(s)
                .filter(|n| n.kind() == "lambda_literal")
                .count()
        });
        if args(call).count() + trailing != 1 {
            return;
        }
    }
    out.at(
        node,
        "prefer_key_path",
        "Use a key path for this member-access closure",
    );
}

fn conditions(out: &mut Findings, node: Node<'_>) {
    let mut cursor = node.walk();
    for mut expr in node.children_by_field_name("condition", &mut cursor) {
        // Optional binding initializers are expressions too, but cannot be split into conditions.
        if expr.prev_sibling().is_some_and(|n| n.kind() == "=") {
            continue;
        }
        loop {
            if expr.kind() == "tuple_expression"
                && child(expr, "value_argument_label").is_none()
                && syntax_children(expr).count() == 1
            {
                let Some(inner) = expr.child_by_field_name("value") else {
                    break;
                };
                expr = inner;
                continue;
            }
            if expr.kind() != "conjunction_expression" {
                break;
            }
            if let Some(op) = expr.child_by_field_name("op") {
                out.at(
                    op,
                    "prefer_condition_list",
                    "Separate these conditions with a comma",
                );
            }
            let Some(lhs) = expr.child_by_field_name("lhs") else {
                break;
            };
            expr = lhs;
        }
    }
}

fn optional_case(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    fn check_mark(cx: &Context<'_>, out: &mut Findings, pattern: Node<'_>) {
        let Some(mark) = children(pattern)
            .filter(|n| !n.is_extra())
            .last()
            .filter(|n| n.kind() == "?")
        else {
            return;
        };
        let source = cx.source[pattern.start_byte()..mark.start_byte()].trim();
        if !matches!(source, "_" | "true" | "false")
            && !source.starts_with("let ")
            && !source.starts_with("var ")
        {
            out.at(
                mark,
                "optional_enum_case_matching",
                "Omit the optional marker when matching this enum case",
            );
        }
    }
    let pattern = child(node, "pattern").unwrap_or(node);
    check_mark(cx, out, pattern);
    if children(pattern).next().is_some_and(|n| n.kind() == "(") {
        for element in syntax_children(pattern).filter(|n| n.kind() == "pattern") {
            check_mark(cx, out, element);
        }
    }
}

fn private_state(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    if !node.parent().is_some_and(member_scope)
        || !has_attribute(cx, node, &["State", "StateObject"])
        || has_modifier(cx, node, "private")
        || has_modifier(cx, node, "fileprivate")
    {
        return;
    }
    if owner(node).is_some_and(|ty| {
        matches!(
            field(cx, ty, "declaration_kind"),
            "struct" | "class" | "actor"
        ) && inherits(cx, ty, &["View", "ViewModifier", "App", "Scene"])
    }) {
        let location = child(node, "modifiers")
            .and_then(|mods| child(mods, "visibility_modifier"))
            .or_else(|| child(node, "value_binding_pattern"))
            .unwrap_or(node);
        out.at(
            location,
            "private_swiftui_state",
            "Declare this SwiftUI state property private",
        );
    }
}

fn accessibility(cx: &Context<'_>, out: &mut Findings, call: Node<'_>) {
    let name = call_name(cx, call);
    let image = name == "Image"
        && cx.enabled("accessibility_label_for_image")
        && callee(call).is_some_and(|n| matches!(cx.text(n), "Image" | "SwiftUI.Image"));
    let single_tap = |n| arg_value(cx, n, "count").is_none_or(|v| cx.text(v) == "1");
    let gesture = cx.enabled("accessibility_trait_for_button")
        && match name {
            "onTapGesture" => single_tap(call),
            "gesture" | "simultaneousGesture" | "highPriorityGesture" => args(call)
                .next()
                .and_then(|a| a.child_by_field_name("value"))
                .and_then(|v| std::iter::successors(Some(v), |n| base_call(*n)).last())
                .is_some_and(|v| is_call(v) && call_name(cx, v) == "TapGesture" && single_tap(v)),
            _ => false,
        };
    if !image && !gesture {
        return;
    }
    if !owner(call).is_some_and(|ty| {
        field(cx, ty, "declaration_kind") == "struct" && inherits(cx, ty, &["View"])
    }) {
        return;
    }
    let enclosing_calls = || {
        ancestors(call)
            .take_while(|n| {
                !matches!(
                    n.kind(),
                    "class_declaration" | "function_declaration" | "property_declaration"
                )
            })
            .filter(|n| is_call(*n))
    };
    if image {
        if args(call).any(|a| matches!(field(cx, a, "name"), "label" | "decorative")) {
            return;
        }
        for parent in enclosing_calls() {
            let parent_name = call_name(cx, parent);
            if parent_name == "NavigationLink"
                || accessibility_hidden(cx, parent)
                || matches!(
                    parent_name,
                    "accessibilityLabel" | "accessibilityValue" | "accessibilityHint"
                )
                || parent_name == "accessibility"
                    && args(parent)
                        .any(|a| matches!(field(cx, a, "name"), "label" | "value" | "hint"))
                || parent_name == "accessibilityElement"
                    && (args(parent).next().is_none()
                        || arg_value(cx, parent, "children")
                            .is_some_and(|v| cx.text(v) == ".ignore"))
            {
                return;
            }
            if parent_name == "Label" {
                let encloses_image = |n: Node<'_>| n.byte_range().contains(&call.start_byte());
                if arg_value(cx, parent, "icon").is_some_and(encloses_image)
                    || suffix(parent).is_some_and(|s| {
                        syntax_children(s).any(|n| {
                            n.kind() == "lambda_literal"
                                && encloses_image(n)
                                && n.prev_named_sibling().is_some_and(|l| cx.text(l) == "icon")
                        })
                    })
                {
                    return;
                }
            }
        }
        out.at(
            call,
            "accessibility_label_for_image",
            "Give this image an accessibility label or mark it decorative",
        );
    }
    if gesture {
        let chain = std::iter::successors(Some(call), |n| base_call(*n)).chain(enclosing_calls());
        for modifier in chain {
            let name = call_name(cx, modifier);
            let traits = match name {
                "accessibility" => arg_value(cx, modifier, "addTraits"),
                "accessibilityAddTraits" => args(modifier)
                    .next()
                    .and_then(|a| a.child_by_field_name("value")),
                _ => None,
            };
            if matches!(name, "Button" | "Link")
                || accessibility_hidden(cx, modifier)
                || traits.is_some_and(|v| button_trait(cx, v))
            {
                return;
            }
        }
        out.at(
            call,
            "accessibility_trait_for_button",
            "Give this tappable view the isButton or isLink accessibility trait",
        );
    }
}

fn accessibility_hidden(cx: &Context<'_>, call: Node<'_>) -> bool {
    let value = match call_name(cx, call) {
        "accessibilityHidden" => args(call)
            .next()
            .and_then(|a| a.child_by_field_name("value")),
        "accessibility" => arg_value(cx, call, "hidden"),
        _ => None,
    };
    value.is_some_and(|v| cx.text(v) == "true")
}

fn button_trait(cx: &Context<'_>, node: Node<'_>) -> bool {
    if node.kind() == "array_literal" {
        return syntax_children(node).any(|v| button_trait(cx, v));
    }
    matches!(node.kind(), "prefix_expression" | "navigation_expression")
        && matches!(
            cx.text(node).rsplit('.').next(),
            Some("isButton" | "isLink")
        )
}

fn synthesized_init(cx: &Context<'_>, out: &mut Findings, decl: Node<'_>) {
    let Some(body) = decl.child_by_field_name("body") else {
        return;
    };
    let inits: Vec<_> = syntax_children(body)
        .filter(|n| n.kind() == "init_declaration")
        .collect();
    if inits.is_empty() {
        return;
    }
    let props: Vec<_> = syntax_children(body)
        .filter(|n| {
            n.kind() == "property_declaration"
                && !has_modifier(cx, *n, "static")
                && n.child_by_field_name("computed_value").is_none()
        })
        .collect();
    // An explicit initializer suppresses synthesis of the others in the same declaration.
    if inits.iter().all(|&init| equivalent_init(cx, init, &props)) {
        for init in inits {
            out.at(
                init,
                "unneeded_synthesized_initializer",
                "Swift synthesizes this initializer automatically",
            );
        }
    }
}

fn equivalent_init(cx: &Context<'_>, init: Node<'_>, props: &[Node<'_>]) -> bool {
    let access = if props.iter().any(|p| has_modifier(cx, *p, "private")) {
        "private"
    } else if props.iter().any(|p| has_modifier(cx, *p, "fileprivate")) {
        "fileprivate"
    } else {
        "internal"
    };
    if child(init, "modifiers").is_some_and(|m| syntax_children(m).any(|n| cx.text(n) != access))
        || access != "internal" && !has_modifier(cx, init, access)
        || children(init).any(|n| {
            matches!(
                n.kind(),
                "?" | "!"
                    | "bang"
                    | "throws"
                    | "throws_clause"
                    | "async"
                    | "attribute"
                    | "type_parameters"
                    | "type_constraints"
            )
        })
    {
        return false;
    }
    let Some(body) = init.child_by_field_name("body") else {
        return false;
    };
    let statements: Vec<_> = child(body, "statements")
        .into_iter()
        .flat_map(syntax_children)
        .collect();
    let params: Vec<_> = syntax_children(init)
        .filter(|n| n.kind() == "parameter")
        .collect();
    if params.is_empty() {
        return statements.is_empty()
            && props.iter().all(|&p| {
                child(p, "value_binding_pattern").is_some_and(|k| cx.text(k) == "var")
                    && p.child_by_field_name("value").is_some()
            });
    }
    if props.len() != params.len() || props.len() != statements.len() {
        return false;
    }
    let mut expected = HashSet::new();
    for (&prop, param) in props.iter().zip(params) {
        let name = field(cx, param, "name");
        let prop_name = field(cx, prop, "name");
        if param.child_by_field_name("external_name").is_some()
            || name != prop_name
            || !expected.insert(name)
            || child(prop, "modifiers").is_some_and(|m| child(m, "attribute").is_some())
            || param
                .prev_named_sibling()
                .is_some_and(|n| n.kind() == "attribute")
            || child(prop, "type_annotation")
                .is_some_and(|ty| field(cx, ty, "type") != field(cx, param, "type"))
        {
            return false;
        }
        let value = prop.child_by_field_name("value").map(|v| cx.text(v));
        if value.is_some()
            && child(prop, "value_binding_pattern").is_some_and(|k| cx.text(k) == "let")
            || default_value(param).map(|v| cx.text(v)) != value
        {
            return false;
        }
    }
    statements.iter().all(|&s| {
        if s.kind() != "assignment" || field(cx, s, "operator") != "=" {
            return false;
        }
        let Some(target) = s.child_by_field_name("target") else {
            return false;
        };
        let name = field(cx, s, "result");
        let member = child(target, "navigation_expression").unwrap_or(target);
        field(cx, member, "target") == "self"
            && member
                .child_by_field_name("suffix")
                .is_some_and(|n| field(cx, n, "suffix") == name)
            && expected.remove(name)
    })
}

// Bindings live on the scope in which Swift makes them visible. An activation offset
// keeps a local declaration out of its own initializer and a guard binding out of its else.
#[derive(Default)]
struct Scopes<'a> {
    bindings: HashMap<usize, HashMap<&'a str, usize>>,
}

impl<'a> Scopes<'a> {
    fn insert(&mut self, scope: Node<'_>, name: &'a str, start: usize) {
        if name.is_empty() || name == "_" {
            return;
        }
        self.bindings
            .entry(scope.id())
            .or_default()
            .entry(name)
            .and_modify(|offset| *offset = (*offset).min(start))
            .or_insert(start);
    }

    fn report(&self, cx: &Context<'_>, out: &mut Findings, ident: Node<'_>) {
        let name = cx.text(ident);
        for scope in ancestors(ident) {
            if self
                .bindings
                .get(&scope.id())
                .and_then(|table| table.get(name))
                .is_some_and(|&start| start <= ident.start_byte())
            {
                out.at(
                    ident,
                    "variable_shadowing",
                    format!("'{name}' shadows an existing declaration"),
                );
                return;
            }
            if member_scope(scope)
                && scope.parent().is_some_and(|p| {
                    matches!(
                        field(cx, p, "declaration_kind"),
                        "struct" | "class" | "enum" | "actor"
                    )
                })
            {
                return;
            }
        }
    }

    fn visit(&mut self, cx: &Context<'a>, out: &mut Findings, node: Node<'_>) {
        match node.kind() {
            "class_body" | "enum_class_body" => {
                for member in syntax_children(node).filter(|n| n.kind() == "property_declaration") {
                    let mut cursor = member.walk();
                    for pattern in member.children_by_field_name("name", &mut cursor) {
                        if let Some(id) = simple_binding(pattern) {
                            self.insert(node, cx.text(id), 0);
                        }
                    }
                }
            }
            "function_declaration" | "init_declaration" | "subscript_declaration" => {
                if node.kind() == "function_declaration"
                    && !node.parent().is_some_and(member_scope)
                    && let (Some(name), Some(scope)) =
                        (node.child_by_field_name("name"), local_scope(node))
                {
                    self.insert(scope, cx.text(name), node.start_byte());
                }
                if let Some(body) = node
                    .child_by_field_name("body")
                    .or_else(|| child(node, "computed_property"))
                {
                    for p in syntax_children(node).filter(|n| n.kind() == "parameter") {
                        if let Some(name) = p.child_by_field_name("name") {
                            self.insert(body, cx.text(name), 0);
                        }
                    }
                }
            }
            "lambda_parameter" => {
                if let Some(closure) = ancestors(node).find(|n| n.kind() == "lambda_literal")
                    && let Some(body) = child(closure, "statements")
                    && let Some(name) = node.child_by_field_name("name")
                {
                    self.insert(body, cx.text(name), 0);
                }
            }
            "property_declaration" if !node.parent().is_some_and(member_scope) => {
                let Some(scope) = local_scope(node) else {
                    return;
                };
                let mutable =
                    child(node, "value_binding_pattern").is_some_and(|n| cx.text(n) == "var");
                let mut cursor = node.walk();
                for pattern in node.children_by_field_name("name", &mut cursor) {
                    let value = binding_initializer(pattern);
                    visit_binding(pattern, &mut |id| {
                        if !(mutable && identity(cx, id, value)) {
                            self.report(cx, out, id);
                        }
                        self.insert(scope, cx.text(id), node.end_byte());
                    });
                }
            }
            "for_statement" => {
                if let Some(item) = node.child_by_field_name("item") {
                    visit_binding(item, &mut |id| {
                        self.report(cx, out, id);
                        if let Some(body) = child(node, "statements") {
                            self.insert(body, cx.text(id), 0);
                        }
                    });
                }
            }
            "if_statement" | "while_statement" | "guard_statement" => {
                let guard = node.kind() == "guard_statement";
                let scope = if guard {
                    local_scope(node)
                } else {
                    // For an empty then-block, an else-block can be the first named statement list.
                    children(node)
                        .find(|n| n.kind() == "{")
                        .and_then(|brace| {
                            std::iter::successors(brace.next_sibling(), |n| n.next_sibling())
                                .find(|n| !n.is_extra())
                        })
                        .filter(|n| n.kind() == "statements")
                };
                let mut in_optional = false;
                let mut pattern_ids = Vec::new();
                for token in children(node) {
                    match token.kind() {
                        "value_binding_pattern" => in_optional = true,
                        "=" | "," | "{" | "else" if in_optional => {
                            let initializer = if token.kind() == "=" {
                                std::iter::successors(token.next_named_sibling(), |n| {
                                    n.next_named_sibling()
                                })
                                .find(|n| !n.is_extra())
                            } else {
                                None
                            };
                            for id in pattern_ids.drain(..) {
                                if initializer.is_some() && !identity(cx, id, initializer) {
                                    self.report(cx, out, id);
                                }
                                if let Some(scope) = scope {
                                    self.insert(
                                        scope,
                                        cx.text(id),
                                        if guard { node.end_byte() } else { 0 },
                                    );
                                }
                            }
                            in_optional = false;
                        }
                        "simple_identifier" | "pattern" if in_optional => {
                            visit_binding(token, &mut |id| pattern_ids.push(id))
                        }
                        _ => {}
                    }
                    if token.kind() == "{" {
                        break;
                    }
                }
            }
            "catch_block" => {
                if let Some(body) = child(node, "statements") {
                    if let Some(pattern) = node.child_by_field_name("error") {
                        visit_binding(pattern, &mut |id| self.insert(body, cx.text(id), 0));
                    } else {
                        self.insert(body, "error", 0);
                    }
                }
            }
            "switch_entry" => {
                if let Some(body) = child(node, "statements") {
                    for p in syntax_children(node).filter(|n| n.kind() == "switch_pattern") {
                        case_bindings(p, false, &mut |id| self.insert(body, cx.text(id), 0));
                    }
                }
            }
            _ => {}
        }
    }
}

fn local_scope(node: Node<'_>) -> Option<Node<'_>> {
    ancestors(node).find(|n| matches!(n.kind(), "statements" | "source_file"))
}

fn simple_binding(pattern: Node<'_>) -> Option<Node<'_>> {
    if pattern.kind() == "simple_identifier" {
        return Some(pattern);
    }
    let mut names = syntax_children(pattern);
    let id = names.next()?;
    (id.kind() == "simple_identifier" && names.next().is_none()).then_some(id)
}

fn visit_binding<'t>(pattern: Node<'t>, visit: &mut impl FnMut(Node<'t>)) {
    if let Some(id) = simple_binding(pattern) {
        visit(id);
        return;
    }
    if let Some(id) = pattern.child_by_field_name("bound_identifier") {
        visit(id);
        return;
    }
    for part in syntax_children(pattern).filter(|n| n.kind() == "pattern") {
        visit_binding(part, visit);
    }
}

fn case_bindings<'t>(node: Node<'t>, bound: bool, visit: &mut impl FnMut(Node<'t>)) {
    let bound = bound || child(node, "value_binding_pattern").is_some();
    if bound
        && let Some(id) = node
            .child_by_field_name("bound_identifier")
            .or_else(|| simple_binding(node))
    {
        visit(id);
    } else {
        for nested in syntax_children(node).filter(|n| n.kind() == "pattern") {
            case_bindings(nested, bound, visit);
        }
    }
}

fn binding_initializer(node: Node<'_>) -> Option<Node<'_>> {
    let mut siblings = std::iter::successors(node.next_sibling(), |n| n.next_sibling());
    siblings
        .by_ref()
        .take_while(|n| !matches!(n.kind(), "," | "{"))
        .find(|n| n.kind() == "=")?;
    siblings.find(|n| n.is_named() && !n.is_extra())
}

fn identity(cx: &Context<'_>, name: Node<'_>, value: Option<Node<'_>>) -> bool {
    let Some(value) = value else { return false };
    let name = cx.text(name);
    cx.text(value) == name
        || matches!(field(cx, value, "target"), "self" | "Self")
            && value
                .child_by_field_name("suffix")
                .is_some_and(|s| field(cx, s, "suffix") == name)
}
