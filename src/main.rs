mod index;
mod policy;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

/// Find unused Swift declarations in an existing compiler index.
#[derive(Parser)]
#[command(version)]
struct Arguments {
    /// Compiler index store directory passed to swiftc -index-store-path.
    #[arg(long, value_name = "DIR")]
    index_store: PathBuf,

    /// Root of the Swift source tree to report findings in.
    #[arg(long, value_name = "DIR")]
    project_root: PathBuf,
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
    let mut candidates = index::read(&store, &root)?;
    candidates.sort_unstable_by(|a, b| {
        (&a.path, a.line, a.column, &a.name).cmp(&(&b.path, b.line, b.column, &b.name))
    });

    // Finish checking the index and sources before publishing any diagnostics.
    let mut findings = String::new();
    for group in candidates.chunk_by(|a, b| a.path == b.path) {
        let path = &group[0].path;
        let relative = path.strip_prefix(&root).map_err(|e| e.to_string())?;
        let at_path = |err: String| format!("{}: {err}", relative.display());
        let file = policy::SourceFile::read(path).map_err(at_path)?;
        for candidate in group {
            if file
                .is_reportable(candidate.line, candidate.column, &candidate.name)
                .map_err(at_path)?
            {
                writeln!(
                    findings,
                    "{}:{}:{}: error: Unused declaration '{}' (unused_declaration)",
                    relative.display(),
                    candidate.line,
                    candidate.column,
                    candidate.name
                )
                .expect("writing to a String is infallible");
            }
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
