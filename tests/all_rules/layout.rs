use std::fs;
use std::io;
use std::path::Path;

/// The violating fixtures exceed each threshold by one. Their cleaned counterparts
/// sit exactly at the limit, so either direction of an off-by-one error is observable.
pub fn generate(root: &Path, before: bool) -> io::Result<()> {
    let extra = usize::from(before);

    let mut physical = String::from(
        "/// Marks a source file with a known line count.\npublic func layoutFileSize() -> Bool { true }\n",
    );
    let prefix = "// Width boundary ";
    physical.push_str(prefix);
    physical.push_str(&"x".repeat(120 + extra - prefix.len()));
    physical.push('\n');
    physical.push_str(&"// Physical line.\n".repeat(400 + extra - 3));
    fs::write(root.join("LayoutFileSize.swift"), physical)?;

    let mut bodies = String::from(
        "/// Applies the supplied step repeatedly.\npublic func layoutLongFunction(_ step: () -> Void) {\n",
    );
    bodies.push_str(&"    step()\n".repeat(50 + extra));
    bodies.push_str(
        "}\n\n/// Runs a closure with a known number of source lines.\npublic func layoutLongClosure() {\n    let emit: () -> Void = {\n",
    );
    bodies.push_str(&"        print()\n".repeat(30 + extra));
    bodies.push_str(
        "    }\n    emit()\n}\n\n/// Exercises independent branches.\npublic func layoutBranches(_ enabled: Bool, step: () -> Void) {\n",
    );
    bodies.push_str(&"    if enabled { step() }\n".repeat(10 + extra));
    bodies.push_str("}\n");
    fs::write(root.join("LayoutBoundaries.swift"), bodies)?;

    // The multiline string occupies the type body without introducing a long
    // function. Its opening and closing delimiters each count as a code line.
    let mut type_body = String::from(
        "/// Contains the type-length boundary sample.\npublic enum LayoutBody {\n    /// A multiline sample.\n    public static let text: String = \"\"\"\n",
    );
    type_body.push_str(&"        sample\n".repeat(250 + extra - 2));
    type_body.push_str("        \"\"\"\n}\n");
    fs::write(root.join("LayoutBody.swift"), type_body)
}
