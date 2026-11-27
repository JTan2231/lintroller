mod common;

use std::fs;
use std::io;
use std::process::Command;

#[test]
fn import_and_directive_fixes_preserve_comments_and_scope() -> io::Result<()> {
    let temp = common::TempDir::new()?;
    let source = temp.0.join("Scoped.swift");
    let index = temp.0.join("index");
    let before = "\
import Swift // Keep the import's comment.
// swiftlint:disable:next number_separator line_length - Keep this reason.
internal let first = 123456
// swiftlint:disable:next line_length
// swiftlint:disable:next number_separator
internal let second = 234567
/* swiftlint:disable:this line_length - Keep this block comment. */
internal let third = 345678
";
    fs::write(&source, before)?;
    let compile = Command::new("swiftc")
        .current_dir(&temp.0)
        .arg("-emit-bc")
        .arg("-index-store-path")
        .arg(&index)
        .arg(&source)
        .output()?;
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let fix = Command::new(env!("CARGO_BIN_EXE_lintroller"))
        .arg("--project-root")
        .arg(&temp.0)
        .arg("--index-store")
        .arg(&index)
        .args([
            "--fix",
            "--rules",
            "number_separator,line_length,unused_import,superfluous_disable_command",
        ])
        .output()?;
    assert!(
        fix.status.success(),
        "{}{}",
        String::from_utf8_lossy(&fix.stdout),
        String::from_utf8_lossy(&fix.stderr)
    );
    assert_eq!(
        fs::read_to_string(source)?,
        "\
// Keep the import's comment.
// swiftlint:disable:next number_separator - Keep this reason.
internal let first = 123456

// swiftlint:disable:next number_separator
internal let second = 234567
/* Keep this block comment. */
internal let third = 345_678
",
    );
    Ok(())
}
