mod index;
mod policy;
mod rules;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use tree_sitter::Point;

use rules::{Context, Directives, Findings, Selection};

/// Lint Swift project sources from an existing compiler index.
#[derive(Parser)]
#[command(version)]
struct Arguments {
    /// Compiler index store directory passed to swiftc -index-store-path.
    #[arg(long, value_name = "DIR")]
    index_store: PathBuf,

    /// Root of the Swift source tree to report findings in.
    #[arg(long, value_name = "DIR")]
    project_root: PathBuf,

    /// Run only these rule identifiers (repeat or separate with commas). Defaults to all 61.
    #[arg(long, value_delimiter = ',', value_name = "ID", value_parser = rules::rule_parser())]
    rules: Vec<String>,
}

fn directory(path: &Path) -> Result<PathBuf, String> {
    let absolute = path
        .canonicalize()
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if !absolute.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(absolute)
}

fn scan(args: Arguments) -> Result<String, String> {
    let root = directory(&args.project_root)?;
    let store = directory(&args.index_store)?;
    let selection = Selection::new(args.rules);
    let sources = index::read(
        &store,
        &root,
        selection.enabled("unused_declaration"),
        selection.enabled("unused_import"),
    )?;
    let mut parser = policy::SourceFile::parser()?;
    let mut referenced = HashSet::new();
    let mut reports = Vec::new();
    let declarations_only = selection.only("unused_declaration");
    for (path, indexed) in sources {
        if declarations_only && indexed.candidates.is_empty() && indexed.bare_types.is_empty() {
            continue;
        }
        let relative = path
            .strip_prefix(&root)
            .map_err(|e| e.to_string())?
            .to_owned();
        let at_path = |err: String| format!("{}: {err}", relative.display());
        let file = policy::SourceFile::read(&path, &mut parser).map_err(at_path)?;
        let mut findings = Findings::new(&selection);
        let directives = if declarations_only && !file.source().contains("swiftlint:") {
            Directives::default()
        } else {
            Context::new(file.source(), &relative, file.root(), &selection)
                .check(&indexed.imports, &mut findings)
        };

        // Resolve ambiguous type references against the same tree as the source
        // rules. A use in a later file can still keep an earlier candidate.
        for reference in indexed.bare_types {
            if !file.is_extension_target(reference.line, reference.column) {
                referenced.insert(reference.usr);
            }
        }
        let mut pending = Vec::new();
        for (decl, usrs) in indexed.candidates {
            if file
                .is_reportable(decl.line, decl.column, &decl.name)
                .map_err(at_path)?
            {
                let point = Point::new(decl.line as usize - 1, decl.column as usize - 1);
                pending.push((point, decl.name, usrs));
            }
        }
        reports.push((relative, findings, directives, pending));
    }

    let mut findings = String::new();
    for (path, mut report, directives, pending) in reports {
        for (point, name, usrs) in pending {
            if usrs.iter().all(|usr| !referenced.contains(usr)) {
                report.push(
                    point,
                    "unused_declaration",
                    format!("Unused declaration '{name}'"),
                );
            }
        }
        directives.apply(&mut report);
        report.items.sort_unstable();
        report.items.dedup();
        for finding in report.items {
            writeln!(
                findings,
                "{}:{}:{}: error: {} ({})",
                path.display(),
                finding.point.row + 1,
                finding.point.column + 1,
                finding.message,
                finding.rule
            )
            .expect("writing to a String is infallible");
        }
    }
    Ok(findings)
}

fn run() -> Result<ExitCode, String> {
    let findings = scan(Arguments::parse())?;
    io::stdout()
        .lock()
        .write_all(findings.as_bytes())
        .map_err(|err| format!("writing diagnostics: {err}"))?;
    Ok(if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    run().unwrap_or_else(|err| {
        eprintln!("lintroller: {err}");
        ExitCode::from(2)
    })
}
