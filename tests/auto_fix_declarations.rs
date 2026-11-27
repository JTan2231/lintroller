mod common;

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Output};

fn build_index(index: &Path, file: &Path) -> io::Result<()> {
    let compiled = Command::new("swiftc")
        .args([
            "-typecheck",
            "-index-ignore-system-modules",
            "-index-store-path",
        ])
        .arg(index)
        .arg(file)
        .output()?;
    assert!(compiled.status.success(), "{}", output(&compiled));
    Ok(())
}

fn fix(project: &Path, index: &Path, rules: &str) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_lintroller"))
        .arg("--project-root")
        .arg(project)
        .arg("--index-store")
        .arg(index)
        .args(["--fix", "--rules", rules])
        .output()
}

fn output(result: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

#[test]
fn fixes_declaration_headers_and_moves_comments_with_types() -> io::Result<()> {
    let temp = common::TempDir::new()?;
    let project = temp.0.join("project");
    fs::create_dir(&project)?;
    let path = project.join("Widget.swift");
    let index = temp.0.join("index");
    let source = r#"extension Widget {}

/// The widget.
@available(*, deprecated) // Keep this attribute note.

final class Widget { let value = 1 } // Keep this class note.

/// The widget's helper.
struct Helper {}

@inline(never) /* Keep this inline note. */ func answer() -> Int { 42 }

@MainActor /* Keep this property note. */
let name = "Widget"
"#;
    fs::write(&path, source.replace('\n', "\r\n"))?;
    build_index(&index, &path)?;

    let fixed = fix(
        &project,
        &index,
        "attributes,explicit_acl,file_types_order,required_deinit,no_empty_block",
    )?;
    assert!(fixed.status.success(), "{}", output(&fixed));
    let expected = r#"/// The widget's helper.
internal struct Helper {}

/// The widget.
@available(*, deprecated) // Keep this attribute note.
internal final class Widget { internal let value = 1

    deinit {
        // No explicit cleanup is required.
    }
} // Keep this class note.

extension Widget {}

@inline(never) /* Keep this inline note. */
internal func answer() -> Int { 42 }

@MainActor /* Keep this property note. */ internal let name = "Widget"
"#;
    assert_eq!(fs::read_to_string(&path)?, expected.replace('\n', "\r\n"));
    build_index(&temp.0.join("fixed-index"), &path)?;
    Ok(())
}

#[test]
fn declaration_order_fixes_respect_conditionals_and_ambiguous_primary_types() -> io::Result<()> {
    let temp = common::TempDir::new()?;
    let project = temp.0.join("project");
    fs::create_dir(&project)?;
    for (stem, source) in [
        (
            "Widget",
            "struct Widget {}\n#if DEBUG\nstruct Helper {}\n#endif\n",
        ),
        ("Unrelated", "struct First {}\nstruct Second {}\n"),
    ] {
        let path = project.join(format!("{stem}.swift"));
        let index = temp.0.join(stem);
        fs::write(&path, source)?;
        build_index(&index, &path)?;

        let fixed = fix(&project, &index, "file_types_order")?;
        assert_eq!(fixed.status.code(), Some(1), "{}", output(&fixed));
        let diagnostics = output(&fixed);
        assert!(
            diagnostics.contains(&format!("{stem}.swift:1:1:")),
            "{diagnostics}"
        );
        assert_eq!(fs::read_to_string(path)?, source);
    }
    Ok(())
}
