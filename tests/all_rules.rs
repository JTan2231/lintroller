mod common;
#[path = "all_rules/expectations.rs"]
mod expectations;
#[path = "all_rules/layout.rs"]
mod layout;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const GROUPS: &[&str] = &[
    "declarations",
    "expressions",
    "layout",
    "references",
    "directives",
];

const FIXABLE_RULES: &[&str] = &[
    "attributes",
    "closure_end_indentation",
    "closure_parameter_position",
    "conditional_returns_on_newline",
    "explicit_acl",
    "file_types_order",
    "implicit_return",
    "let_var_whitespace",
    "multiline_arguments",
    "multiline_call_arguments",
    "multiline_function_chains",
    "multiline_literal_brackets",
    "number_separator",
    "opening_brace",
    "required_deinit",
    "superfluous_disable_command",
    "switch_case_on_newline",
    "trailing_comma",
    "unused_import",
];

fn swift_sources(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "swift") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn compile(command: &mut Command) -> io::Result<()> {
    let result = command.output()?;
    assert!(
        result.status.success(),
        "{command:?} failed:\n{}",
        String::from_utf8_lossy(&result.stderr),
    );
    Ok(())
}

fn build_imported_modules(fixtures: &Path, modules: &Path) -> io::Result<()> {
    fs::create_dir(modules)?;
    for source in swift_sources(&fixtures.join("references/modules"))? {
        let name = source.file_stem().unwrap();
        compile(
            Command::new("swiftc")
                .current_dir(modules)
                .arg("-emit-module")
                .arg("-module-name")
                .arg(name)
                .arg("-emit-module-path")
                .arg(modules.join(name).with_extension("swiftmodule"))
                .arg(&source),
        )?;
    }
    Ok(())
}

fn build_feature(work: &Path, modules: &Path, index: &Path, sources: &[PathBuf]) -> io::Result<()> {
    compile(
        Command::new("swiftc")
            .current_dir(work)
            .args([
                "-emit-bc",
                "-whole-module-optimization",
                "-num-threads",
                "4",
            ])
            .args([
                "-module-name",
                "FeatureFixture",
                "-index-ignore-system-modules",
            ])
            .arg("-I")
            .arg(modules)
            .arg("-index-store-path")
            .arg(index)
            .args(sources),
    )
}

fn lint(project: &Path, index: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lintroller"));
    command
        .arg("--project-root")
        .arg(project)
        .arg("--index-store")
        .arg(index);
    command
}

fn verify_autofixes(
    work: &Path,
    project: &Path,
    modules: &Path,
    index: &Path,
    sources: &[PathBuf],
) -> io::Result<()> {
    let fixed = lint(project, index).arg("--fix").output()?;
    assert_eq!(
        fixed.status.code(),
        Some(1),
        "fixing a fixture with remaining findings:\n{}{}",
        String::from_utf8_lossy(&fixed.stdout),
        String::from_utf8_lossy(&fixed.stderr),
    );

    let rebuilt_index = work.join("fixed-index");
    build_feature(work, modules, &rebuilt_index, sources)?;
    let scan = lint(project, &rebuilt_index).output()?;
    let diagnostics = String::from_utf8_lossy(&scan.stdout);
    assert_eq!(
        scan.status.code(),
        Some(1),
        "fixed fixture scan:\n{diagnostics}{}",
        String::from_utf8_lossy(&scan.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&fixed.stdout), diagnostics);
    let fixable: Vec<_> = diagnostics
        .lines()
        .filter(|line| {
            line.strip_suffix(')')
                .and_then(|line| line.rsplit_once(" ("))
                .is_some_and(|(_, rule)| FIXABLE_RULES.contains(&rule))
        })
        .collect();
    assert!(
        fixable.is_empty(),
        "remaining fixable diagnostics:\n{}",
        fixable.join("\n"),
    );

    let directives = fs::read_to_string(project.join("directives/FeatureCodes.swift"))?;
    assert!(
        directives.contains("return true"),
        "a suppressed finding was fixed",
    );
    assert!(directives.contains("// These two codes are reserved."));
    Ok(())
}

#[test]
fn feature_cleanup_resolves_all_61_rules() -> io::Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/all_rules");
    let temp = common::TempDir::new()?;
    let modules = temp.0.join("modules");
    build_imported_modules(&fixtures, &modules)?;

    for revision in ["before", "after"] {
        let work = temp.0.join(revision);
        let project = work.join("Feature");
        let index = work.join("index");
        let mut sources = Vec::new();
        let mut expected = BTreeSet::new();
        let mut covered_rules = BTreeSet::new();
        for &group in GROUPS {
            let fixture = fixtures.join(group);
            let destination = project.join(group);
            fs::create_dir_all(&destination)?;
            let mut files = swift_sources(&fixture.join(revision))?;
            let shared = fixture.join("shared");
            if shared.is_dir() {
                files.extend(swift_sources(&shared)?);
            }
            for file in files {
                fs::copy(&file, destination.join(file.file_name().unwrap()))?;
            }
            if group == "layout" {
                layout::generate(&destination, revision == "before")?;
            }
            sources.extend(swift_sources(&destination)?);

            // Stand-ins and test callers participate in the compiler index but
            // live outside the source tree that lintroller reports on.
            let support = fixture.join("support");
            if support.is_dir() {
                sources.extend(swift_sources(&support)?);
            }
            if revision == "before" {
                expectations::read(
                    &fixture.join("before/expected.tsv"),
                    &destination,
                    group,
                    &mut expected,
                    &mut covered_rules,
                )?;
            }
        }

        build_feature(&work, &modules, &index, &sources)?;
        let scan = lint(&project, &index).output()?;
        let stdout = String::from_utf8(scan.stdout).expect("diagnostics must be UTF-8");
        let expected_status = if revision == "before" { 1 } else { 0 };
        assert_eq!(
            scan.status.code(),
            Some(expected_status),
            "{revision} scan:\n{stdout}{}",
            String::from_utf8_lossy(&scan.stderr),
        );
        if revision == "before" {
            assert_eq!(
                covered_rules.len(),
                61,
                "fixture coverage: {covered_rules:?}"
            );
            expectations::compare(expected, &stdout);
            verify_autofixes(&work, &project, &modules, &index, &sources)?;
        } else {
            assert!(stdout.is_empty(), "corrected feature emitted:\n{stdout}");
        }
    }
    Ok(())
}

#[test]
fn implicit_return_preserves_comments_and_required_returns() -> io::Result<()> {
    let temp = common::TempDir::new()?;
    let file = temp.0.join("Returns.swift");
    let index = temp.0.join("index");
    let before = "\
func ordinary() -> Int { return /* value */ 42 }
let ordinaryClosure: () -> Int = { return 42 }
struct Ordinary {
    var shorthand: Int { return 42 }
    var explicitGetter: Int { get { return 42 } }
}
func bareReturn() { return }
struct Failable { init?() { return nil } }
@resultBuilder enum Offset {
    static func buildBlock(_ value: Int) -> Int { value + 1 }
}
@Offset func withBuilder() -> Int { return 42 }
func acceptsBuilder(@Offset _ operation: () -> Int) -> Int { operation() }
let contextualClosure = acceptsBuilder { return 42 }
protocol Provider { @Offset var result: Int { get } }
struct Witness: Provider { var result: Int { return 42 } }
";
    fs::write(&file, before)?;
    compile(
        Command::new("swiftc")
            .current_dir(&temp.0)
            .arg("-emit-bc")
            .arg("-index-store-path")
            .arg(&index)
            .arg(&file),
    )?;

    let fixed = lint(&temp.0, &index)
        .args(["--fix", "--rules", "implicit_return"])
        .output()?;
    assert_eq!(
        fixed.status.code(),
        Some(1),
        "remaining return diagnostics:\n{}{}",
        String::from_utf8_lossy(&fixed.stdout),
        String::from_utf8_lossy(&fixed.stderr),
    );
    let expected = before
        .replace("return /* value */ 42", "/* value */ 42")
        .replace("= { return 42 }", "= { 42 }")
        .replace("shorthand: Int { return 42 }", "shorthand: Int { 42 }")
        .replace("get { return 42 }", "get { 42 }");
    assert_eq!(fs::read_to_string(&file)?, expected);
    compile(Command::new("swiftc").arg("-typecheck").arg(&file))
}

#[test]
fn layout_fixes_preserve_crlf_comments_and_punctuation() -> io::Result<()> {
    let temp = common::TempDir::new()?;
    let file = temp.0.join("LayoutEdges.swift");
    let index = temp.0.join("index");
    let before = r#"func call(_ values: Int...) {}
func demo() -> Int
{
    call(
        1 // argument comment
        , 2,
        3 +
            4, 5
    )
    let number = 10_00.1_2e0_0
    let mapped = Optional(1).map {
        [offset = 2,
         factor = 3]
        (value: Int)
        in
        value * factor + offset
        } // closure comment
    let values = ["a": 1,
        "b": 2,]
    let chain = "ABCD".uppercased()
        .lowercased().first.map(String.init)?.dropFirst()
    if number > 0 { return (mapped ?? 0) + values.count + (chain?.count ?? 0) } else { return 0 }
} // declaration comment
let example = 1000
func keepBrace() // brace comment
{
    call(example)
}
"#;
    fs::write(&file, before.replace('\n', "\r\n"))?;
    compile(
        Command::new("swiftc")
            .current_dir(&temp.0)
            .arg("-emit-bc")
            .arg("-index-store-path")
            .arg(&index)
            .arg(&file),
    )?;

    let rules = [
        "closure_end_indentation",
        "closure_parameter_position",
        "conditional_returns_on_newline",
        "let_var_whitespace",
        "multiline_call_arguments",
        "multiline_function_chains",
        "multiline_literal_brackets",
        "number_separator",
        "opening_brace",
        "trailing_comma",
    ];
    let result = lint(&temp.0, &index)
        .args(["--fix", "--rules", &rules.join(",")])
        .output()?;
    assert_eq!(result.status.code(), Some(1));
    let diagnostics = String::from_utf8_lossy(&result.stdout);
    assert_eq!(diagnostics.lines().count(), 1, "{diagnostics}");
    assert!(diagnostics.contains("(opening_brace)"), "{diagnostics}");

    let fixed = fs::read_to_string(&file)?;
    assert!(!fixed.replace("\r\n", "").contains(['\r', '\n']));
    for expected in [
        "func demo() -> Int {",
        "1, // argument comment\r\n        2,",
        "3 +\r\n            4,\r\n        5",
        "1_000.12e0_0",
        "{ [offset = 2, factor = 3] (value: Int) in",
        "\r\n    } // closure comment",
        "[\r\n        \"a\": 1,\r\n        \"b\": 2\r\n    ]",
        ".map(String.init)?\r\n        .dropFirst()",
        "else {\r\n        return 0",
        "} // declaration comment\r\n\r\nlet example = 1_000",
        "func keepBrace() // brace comment\r\n{",
    ] {
        assert!(
            fixed.contains(expected),
            "missing {expected:?} in:\n{fixed}"
        );
    }
    compile(Command::new("swiftc").args(["-typecheck"]).arg(&file))
}
