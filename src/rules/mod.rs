mod declarations;
mod directives;
mod expressions;
mod imports;
mod layout;
mod syntax;

pub use directives::Directives;

use clap::builder::PossibleValuesParser;
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::{Node, Point};

const DECLARATIONS: &[&str] = &[
    "attributes",
    "discouraged_default_parameter",
    "discouraged_optional_boolean",
    "discouraged_optional_collection",
    "explicit_acl",
    "explicit_type_interface",
    "file_name",
    "file_types_order",
    "lower_acl_than_parent",
    "missing_docs",
    "no_grouping_extension",
    "one_declaration_per_file",
    "redundant_type_annotation",
    "required_deinit",
    "sorted_enum_cases",
    "strict_fileprivate",
    "type_contents_order",
    "type_name",
];
const EXPRESSIONS: &[&str] = &[
    "accessibility_label_for_image",
    "accessibility_trait_for_button",
    "anonymous_argument_in_multiline_closure",
    "function_default_parameter_at_end",
    "function_parameter_count",
    "implicit_return",
    "legacy_objc_type",
    "multiple_closures_with_trailing_closure",
    "no_empty_block",
    "no_magic_numbers",
    "optional_enum_case_matching",
    "prefer_condition_list",
    "prefer_key_path",
    "prefer_self_in_static_references",
    "private_swiftui_state",
    "trailing_closure",
    "unneeded_synthesized_initializer",
    "variable_shadowing",
];
const LAYOUT: &[&str] = &[
    "closure_body_length",
    "closure_end_indentation",
    "closure_parameter_position",
    "conditional_returns_on_newline",
    "cyclomatic_complexity",
    "file_length",
    "function_body_length",
    "indentation_width",
    "let_var_whitespace",
    "line_length",
    "multiline_arguments",
    "multiline_call_arguments",
    "multiline_function_chains",
    "multiline_literal_brackets",
    "nesting",
    "number_separator",
    "opening_brace",
    "orphaned_doc_comment",
    "switch_case_on_newline",
    "trailing_comma",
    "type_body_length",
];
const OTHER: &[&str] = &[
    "blanket_disable_command",
    "superfluous_disable_command",
    "unused_declaration",
    "unused_import",
];

fn identifiers() -> impl Iterator<Item = &'static str> {
    [DECLARATIONS, EXPRESSIONS, LAYOUT, OTHER]
        .into_iter()
        .flatten()
        .copied()
}

pub fn rule_parser() -> PossibleValuesParser {
    PossibleValuesParser::new(identifiers())
}

#[derive(Default)]
pub struct Selection(HashSet<String>);

impl Selection {
    pub fn new(names: Vec<String>) -> Self {
        Self(names.into_iter().collect())
    }

    pub fn enabled(&self, rule: &str) -> bool {
        self.0.is_empty() || self.0.contains(rule)
    }

    pub fn only(&self, rule: &str) -> bool {
        self.0.len() == 1 && self.0.contains(rule)
    }

    fn any(&self, group: &[&str]) -> bool {
        self.0.is_empty() || group.iter().any(|&rule| self.0.contains(rule))
    }
}

pub struct Context<'a> {
    source: &'a str,
    path: &'a Path,
    root: Node<'a>,
    nodes: Vec<Node<'a>>,
    lines: Vec<&'a str>,
    line_starts: Vec<usize>,
    selection: &'a Selection,
}

impl<'a> Context<'a> {
    pub fn new(source: &'a str, path: &'a Path, root: Node<'a>, selection: &'a Selection) -> Self {
        let mut cursor = root.walk();
        let mut nodes = Vec::new();
        'visit: loop {
            let node = cursor.node();
            if node.is_named() {
                nodes.push(node);
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break 'visit;
                }
            }
        }
        let mut lines = Vec::new();
        let mut line_starts = Vec::new();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            line_starts.push(offset);
            offset += line.len();
            let content = line.strip_suffix('\n').unwrap_or(line);
            lines.push(content.strip_suffix('\r').unwrap_or(content));
        }
        Self {
            source,
            path,
            root,
            selection,
            nodes,
            lines,
            line_starts,
        }
    }

    fn text(&self, node: Node<'_>) -> &'a str {
        &self.source[node.byte_range()]
    }

    fn enabled(&self, rule: &str) -> bool {
        self.selection.enabled(rule)
    }

    pub fn check(&self, usage: &crate::index::ImportUsage, out: &mut Findings) -> Directives {
        if self.selection.any(DECLARATIONS) {
            declarations::check(self, out);
        }
        if self.selection.any(EXPRESSIONS) {
            expressions::check(self, out);
        }
        if self.selection.any(LAYOUT) {
            layout::check(self, out);
        }
        if self.enabled("unused_import") {
            imports::check(self, usage, out);
        }
        Directives::from_comments(self)
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
pub struct Finding {
    pub point: Point,
    pub rule: &'static str,
    pub message: String,
}

pub struct Findings<'a> {
    selection: &'a Selection,
    pub items: Vec<Finding>,
}

impl<'a> Findings<'a> {
    pub fn new(selection: &'a Selection) -> Self {
        Self {
            selection,
            items: Vec::new(),
        }
    }

    fn at(&mut self, node: Node<'_>, rule: &'static str, message: impl Into<String>) {
        self.push(node.start_position(), rule, message);
    }

    pub fn push(&mut self, point: Point, rule: &'static str, message: impl Into<String>) {
        if self.selection.enabled(rule) {
            self.items.push(Finding {
                point,
                rule,
                message: message.into(),
            });
        }
    }
}
