use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Point};

use super::syntax::{ancestors, child, children, declaration_keyword, named_children};
use super::{Context, Findings};

fn is_group(node: Node<'_>) -> bool {
    matches!(node.kind(), "class_declaration" | "protocol_declaration")
}

fn group_kind(node: Node<'_>) -> &'static str {
    node.child_by_field_name("declaration_kind")
        .map_or("", |n| n.kind())
}

fn name(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name").or_else(|| {
        (node.kind() == "macro_declaration")
            .then(|| child(node, "simple_identifier"))
            .flatten()
    })
}

// The grammar places leading attributes in modifiers and signature attributes on
// the declaration itself. Stop at the introducer to keep parameter attributes out.
fn modifiers(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    children(node)
        .take_while(|m| {
            m.is_extra()
                || m.kind() == "modifiers"
                || m.kind() == "attribute"
                || m.kind().ends_with("_modifier")
        })
        .filter(|m| !m.is_extra())
        .flat_map(|m| {
            (m.kind() != "modifiers").then_some(m).into_iter().chain(
                (m.kind() == "modifiers")
                    .then_some(m)
                    .into_iter()
                    .flat_map(named_children),
            )
        })
}

fn has_modifier(cx: &Context<'_>, node: Node<'_>, flag: &str) -> bool {
    modifiers(node).any(|m| cx.text(m) == flag)
}

fn attribute_name<'a>(cx: &'a Context<'_>, attr: Node<'_>) -> &'a str {
    attr.named_child(0).map_or("", |n| cx.text(n))
}

fn has_attribute(cx: &Context<'_>, node: Node<'_>, flag: &str) -> bool {
    modifiers(node).any(|m| m.kind() == "attribute" && attribute_name(cx, m) == flag)
}

fn inherited_types<'a>(cx: &'a Context<'_>, node: Node<'_>) -> impl Iterator<Item = &'a str> {
    named_children(node)
        .filter(|n| n.kind() == "inheritance_specifier")
        .filter_map(|n| n.child_by_field_name("inherits_from"))
        .filter(|n| n.kind() != "suppressed_constraint")
        .map(move |n| cx.text(n).split('<').next().unwrap_or("").trim())
}

fn in_local_scope(node: Node<'_>) -> bool {
    ancestors(node).any(|n| {
        matches!(
            n.kind(),
            "function_body" | "lambda_literal" | "computed_property" | "willset_didset_block"
        )
    })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Access {
    Private,
    Fileprivate,
    Internal,
    Package,
    Public,
    Open,
}

impl Access {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "private" => Self::Private,
            "fileprivate" => Self::Fileprivate,
            "internal" => Self::Internal,
            "package" => Self::Package,
            "public" => Self::Public,
            "open" => Self::Open,
            _ => return None,
        })
    }
}

fn access(cx: &Context<'_>, node: Node<'_>) -> Option<Access> {
    modifiers(node)
        .filter(|m| m.kind() == "visibility_modifier")
        .find_map(|m| Access::parse(cx.text(m)))
}

fn declaration_access(cx: &Context<'_>, node: Node<'_>, effective: bool) -> Access {
    let explicit = access(cx, node);
    let Some(owner) = ancestors(node).find(|n| is_group(*n)) else {
        return explicit.unwrap_or(Access::Internal);
    };
    let parent_level = declaration_access(cx, owner, effective);
    match group_kind(owner) {
        "protocol" => parent_level,
        "extension" => explicit.unwrap_or(parent_level),
        "enum" if node.kind() == "enum_entry" => parent_level,
        _ => match explicit {
            Some(acl) if effective => acl.min(parent_level),
            Some(acl) => acl,
            None => Access::Internal.min(parent_level),
        },
    }
}

pub(super) fn check(cx: &Context<'_>, out: &mut Findings) {
    let mut groups = Vec::new();
    let mut protocols = HashMap::new();
    let mut fileprivate_members = Vec::new();

    for &node in &cx.nodes {
        match node.kind() {
            "class_declaration" | "protocol_declaration" => {
                groups.push(node);
                inspect_group(cx, out, node);
                if node.kind() == "protocol_declaration" && cx.enabled("strict_fileprivate") {
                    if let (Some(id), Some(body)) = (name(node), node.child_by_field_name("body")) {
                        protocols.insert(cx.text(id), requirements(cx, body));
                    }
                }
            }
            "typealias_declaration" | "macro_declaration" => groups.push(node),
            "property_declaration" => {
                if cx.enabled("explicit_type_interface") {
                    inspect_bindings(out, node);
                }
            }
            "type_annotation" if cx.enabled("redundant_type_annotation") => {
                redundant_annotation(cx, out, node);
            }
            "optional_type" => inspect_optional(cx, out, node),
            "call_expression" | "constructor_expression"
                if cx.enabled("discouraged_optional_boolean") =>
            {
                inspect_optional_call(cx, out, node);
            }
            _ => {}
        }
        if !node.kind().ends_with("_declaration") && node.kind() != "enum_entry" {
            continue;
        }
        if cx.enabled("attributes") {
            inspect_attributes(cx, out, node);
        }
        if cx.enabled("type_name")
            && (is_group(node) && group_kind(node) != "extension"
                || matches!(
                    node.kind(),
                    "typealias_declaration" | "associatedtype_declaration"
                ))
        {
            inspect_type_name(cx, out, node);
        }
        if cx.enabled("explicit_acl")
            || cx.enabled("missing_docs")
            || cx.enabled("discouraged_default_parameter")
        {
            if !in_local_scope(node) {
                inspect_declaration_access(cx, out, node);
            }
        }
        for modifier in modifiers(node).filter(|m| m.kind() == "visibility_modifier") {
            if cx.enabled("strict_fileprivate") && cx.text(modifier).starts_with("fileprivate") {
                fileprivate_members.push((node, modifier));
            }
            if cx.enabled("lower_acl_than_parent") {
                inspect_parent_access(cx, out, node, modifier);
            }
        }
    }

    for (member, modifier) in fileprivate_members {
        if !implements_requirement(cx, member, modifier, &protocols) {
            out.at(modifier, "strict_fileprivate", "Avoid fileprivate access");
        }
    }
    inspect_file(cx, out, &groups);
}

fn inspect_declaration_access(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let kind = node.kind();
    let default_capable = matches!(
        kind,
        "function_declaration" | "init_declaration" | "subscript_declaration"
    );
    if default_capable
        && cx.enabled("discouraged_default_parameter")
        && node.child_by_field_name("default_value").is_some()
        && matches!(
            declaration_access(cx, node, true),
            Access::Internal | Access::Package
        )
    {
        for eq in children(node).filter(|n| n.kind() == "=") {
            out.at(
                eq,
                "discouraged_default_parameter",
                "Avoid default parameters on internal or package declarations",
            );
        }
    }
    let acl_capable = is_group(node) && group_kind(node) != "extension"
        || default_capable
        || matches!(kind, "property_declaration" | "typealias_declaration");
    if cx.enabled("explicit_acl") && acl_capable && access(cx, node).is_none() {
        let mut owners = ancestors(node).filter(|n| is_group(*n)).peekable();
        let inherits_extension = owners
            .peek()
            .is_some_and(|p| group_kind(*p) == "extension" && access(cx, *p).is_some());
        if !inherits_extension
            && !owners.any(|p| {
                group_kind(p) == "protocol"
                    || access(cx, p).is_some_and(|acl| acl <= Access::Fileprivate)
            })
        {
            out.at(
                declaration_keyword(node),
                "explicit_acl",
                "Specify an explicit access control level",
            );
        }
    }
    if cx.enabled("missing_docs")
        && (acl_capable
            || matches!(
                kind,
                "associatedtype_declaration"
                    | "enum_entry"
                    | "protocol_function_declaration"
                    | "protocol_property_declaration"
            ))
        && declaration_access(cx, node, false) >= Access::Public
        && !has_modifier(cx, node, "override")
        && !std::iter::once(node)
            .chain(ancestors(node))
            .filter(|n| is_group(*n))
            .any(|p| inherited_types(cx, p).next().is_some())
        && !has_docs(cx, node)
    {
        out.at(
            declaration_keyword(node),
            "missing_docs",
            "Document this public declaration",
        );
    }
}

fn has_docs(cx: &Context<'_>, node: Node<'_>) -> bool {
    let mut previous = node.prev_named_sibling();
    // A doc comment before #if also documents the first declaration of the branch.
    if let Some(directive) =
        previous.filter(|n| n.kind() == "directive" && cx.text(*n).starts_with("#if"))
    {
        previous = directive.prev_named_sibling();
    }
    previous.is_some_and(|p| {
        matches!(p.kind(), "comment" | "multiline_comment")
            && (cx.text(p).starts_with("///") || cx.text(p).starts_with("/**"))
            && cx.source[..p.start_byte()]
                .rsplit('\n')
                .next()
                .is_none_or(|s| s.trim().is_empty())
    })
}

fn inspect_parent_access(cx: &Context<'_>, out: &mut Findings, node: Node<'_>, modifier: Node<'_>) {
    let Some(acl) = Access::parse(cx.text(modifier).split('(').next().unwrap_or("").trim()) else {
        return;
    };
    if !matches!(acl, Access::Internal | Access::Public | Access::Open) {
        return;
    }
    let Some(parent) =
        ancestors(node).find(|p| matches!(group_kind(*p), "actor" | "class" | "enum" | "struct"))
    else {
        return;
    };
    let parent_acl = access(cx, parent);
    let enclosing_extension = ancestors(parent)
        .find(|n| is_group(*n))
        .filter(|n| group_kind(*n) == "extension");
    let inherited = enclosing_extension.and_then(|p| access(cx, p));
    let higher = match acl {
        Access::Internal => parent_acl
            .or(inherited)
            .is_some_and(|a| a <= Access::Fileprivate),
        Access::Public => {
            parent_acl.map_or(inherited != Some(Access::Public), |a| a <= Access::Internal)
        }
        Access::Open => parent_acl != Some(Access::Open),
        _ => false,
    };
    if higher {
        out.at(
            modifier,
            "lower_acl_than_parent",
            "Access level exceeds the enclosing type's access level",
        );
    }
}

fn inspect_type_name(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let Some(id) = name(node) else { return };
    let raw = cx.text(id);
    let mut word = raw.trim_matches('`');
    if access(cx, node).is_some_and(|acl| acl <= Access::Fileprivate) {
        word = word.strip_prefix('_').unwrap_or(word);
    }
    if word.ends_with("_Previews") && inherited_types(cx, node).any(|t| t == "PreviewProvider") {
        word = word.trim_end_matches("_Previews");
    }
    if !word.chars().all(char::is_alphanumeric)
        || word.starts_with(char::is_lowercase)
        || !(3..=40).contains(&word.chars().count())
    {
        out.at(id, "type_name", format!("Type name '{raw}' should be alphanumeric, start uppercase, and have 3–40 characters"));
    }
}

fn inspect_attributes(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let same_line = match node.kind() {
        "import_declaration" | "property_declaration" | "protocol_property_declaration" => true,
        "function_declaration"
        | "protocol_function_declaration"
        | "init_declaration"
        | "protocol_declaration" => false,
        "class_declaration" if group_kind(node) != "actor" => false,
        _ => return,
    };
    let token = declaration_keyword(node);
    let line = token.start_position().row;
    let mut dedicated_end = None;
    let mut previous_end = None;
    let mut invalid = false;
    for attr in modifiers(node).filter(|m| m.kind() == "attribute") {
        let start = attr.start_position().row;
        let end = attr.end_position().row;
        let on_line = matches!(attribute_name(cx, attr), "IBAction" | "NSManaged")
            || (same_line && !children(attr).any(|c| c.kind() == "("));
        invalid |= previous_end.is_some_and(|prev| start > prev + 1);
        if on_line {
            invalid |= start != line;
        } else {
            invalid |= start == line || dedicated_end == Some(start);
            dedicated_end = Some(end);
        }
        previous_end = Some(end);
    }
    if invalid || dedicated_end.is_some_and(|end| end + 1 != line) {
        out.at(
            token,
            "attributes",
            "Place declaration attributes on the appropriate line",
        );
    }
}

fn inspect_bindings(out: &mut Findings, node: Node<'_>) {
    if ancestors(node).any(|p| p.kind() == "protocol_declaration") {
        return;
    }
    for binding in named_children(node).filter(|c| c.kind() == "pattern") {
        let annotated = next_named(binding).is_some_and(|c| c.kind() == "type_annotation");
        if !annotated {
            out.at(
                binding,
                "explicit_type_interface",
                "Add a type annotation to this binding",
            );
        }
    }
}

fn next_named(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.next_named_sibling(), |n| n.next_named_sibling())
        .find(|n| !matches!(n.kind(), "comment" | "multiline_comment"))
}

fn redundant_annotation(cx: &Context<'_>, out: &mut Findings, annotation: Node<'_>) {
    let Some(parent) = annotation.parent() else {
        return;
    };
    if !matches!(
        parent.kind(),
        "property_declaration" | "if_statement" | "while_statement" | "guard_statement"
    ) || has_attribute(cx, parent, "IBInspectable")
    {
        return;
    }
    let Some(eq) = std::iter::successors(annotation.next_sibling(), |n| n.next_sibling())
        .find(|n| !n.is_extra())
    else {
        return;
    };
    if eq.kind() != "=" {
        return;
    }
    let Some(value) = next_named(eq) else { return };
    let annotation_text = cx.text(annotation).trim_start_matches(':').trim();
    if expression_accesses_type(cx, value, annotation_text) {
        out.at(
            annotation,
            "redundant_type_annotation",
            "The initializer already specifies this type",
        );
    }
}

fn expression_accesses_type(cx: &Context<'_>, node: Node<'_>, expected: &str) -> bool {
    match node.kind() {
        "postfix_expression"
            if node
                .child_by_field_name("operation")
                .is_some_and(|op| op.kind() == "bang") =>
        {
            node.child_by_field_name("target")
                .is_some_and(|base| expression_accesses_type(cx, base, expected))
        }
        "call_expression" => node
            .named_child(0)
            .is_some_and(|base| expression_accesses_type(cx, base, expected)),
        "constructor_expression" => node
            .child_by_field_name("constructed_type")
            .is_some_and(|base| expression_accesses_type(cx, base, expected)),
        "navigation_expression" => {
            cx.text(node) == expected
                || node
                    .child_by_field_name("target")
                    .is_some_and(|base| expression_accesses_type(cx, base, expected))
        }
        "user_type" => {
            let raw = cx.text(node);
            let Some(rest) = raw.strip_prefix(expected) else {
                return false;
            };
            rest.is_empty() || rest.starts_with('.') || rest.starts_with('<')
        }
        "simple_identifier" | "array_type" | "array_literal" | "dictionary_type" => {
            cx.text(node) == expected
        }
        _ => false,
    }
}

fn inspect_optional(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let Some(base) = node.child_by_field_name("wrapped") else {
        return;
    };
    match base.kind() {
        "array_type" | "dictionary_type" => out.at(
            node,
            "discouraged_optional_collection",
            "Prefer an empty collection to an optional collection",
        ),
        "user_type" => {
            let raw = cx.text(base);
            match raw.split('<').next().unwrap_or("").trim() {
                "Bool" => out.at(
                    node,
                    "discouraged_optional_boolean",
                    "Prefer a non-optional Bool",
                ),
                "Array" | "Set" | "Dictionary" => out.at(
                    node,
                    "discouraged_optional_collection",
                    "Prefer an empty collection to an optional collection",
                ),
                _ => {}
            }
        }
        _ => {}
    }
}

fn inspect_optional_call(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let Some(callee) = node.named_child(0) else {
        return;
    };
    if cx.text(callee) != "Optional.some" {
        return;
    }
    let Some(suffix) =
        named_children(node).find(|n| matches!(n.kind(), "call_suffix" | "constructor_suffix"))
    else {
        return;
    };
    let Some(args) = child(suffix, "value_arguments") else {
        return;
    };
    let mut args = named_children(args).filter(|n| n.kind() == "value_argument");
    if let (Some(arg), None) = (args.next(), args.next()) {
        if arg
            .child_by_field_name("value")
            .is_some_and(|v| v.kind() == "boolean_literal")
        {
            out.at(
                node,
                "discouraged_optional_boolean",
                "Prefer a non-optional Bool",
            );
        }
    }
}

fn inspect_group(cx: &Context<'_>, out: &mut Findings, node: Node<'_>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    if cx.enabled("required_deinit")
        && group_kind(node) == "class"
        && child(body, "deinit_declaration").is_none()
    {
        out.at(
            declaration_keyword(node),
            "required_deinit",
            "Declare an explicit deinitializer for this class",
        );
    }
    if cx.enabled("type_contents_order") {
        let items: Vec<_> = named_children(body)
            .filter_map(|m| member_category(cx, m))
            .collect();
        check_order(out, &items, "type_contents_order", MEMBER_ORDER);
    }
    if cx.enabled("sorted_enum_cases")
        && group_kind(node) == "enum"
        && !has_attribute(cx, node, "frozen")
        && ancestors(node)
            .filter(|p| p.kind().ends_with("_declaration"))
            .all(|p| group_kind(p) == "enum" && !has_attribute(cx, p, "frozen"))
    {
        inspect_cases(cx, out, body);
    }
}

const MEMBER_ORDER: &[&str] = &[
    "case",
    "type_alias or associated_type",
    "subtype",
    "type_property",
    "instance_property",
    "ib_inspectable",
    "ib_outlet",
    "initializer",
    "type_method",
    "view_life_cycle_method",
    "ib_action or ib_segue_action",
    "other_method",
    "subscript",
    "deinitializer",
];
const FILE_ORDER: &[&str] = &[
    "supporting_type",
    "main_type",
    "extension",
    "preview_provider",
    "library_content_provider",
];

fn member_category<'t>(cx: &Context<'_>, node: Node<'t>) -> Option<(Node<'t>, usize)> {
    let mut token = declaration_keyword(node);
    let category = match node.kind() {
        "enum_entry" => 0,
        "typealias_declaration" | "associatedtype_declaration" => 1,
        "class_declaration" | "protocol_declaration" => 2,
        "property_declaration"
        | "protocol_property_declaration"
        | "function_declaration"
        | "protocol_function_declaration" => {
            let property = matches!(
                node.kind(),
                "property_declaration" | "protocol_property_declaration"
            );
            if let Some(static_modifier) =
                modifiers(node).find(|m| matches!(cx.text(*m), "static" | "class"))
            {
                token = static_modifier;
                if property { 3 } else { 8 }
            } else if property {
                if has_attribute(cx, node, "IBOutlet") {
                    6
                } else if has_attribute(cx, node, "IBInspectable") {
                    5
                } else {
                    4
                }
            } else if name(node).is_some_and(|id| {
                matches!(
                    cx.text(id),
                    "loadView"
                        | "loadViewIfNeeded"
                        | "viewIsAppearing"
                        | "viewDidLoad"
                        | "viewWillAppear"
                        | "viewWillLayoutSubviews"
                        | "viewDidLayoutSubviews"
                        | "viewDidAppear"
                        | "viewWillDisappear"
                        | "viewDidDisappear"
                        | "willMove"
                )
            }) {
                9
            } else if has_attribute(cx, node, "IBAction")
                || has_attribute(cx, node, "IBSegueAction")
            {
                10
            } else {
                11
            }
        }
        "init_declaration" => 7,
        "subscript_declaration" => 12,
        "deinit_declaration" => 13,
        _ => return None,
    };
    Some((token, category))
}

fn check_order(
    out: &mut Findings,
    items: &[(Node<'_>, usize)],
    rule: &'static str,
    categories: &[&str],
) {
    // A member is misplaced if a member of any earlier category still follows.
    let mut earliest = categories.len();
    for &(token, category) in items.iter().rev() {
        if category > earliest {
            out.at(
                token,
                rule,
                format!(
                    "Place {} after {}",
                    categories[category], categories[earliest]
                ),
            );
        }
        earliest = earliest.min(category);
    }
}

fn inspect_cases(cx: &Context<'_>, out: &mut Findings, body: Node<'_>) {
    let mut entries = Vec::new();
    let mut conditional_depth = 0usize;
    for member in named_children(body) {
        if member.kind() == "directive" {
            match cx.text(member).split_whitespace().next() {
                Some("#if") => conditional_depth += 1,
                Some("#endif") => conditional_depth = conditional_depth.saturating_sub(1),
                _ => {}
            }
            continue;
        }
        if member.kind() != "enum_entry" {
            continue;
        }
        let mut cursor = member.walk();
        let names: Vec<_> = member
            .children_by_field_name("name", &mut cursor)
            .map(|id| (id, cx.text(id)))
            .collect();
        if let Some(&(_, first)) = names.first() {
            if conditional_depth == 0 {
                entries.push((member, first));
            }
        }
        check_sorted(out, &names);
    }
    check_sorted(out, &entries);
}

fn check_sorted(out: &mut Findings, entries: &[(Node<'_>, &str)]) {
    if entries.len() < 2 {
        return;
    }
    let mut sorted: Vec<_> = entries.iter().map(|(_, name)| *name).collect();
    sorted.sort_by_cached_key(|name| name.to_lowercase());
    for (&(token, name), ordered) in entries.iter().zip(sorted) {
        if name != ordered {
            out.at(
                token,
                "sorted_enum_cases",
                format!("Enum case '{name}' is out of alphabetical order"),
            );
        }
    }
}

fn inspect_file(cx: &Context<'_>, out: &mut Findings, groups: &[Node<'_>]) {
    let stem = cx.path.file_stem().and_then(|p| p.to_str()).unwrap_or("");
    let mut top_level = Vec::new();
    let mut names = HashSet::new();
    let mut extensions = Vec::new();
    let mut matches_file = false;
    let mut seen_type = false;
    let check_filename =
        cx.enabled("file_name") && !groups.is_empty() && !matches!(stem, "main" | "LinuxMain");
    let filename_type = stem.split('+').next().unwrap_or(stem);
    for &group in groups {
        let Some(id) = name(group) else { continue };
        let name = cx.text(id);
        let kind = group_kind(group);
        let top = group.parent().is_some_and(|p| p.kind() == "source_file");
        if top && group.kind() != "macro_declaration" {
            top_level.push(group);
            if cx.enabled("one_declaration_per_file") && is_group(group) && kind != "extension" {
                if seen_type {
                    out.at(
                        declaration_keyword(group),
                        "one_declaration_per_file",
                        "Place this top-level type in a separate file",
                    );
                }
                seen_type = true;
            }
        }
        let qualified = || {
            let mut parts: Vec<_> = ancestors(group)
                .filter(|p| is_group(*p))
                .filter_map(self::name)
                .map(|p| cx.text(p))
                .collect();
            parts.reverse();
            parts.push(name);
            parts.join(".")
        };
        if check_filename && !matches_file {
            matches_file = filename_type == name || (!top && filename_type == qualified());
        }
        if cx.enabled("no_grouping_extension")
            && is_group(group)
            && kind != "protocol"
            && !in_local_scope(group)
            && !ancestors(group).any(|p| {
                p.kind() == "protocol_declaration"
                    || (group_kind(p) == "extension" && child(p, "type_constraints").is_some())
            })
        {
            if kind == "extension" {
                if child(group, "type_constraints").is_none() {
                    extensions.push((group, name));
                }
            } else {
                names.insert(qualified());
            }
        }
    }
    if check_filename && !matches_file {
        out.push(
            Point::new(0, 0),
            "file_name",
            "File name should match a type declared in this file",
        );
    }
    for (extension, name) in extensions {
        if names.contains(name) {
            out.at(
                declaration_keyword(extension),
                "no_grouping_extension",
                "Move this extension into the type declared in this file",
            );
        }
    }
    if cx.enabled("file_types_order") {
        inspect_file_order(cx, out, &top_level, stem);
    }
}

fn inspect_file_order(cx: &Context<'_>, out: &mut Findings, decls: &[Node<'_>], stem: &str) {
    let mut longest = None;
    let mut by_name = None;
    let mut items = Vec::with_capacity(decls.len());
    for (index, &decl) in decls.iter().enumerate() {
        let kind = group_kind(decl);
        let category = if kind == "extension" {
            2
        } else if inherited_types(cx, decl).any(|p| p == "PreviewProvider") {
            3
        } else if inherited_types(cx, decl).any(|p| p == "LibraryContentProvider") {
            4
        } else {
            0
        };
        if by_name.is_none() && name(decl).is_some_and(|n| cx.text(n) == stem) {
            by_name = Some(index);
        }
        if category == 0 && matches!(kind, "actor" | "class" | "enum" | "struct") {
            let length = decl
                .child_by_field_name("body")
                .map_or(0, |n| n.end_byte() - n.start_byte());
            if longest.is_none_or(|(_, size)| length > size) {
                longest = Some((index, length));
            }
        }
        items.push((declaration_keyword(decl), category));
    }
    if let Some(main) = by_name.or_else(|| longest.map(|(i, _)| i)) {
        if items[main].1 != 2 {
            items[main].1 = 1;
            check_order(out, &items, "file_types_order", FILE_ORDER);
        }
    }
}

// SwiftLint exempts fileprivate protocol witnesses when the protocol and the
// conformance are both visible in this file, including setter-only access.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Requirement<'a> {
    Method(&'a str),
    Getter(&'a str),
    Setter(&'a str),
}

fn requirements<'a>(cx: &'a Context<'_>, body: Node<'_>) -> HashSet<Requirement<'a>> {
    let mut result = HashSet::new();
    for decl in named_children(body) {
        let Some(id) = name(decl) else { continue };
        match decl.kind() {
            "protocol_function_declaration" => {
                result.insert(Requirement::Method(cx.text(id)));
            }
            "protocol_property_declaration" => {
                let Some(ident) = named_children(id)
                    .find(|n| n.kind() == "simple_identifier" || n.kind() == "pattern")
                else {
                    continue;
                };
                let Some(accessors) = child(decl, "protocol_property_requirements") else {
                    continue;
                };
                let word = cx.text(ident);
                for accessor in named_children(accessors) {
                    match accessor.kind() {
                        "getter_specifier" => {
                            result.insert(Requirement::Getter(word));
                        }
                        "setter_specifier" => {
                            result.insert(Requirement::Setter(word));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn implements_requirement(
    cx: &Context<'_>,
    member: Node<'_>,
    modifier: Node<'_>,
    protocols: &HashMap<&str, HashSet<Requirement<'_>>>,
) -> bool {
    if !matches!(
        member.kind(),
        "function_declaration" | "property_declaration"
    ) {
        return false;
    }
    let Some(owner) = ancestors(member).find(|n| is_group(*n)) else {
        return false;
    };
    let required: Vec<_> = inherited_types(cx, owner)
        .filter_map(|p| protocols.get(p))
        .collect();
    if required.is_empty() {
        return false;
    }
    if member.kind() == "function_declaration" {
        return name(member).is_some_and(|id| {
            required
                .iter()
                .any(|set| set.contains(&Requirement::Method(cx.text(id))))
        });
    }
    let setter = cx.text(modifier).contains('(');
    let mut patterns: Vec<_> = named_children(member)
        .filter(|p| p.kind() == "pattern")
        .collect();
    while let Some(pattern) = patterns.pop() {
        for item in named_children(pattern) {
            match item.kind() {
                "simple_identifier" => {
                    let name = cx.text(item);
                    let witness = if setter {
                        Requirement::Setter(name)
                    } else {
                        Requirement::Getter(name)
                    };
                    if required.iter().any(|set| set.contains(&witness)) {
                        return true;
                    }
                }
                "pattern" => patterns.push(item),
                _ => {}
            }
        }
    }
    false
}
