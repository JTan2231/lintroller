use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct Correction {
    pub original: String,
    pub updated: String,
}

struct TemporaryFile(PathBuf);

impl TemporaryFile {
    fn create(directory: &Path, sequence: &mut usize) -> io::Result<(Self, File)> {
        loop {
            *sequence += 1;
            let path = directory.join(format!(".lintroller-{}-{sequence}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(path), file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn unchanged(path: &Path, original: &str) -> io::Result<()> {
    if fs::read(path)? == original.as_bytes() {
        Ok(())
    } else {
        Err(io::Error::other("source changed during the scan"))
    }
}

pub fn write(corrections: Vec<(PathBuf, Correction)>) -> Result<(), String> {
    let mut sequence = 0;
    let mut staged = Vec::with_capacity(corrections.len());
    // Prepare each replacement before touching any source file.
    for (path, correction) in corrections {
        let mut stage = || -> io::Result<TemporaryFile> {
            unchanged(&path, &correction.original)?;
            let permissions = fs::metadata(&path)?.permissions();
            let (temporary, mut file) =
                TemporaryFile::create(path.parent().unwrap(), &mut sequence)?;
            file.write_all(correction.updated.as_bytes())?;
            file.set_permissions(permissions)?;
            Ok(temporary)
        };
        let temporary = stage().map_err(|err| format!("{}: {err}", path.display()))?;
        staged.push((path, correction.original, temporary));
    }
    for (path, original, temporary) in staged {
        unchanged(&path, &original)
            .and_then(|()| fs::rename(&temporary.0, &path))
            .map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(())
}
