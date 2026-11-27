use std::{fs, path::Path};

use tree_sitter::{Node, Parser, Point, Tree};

pub struct SourceFile {
    source: String,
    tree: Tree,
}

impl SourceFile {
    pub fn parser() -> Result<Parser, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_swift::LANGUAGE.into())
            .map_err(|error| error.to_string())?;
        Ok(parser)
    }

    pub fn read(path: &Path, parser: &mut Parser) -> Result<Self, String> {
        let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(source, parser)
    }

    pub fn parse(source: String, parser: &mut Parser) -> Result<Self, String> {
        let tree = parser.parse(&source, None).ok_or("could not parse Swift")?;
        Ok(Self { source, tree })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn into_source(self) -> String {
        self.source
    }

    pub fn root(&self) -> Node<'_> {
        self.tree.root_node()
    }

    // Swift may emit a bare reference for the nominal type of an empty
    // extension. Generic arguments and qualifiers are still ordinary uses.
    pub fn is_extension_target(&self, line: u32, column: u32) -> bool {
        let Some(leaf) = self.indexed_token(line, column) else {
            return false;
        };
        let Some(ty) = leaf.parent() else {
            return false;
        };
        let Some(decl) = ty.parent() else {
            return false;
        };
        if decl.child_by_field_name("name") != Some(ty) || !self.is_extension(decl) {
            return false;
        }
        let mut cursor = ty.walk();
        ty.named_children(&mut cursor)
            .filter(|child| child.kind() == "type_identifier")
            .last()
            == Some(leaf)
    }

    fn indexed_token(&self, line: u32, column: u32) -> Option<Node<'_>> {
        let row = line.checked_sub(1)? as usize;
        let col = column.checked_sub(1)? as usize;
        self.tree
            .root_node()
            .descendant_for_point_range(Point::new(row, col), Point::new(row, col + 1))
    }

    // Both the compiler and tree-sitter measure Swift columns in UTF-8 bytes.
    pub fn is_reportable(&self, line: u32, column: u32, name: &str) -> Result<bool, String> {
        let stale = || {
            format!(
                "the current source does not match indexed declaration '{name}' at {line}:{column}. Rebuild the index"
            )
        };
        let row = line.checked_sub(1).ok_or_else(stale)? as usize;
        let col = column.checked_sub(1).ok_or_else(stale)? as usize;
        let point = Point::new(row, col);
        let leaf = self.indexed_token(line, column).ok_or_else(stale)?;
        let base = name.split('(').next().unwrap_or(name).trim_matches('`');
        let spelling = self.text(leaf);
        if spelling.trim_matches('`') != base {
            // A Swift feature newer than the parser can leave the declaration
            // inside an error node. Withhold it if the indexed name still fits.
            if leaf.has_error()
                && self
                    .source
                    .lines()
                    .nth(row)
                    .and_then(|line| line.get(col..))
                    .is_some_and(|rest| rest.trim_start_matches('`').starts_with(base))
            {
                return Ok(false);
            }
            return Err(stale());
        }
        let token_start = leaf.start_position();
        if token_start != point
            && !(spelling.starts_with('`')
                && token_start.row == row
                && token_start.column + 1 == col)
        {
            return Err(stale());
        }

        let Some(decl) = enclosing_declaration(leaf) else {
            // The parser can recover from a newer Swift construct without knowing its modifiers.
            return Ok(false);
        };
        let mut cursor = decl.walk();
        if !decl
            .children_by_field_name("name", &mut cursor)
            .any(|node| node.byte_range().contains(&leaf.start_byte()))
        {
            return Ok(false);
        }

        let modifiers = self.modifiers(decl);
        if self.is_public(decl, &modifiers)
            || modifiers.iter().any(|flag| {
                matches!(
                    *flag,
                    "override"
                        | "dynamic"
                        | "objc"
                        | "objcMembers"
                        | "IBAction"
                        | "IBSegueAction"
                        | "IBInspectable"
                        | "IBOutlet"
                        | "IBDesignable"
                        | "NSManaged"
                        | "main"
                        | "NSApplicationMain"
                        | "UIApplicationMain"
                        | "Test"
                        | "Suite"
                        | "_cdecl"
                        | "_silgen_name"
                )
            })
        {
            return Ok(false);
        }
        if modifiers.contains(&"static")
            && matches!(
                name,
                "buildBlock(_:)"
                    | "buildBlock()"
                    | "buildExpression(_:)"
                    | "buildOptional(_:)"
                    | "buildEither(first:)"
                    | "buildEither(second:)"
                    | "buildArray(_:)"
                    | "buildLimitedAvailability(_:)"
                    | "buildFinalResult(_:)"
                    | "buildPartialBlock(first:)"
                    | "buildPartialBlock(accumulated:next:)"
            )
        {
            return Ok(false);
        }
        if let Some(owner) = enclosing_declaration(decl) {
            let owner_modifiers = self.modifiers(owner);
            let owner_kind = owner
                .child_by_field_name("declaration_kind")
                .map(|node| self.text(node));
            if !modifiers.contains(&"nonobjc")
                && (owner_modifiers.contains(&"objcMembers")
                    || (owner_modifiers.contains(&"objc")
                        && matches!(owner_kind, Some("protocol" | "extension"))))
            {
                return Ok(false);
            }
            if owner_modifiers.contains(&"main")
                && name == "main()"
                && modifiers.contains(&"static")
            {
                return Ok(false);
            }
            if owner_modifiers.contains(&"propertyWrapper")
                && matches!(base, "wrappedValue" | "projectedValue")
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // A public type's ordinary members default to internal. Protocols and
    // extensions can give their members public access, even when nested.
    fn is_public(&self, decl: Node<'_>, modifiers: &[&str]) -> bool {
        if modifiers
            .iter()
            .any(|flag| matches!(*flag, "public" | "open"))
        {
            return true;
        }
        if modifiers
            .iter()
            .any(|flag| matches!(*flag, "private" | "fileprivate" | "internal" | "package"))
        {
            return false;
        }
        let Some(owner) = enclosing_declaration(decl) else {
            return false;
        };
        (self.is_extension(owner) || owner.kind() == "protocol_declaration")
            && self.is_public(owner, &self.modifiers(owner))
    }

    fn is_extension(&self, decl: Node<'_>) -> bool {
        decl.child_by_field_name("declaration_kind")
            .is_some_and(|kind| self.text(kind) == "extension")
    }

    fn text(&self, node: Node<'_>) -> &str {
        &self.source[node.byte_range()]
    }

    fn modifiers(&self, declaration: Node<'_>) -> Vec<&str> {
        let mut cursor = declaration.walk();
        let mut result = Vec::new();
        for child in declaration.named_children(&mut cursor) {
            match child.kind() {
                "modifiers" => {
                    let mut inner = child.walk();
                    result.extend(
                        child
                            .named_children(&mut inner)
                            .filter_map(|node| self.modifier(node)),
                    );
                }
                "attribute" => result.extend(self.modifier(child)),
                _ => {}
            }
        }
        result
    }

    fn modifier(&self, node: Node<'_>) -> Option<&str> {
        if node.kind() == "attribute" {
            // Attribute arguments can contain arbitrary Swift expressions.
            return self.text(node.named_child(0)?).rsplit('.').next();
        }
        Some(self.text(node))
    }
}

fn enclosing_declaration(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.parent(), |parent| parent.parent())
        .find(|parent| parent.kind().ends_with("_declaration"))
}
