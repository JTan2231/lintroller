use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> io::Result<Self> {
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
