//! A checkout-local, process-lifetime guard, shared by workflow and Event agents.
use crate::error::{RefineError, RefineResult};
use fs2::FileExt;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::path::PathBuf;

thread_local! { static HELD: RefCell<BTreeMap<PathBuf, usize>> = const { RefCell::new(BTreeMap::new()) }; }

pub struct WorkspaceLease {
    file: Option<File>,
    path: PathBuf,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl WorkspaceLease {
    pub fn acquire(cwd: &Path) -> RefineResult<Self> {
        // Locate this checkout's Git directory (a linked worktree has a .git file).
        let root = cwd
            .ancestors()
            .find(|p| p.join(".git").exists())
            .unwrap_or(cwd);
        let dotgit = root.join(".git");
        let directory = if dotgit.is_file() {
            let contents =
                std::fs::read_to_string(&dotgit).map_err(|e| RefineError::Io(e.to_string()))?;
            let path = contents.trim().strip_prefix("gitdir: ").ok_or_else(|| {
                RefineError::InvalidInput("invalid worktree Git directory".into())
            })?;
            root.join(path)
        } else if dotgit.is_dir() {
            dotgit
        } else {
            root.join(".refine/runtime")
        };
        std::fs::create_dir_all(&directory).map_err(|e| RefineError::Io(e.to_string()))?;
        let path = directory
            .canonicalize()
            .map_err(|e| RefineError::Io(e.to_string()))?
            .join("refine-workspace.lock");
        if HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(depth) = held.get_mut(&path) {
                *depth += 1;
                true
            } else {
                false
            }
        }) {
            return Ok(Self {
                file: None,
                path,
                _thread: std::marker::PhantomData,
            });
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| RefineError::Io(e.to_string()))?;
        file.try_lock_exclusive().map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                RefineError::Degraded(
                    "workspace is in use by another managed operation; retry when it is available"
                        .into(),
                )
            } else {
                RefineError::Io(e.to_string())
            }
        })?;
        HELD.with(|held| {
            held.borrow_mut().insert(path.clone(), 1);
        });
        Ok(Self {
            file: Some(file),
            path,
            _thread: std::marker::PhantomData,
        })
    }
}
impl Drop for WorkspaceLease {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(depth) = held.get_mut(&self.path) {
                *depth -= 1;
                if *depth == 0 {
                    held.remove(&self.path);
                }
            }
        });
        if let Some(file) = &self.file {
            let _ = FileExt::unlock(file);
        }
    }
}
