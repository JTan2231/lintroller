mod index;
mod policy;
mod rewrite;
mod rules;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use tree_sitter::{Parser as SwiftParser, Point};

use policy::SourceFile;
use rewrite::Correction;
use rules::{Context, Directives, Findings, FixPlan, Selection};

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

    /// Apply available fixes in place and report remaining findings.
    #[arg(long)]
    fix: bool,
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

fn correct<'a>(
    path: &Path,
    original: SourceFile,
    parser: &mut SwiftParser,
    selection: &'a Selection,
    mut indexed: Findings<'a>,
) -> Result<(Findings<'a>, Option<Correction>), String> {
    let mut updated = None;
    for _ in 0..32 {
        let file = updated.as_ref().unwrap_or(&original);
        let source = file.source();
        let mut report = Findings::new(selection, true);
        let directives = Context::new(source, path, file.root(), selection).check(&mut report);
        report.items.extend(indexed.items.iter().cloned());
        directives.apply(&mut report, Some(source));
        let plan = FixPlan::new(source, &directives, &report.items);
        let mut remapped = indexed.items.clone();
        if let Some(replacement) = plan
            .apply(source, &mut remapped)
            .filter(|text| text != source)
        {
            let next = SourceFile::parse(replacement, parser)?;
            if file.root().has_error() || !next.root().has_error() {
                updated = Some(next);
                indexed.items = remapped;
                continue;
            }
        }
        let correction = updated.map(|file| Correction {
            original: original.into_source(),
            updated: file.into_source(),
        });
        return Ok((report, correction));
    }
    Err("auto-fixes did not converge".into())
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
    let mut parser = SourceFile::parser()?;
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
        let file = SourceFile::read(&path, &mut parser).map_err(at_path)?;
        let mut findings = Findings::new(&selection, false);
        let mut indexed_findings = Findings::new(&selection, args.fix);
        let directives = if declarations_only && !file.source().contains("swiftlint:") {
            Directives::default()
        } else {
            let context = Context::new(file.source(), &relative, file.root(), &selection);
            context.check_imports(&indexed.imports, &mut indexed_findings);
            if args.fix {
                Directives::default()
            } else {
                context.check(&mut findings)
            }
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
        reports.push((
            relative,
            findings,
            directives,
            indexed_findings,
            pending,
            args.fix.then_some(file),
        ));
    }

    let mut findings = String::new();
    let mut corrections = Vec::new();
    for (path, mut report, directives, mut indexed, pending, file) in reports {
        for (point, name, usrs) in pending {
            if usrs.iter().all(|usr| !referenced.contains(usr)) {
                indexed.push(
                    point,
                    "unused_declaration",
                    format!("Unused declaration '{name}'"),
                );
            }
        }
        if let Some(file) = file {
            let (corrected, correction) = correct(&path, file, &mut parser, &selection, indexed)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            report = corrected;
            if let Some(correction) = correction {
                corrections.push((root.join(&path), correction));
            }
        } else {
            report.items.extend(indexed.items);
            directives.apply(&mut report, None);
        }
        report.items.sort_unstable_by(|a, b| {
            (a.point, a.rule, &a.message).cmp(&(b.point, b.rule, &b.message))
        });
        report
            .items
            .dedup_by(|a, b| a.point == b.point && a.rule == b.rule && a.message == b.message);
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
    rewrite::write(corrections)?;
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
