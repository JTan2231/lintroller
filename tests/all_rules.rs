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
    "declarations", "expressions", "layout", "references", "directives",
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

        compile(
            Command::new("swiftc")
                .current_dir(&work)
                .args(["-emit-bc", "-whole-module-optimization", "-num-threads", "4"])
                .args(["-module-name", "FeatureFixture", "-index-ignore-system-modules"])
                .arg("-I")
                .arg(&modules)
                .arg("-index-store-path")
                .arg(&index)
                .args(&sources),
        )?;
        let scan = Command::new(env!("CARGO_BIN_EXE_lintroller"))
            .arg("--project-root")
            .arg(&project)
            .arg("--index-store")
            .arg(&index)
            .output()?;
        let stdout = String::from_utf8(scan.stdout).expect("diagnostics must be UTF-8");
        let expected_status = if revision == "before" { 1 } else { 0 };
        assert_eq!(
            scan.status.code(),
            Some(expected_status),
            "{revision} scan:\n{stdout}{}",
            String::from_utf8_lossy(&scan.stderr),
        );
        if revision == "before" {
            assert_eq!(covered_rules.len(), 61, "fixture coverage: {covered_rules:?}");
            expectations::compare(expected, &stdout);
        } else {
            assert!(stdout.is_empty(), "corrected feature emitted:\n{stdout}");
        }
    }
    Ok(())
}
