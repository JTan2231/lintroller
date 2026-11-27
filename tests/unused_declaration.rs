use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{self, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("lintroller-{}-{nonce}", process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path.canonicalize()?))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn finds_exactly_21_unused_declarations_in_a_hundred_file_project() -> io::Result<()> {
    let start = Instant::now();
    let temp = TempDir::new()?;
    let project = temp.0.join("Project");
    let sources = project.join("Sources");
    let index = temp.0.join("index");
    fs::create_dir_all(&sources)?;

    let entry = sources.join("Entry.swift");
    fs::write(
        &entry,
        "func choose(_ value: Int) {}
func choose(_ value: String) {}
func calledOnlyByGeneratedCode() {}
protocol Renderer { func draw() }
struct Screen: Renderer { func draw() {} }
class Base { func perform() {} }
final class Child: Base { override func perform() {} }
func invoke(_ value: Base) { value.perform() }
func left() { right() }
func right() { left() }
public func launch() {
    node000()
    choose(1)
    let renderer: any Renderer = Screen()
    renderer.draw()
    invoke(Child())
}
",
    )?;
    let mut files = vec![entry];
    let mut expected = vec![
        "Sources/Entry.swift:2:6: error: Unused declaration 'choose(_:)' (unused_declaration)"
            .to_string(),
    ];
    for i in 0..99 {
        let file_name = format!("Node{i:03}.swift");
        let file = sources.join(&file_name);
        let call = if i < 98 {
            format!("node{:03}()", i + 1)
        } else {
            String::new()
        };
        let mut source = format!("func node{i:03}() {{ {call} }}\n");
        if i == 0 {
            source.push_str("struct UnusedType {}\nextension UnusedType {}\n");
            expected.push(format!(
                "Sources/{file_name}:2:8: error: Unused declaration 'UnusedType' (unused_declaration)"
            ));
        } else if i % 5 == 0 {
            source.push_str(&format!("func unused{i:03}() {{}}\n"));
            expected.push(format!("Sources/{file_name}:2:6: error: Unused declaration 'unused{i:03}()' (unused_declaration)"));
        }
        fs::write(&file, source)?;
        files.push(file);
    }

    // References from outside the reporting root still count.
    let generated = temp.0.join("ProjectGenerated.swift");
    fs::write(
        &generated,
        "func generatedEntry() { calledOnlyByGeneratedCode() }\n",
    )?;
    files.push(generated);

    let build = Command::new("swiftc")
        .current_dir(&temp.0)
        .args([
            "-emit-bc",
            "-whole-module-optimization",
            "-num-threads",
            "4",
            "-module-name",
            "Fixture",
            "-index-ignore-system-modules",
            "-index-store-path",
        ])
        .arg(&index)
        .args(&files)
        .output()?;
    assert!(
        build.status.success(),
        "swiftc failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let scan_start = Instant::now();
    let scan = Command::new(env!("CARGO_BIN_EXE_lintroller"))
        .arg("--index-store")
        .arg(&index)
        .arg("--project-root")
        .arg(&project)
        .output()?;
    let scan_time = scan_start.elapsed();
    assert_eq!(
        scan.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let diagnostics = String::from_utf8(scan.stdout).unwrap();
    assert_eq!(
        expected.len(),
        21,
        "the fixture should contain 21 violations"
    );
    assert_eq!(diagnostics.lines().collect::<Vec<_>>(), expected);

    drop(temp);
    let total = start.elapsed();
    eprintln!(
        "100 project files, 21 violations. Scan {scan_time:.3?}, including fixture and index {total:.3?}"
    );
    assert!(
        total < Duration::from_secs(10),
        "test took {total:.3?} (scan {scan_time:.3?})"
    );
    Ok(())
}
