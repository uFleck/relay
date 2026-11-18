//! Read-only git context for a project checkout (PLAN 3.5). Shells out to `git`; no
//! libgit2, so there is nothing to keep in sync with the user's install.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("cannot run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("{path} is not a git repository")]
    NotARepository { path: PathBuf },
    #[error("git {command} failed: {stderr}")]
    Failed {
        command: &'static str,
        stderr: String,
    },
}

/// What the task sheet shows for a project with `repo_path`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub head_subject: Option<String>,
    pub dirty: bool,
}

/// Reads the current branch, commit and dirty flag. A missing path or a non-repo is an
/// error the caller shows as "no repo"; it is never fatal to the app.
pub fn state(repo: &Path) -> Result<GitState, GitError> {
    if !repo.exists() {
        return Err(GitError::NotARepository {
            path: repo.to_path_buf(),
        });
    }
    let inside = match run(repo, "rev-parse", &["--is-inside-work-tree"]) {
        Ok(value) => value,
        Err(GitError::Failed { .. }) => {
            return Err(GitError::NotARepository {
                path: repo.to_path_buf(),
            });
        }
        Err(error) => return Err(error),
    };
    if inside.trim() != "true" {
        return Err(GitError::NotARepository {
            path: repo.to_path_buf(),
        });
    }
    let branch = run(repo, "rev-parse", &["--abbrev-ref", "HEAD"])
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty());
    let head = run(repo, "rev-parse", &["HEAD"])
        .map(|value| value.trim().to_string())
        .ok();
    let head_subject = run(repo, "log", &["-1", "--pretty=%s"])
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty());
    let dirty = !run(repo, "status", &["--porcelain"])?.trim().is_empty();
    Ok(GitState {
        branch,
        head,
        head_subject,
        dirty,
    })
}

pub(crate) fn run(repo: &Path, command: &'static str, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg(command)
        .args(args)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitError::Failed { command, stderr });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("relay-git-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "relay@test"]);
        git(&["config", "user.name", "Relay Test"]);
        path
    }

    #[test]
    fn reads_branch_head_and_dirty_state() {
        let repo = temp_repo();
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "first"]);

        let clean = state(&repo).unwrap();
        assert!(clean.branch.is_some(), "{clean:?}");
        assert_eq!(clean.head.as_deref().map(str::len), Some(40));
        assert_eq!(clean.head_subject.as_deref(), Some("first"));
        assert!(!clean.dirty);

        fs::write(repo.join("README.md"), "changed\n").unwrap();
        assert!(state(&repo).unwrap().dirty);

        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn a_plain_folder_is_not_a_repository() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("relay-plain-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        assert!(matches!(state(&path), Err(GitError::NotARepository { .. })));
        fs::remove_dir_all(&path).ok();
    }
}
