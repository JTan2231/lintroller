use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

/// A TSV row names the Swift file, rule ID, a unique snippet identifying the
/// line, and a token starting at the expected byte column. Generated samples
/// and whitespace diagnostics can use an explicit @line:N or @column:N.
pub fn read(
    manifest: &Path,
    directory: &Path,
    group: &str,
    expected: &mut BTreeSet<String>,
    covered_rules: &mut BTreeSet<String>,
) -> io::Result<()> {
    for row in fs::read_to_string(manifest)?.lines() {
        let [file, rule, snippet, anchor]: [&str; 4] = row.split('\t').collect::<Vec<_>>()
            .try_into()
            .unwrap_or_else(|_| panic!("expected four tab-separated fields in {row:?}"));
        let source = fs::read_to_string(directory.join(file))?;
        let number: usize = if let Some(value) = snippet.strip_prefix("@line:") {
            value.parse().expect("expected a line number")
        } else {
            let mut matches = source.lines().enumerate().filter(|(_, line)| line.contains(snippet));
            let (index, _) = matches.next()
                .unwrap_or_else(|| panic!("no matching line for {row:?}"));
            assert!(matches.next().is_none(), "ambiguous line in {row:?}");
            index + 1
        };
        let line = source.lines().nth(number - 1).expect("line outside fixture");
        let column = if let Some(value) = anchor.strip_prefix("@column:") {
            value.parse().expect("expected a byte column")
        } else {
            line.find(anchor).unwrap_or_else(|| panic!("no column anchor for {row:?}")) + 1
        };
        covered_rules.insert(rule.to_string());
        let diagnostic = format!("{group}/{file}:{number}:{column} ({rule})");
        assert!(expected.insert(diagnostic), "duplicate expectation: {row}");
    }
    Ok(())
}

pub fn compare(mut missing: BTreeSet<String>, stdout: &str) {
    let mut unexpected = Vec::new();
    for line in stdout.lines() {
        let (location, message) = line.split_once(": error: ")
            .expect("expected a lintroller diagnostic");
        let (_, rule) = message.strip_suffix(')')
            .and_then(|s| s.rsplit_once(" ("))
            .expect("diagnostic must end with its rule ID");
        if !missing.remove(&format!("{location} ({rule})")) {
            unexpected.push(line);
        }
    }
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "missing findings:\n{}\nunexpected findings:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n"), unexpected.join("\n"),
    );
}
