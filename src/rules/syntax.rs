use tree_sitter::Node;

/// Direct children in source order, including punctuation and comments.
pub(super) fn children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    let mut more = cursor.goto_first_child();
    std::iter::from_fn(move || {
        if !more {
            return None;
        }
        let child = cursor.node();
        more = cursor.goto_next_sibling();
        Some(child)
    })
}

pub(super) fn named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    children(node).filter(|n| n.is_named())
}

/// Named children excluding grammar extras such as comments.
pub(super) fn syntax_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_children(node).filter(|n| !n.is_extra())
}

pub(super) fn child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    children(node).find(|n| n.kind() == kind)
}

pub(super) fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), |n| n.parent())
}

pub(super) fn declaration_keyword(node: Node<'_>) -> Node<'_> {
    node.child_by_field_name("declaration_kind")
        .unwrap_or_else(|| {
            children(node)
                .find(|n| {
                    matches!(
                        n.kind(),
                        "func"
                            | "init"
                            | "deinit"
                            | "subscript"
                            | "case"
                            | "import"
                            | "typealias"
                            | "associatedtype"
                            | "value_binding_pattern"
                            | "pattern"
                    )
                })
                .unwrap_or(node)
        })
}
