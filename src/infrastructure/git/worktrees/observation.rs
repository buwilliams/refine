//! Content observations use an isolated index and never stage the working checkout.
use super::*;

impl FileGitWorktreeService {
    pub(crate) fn commit_tree(&self, commit: &str) -> RefineResult<String> {
        Ok(String::from_utf8_lossy(
            &self
                .git_output(&["rev-parse", "--verify", &format!("{commit}^{{tree}}")])?
                .stdout,
        )
        .trim()
        .to_string())
    }

    pub(crate) fn observed_worktree_tree(&self) -> RefineResult<String> {
        let path = self.git_path(&format!(
            "refine-observation-{}.index",
            uuid::Uuid::new_v4()
        ))?;
        let name = path.to_string_lossy();
        let env = [("GIT_INDEX_FILE", name.as_ref())];
        let result = (|| {
            self.git_output_with_env(&["read-tree", "HEAD"], &env)?;
            self.git_output_with_env(&["add", "-A", "--", ":/"], &env)?;
            Ok(
                String::from_utf8_lossy(&self.git_output_with_env(&["write-tree"], &env)?.stdout)
                    .trim()
                    .to_string(),
            )
        })();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.lock", path.display()));
        result
    }
}
