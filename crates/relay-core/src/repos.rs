//! Local clones on GitHub: scan a folder for them and turn verified pull requests into
//! create-only Note Import candidates. Pure policy; the adapters only fetch and call.

use std::collections::HashSet;
use std::path::Path;

use crate::git::{self, GitError};
use crate::github::{FoundPull, PullRole};
use crate::v2::{
    GithubItemKind, ImportCandidate, ReferenceKey, ReferenceSnapshot, dedupe_candidates,
};

/// A GitHub repository. Case is kept for display; compare with [`GithubRepo::same_as`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    pub owner: String,
    pub name: String,
}

impl GithubRepo {
    pub fn same_as(&self, other: &GithubRepo) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCandidate {
    pub path: String,
    pub repo: GithubRepo,
    /// The project that already points at this path, when there is one.
    pub project_id: Option<String>,
}

/// `https://github.com/o/r(.git)`, `git@github.com:o/r(.git)` and
/// `ssh://git@github.com/o/r`. Other hosts are `None`.
pub fn parse_github_remote(url: &str) -> Option<GithubRepo> {
    let url = url.trim();
    const PREFIXES: [&str; 5] = [
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
        "git://github.com/",
    ];
    let lower = url.to_ascii_lowercase();
    let prefix = PREFIXES.iter().find(|prefix| lower.starts_with(*prefix))?;
    let rest = url[prefix.len()..].trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(GithubRepo {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// The GitHub repository behind a checkout's `origin`, if it has one.
pub fn origin_repo(path: &Path) -> Option<GithubRepo> {
    let url = git::run(path, "remote", &["get-url", "origin"]).ok()?;
    parse_github_remote(&url)
}

/// `root` itself and its immediate subdirectories that are git checkouts with a GitHub
/// `origin`, sorted by path. `project_id` is left empty.
pub fn scan_folder(root: &Path) -> Result<Vec<RepoCandidate>, GitError> {
    scan_folders(&[root])
}

/// [`scan_folder`] over several roots, keeping one checkout per repo across all of them.
pub fn scan_folders<P: AsRef<Path>>(roots: &[P]) -> Result<Vec<RepoCandidate>, GitError> {
    let mut dirs = Vec::new();
    for root in roots {
        let root = root.as_ref();
        dirs.push(root.to_path_buf());
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    let mut found: Vec<RepoCandidate> = dirs
        .into_iter()
        // `.git` is a file in worktrees and submodules, so test existence, not is_dir.
        .filter(|dir| dir.join(".git").exists())
        .filter_map(|dir| {
            Some(RepoCandidate {
                repo: origin_repo(&dir)?,
                path: dir.to_string_lossy().into_owned(),
                project_id: None,
            })
        })
        .collect();
    // Extra worktrees of one repo (`account-PAYMENT-10260`) share its origin; keep one checkout
    // per repo, preferring the main one (`.git` is a directory there) and then the shortest path.
    found.sort_by_key(|candidate| {
        (
            candidate.repo.owner.to_lowercase(),
            candidate.repo.name.to_lowercase(),
            !Path::new(&candidate.path).join(".git").is_dir(),
            candidate.path.len(),
        )
    });
    found.dedup_by(|later, kept| later.repo.same_as(&kept.repo));
    found.sort_by_key(|candidate| candidate.path.to_lowercase());
    Ok(found)
}

/// Builds create-only Note Import candidates from the verified `search_my_pulls` read. Only
/// registered v2 repos are included. Duplicate repo registrations and a PR that is both
/// authored and review-requested collapse to one candidate by canonical key. The provider's
/// exact title is kept for review requests; Import never rewrites it into a local task title.
pub fn import_candidates(
    found: &[FoundPull],
    registered_repos: &[String],
    cached_at: &str,
) -> Vec<ImportCandidate> {
    let allowed: HashSet<String> = registered_repos
        .iter()
        .map(|repo| repo.trim().to_ascii_lowercase())
        .collect();
    dedupe_candidates(
        found
            .iter()
            .filter_map(|pull| {
                let owner_repo =
                    format!("{}/{}", pull.repo.owner, pull.repo.name).to_ascii_lowercase();
                if !allowed.contains(&owner_repo) {
                    return None;
                }
                let key = ReferenceKey::Github {
                    owner_repo,
                    number: pull.number,
                    item: GithubItemKind::Pull,
                };
                let role = match pull.role {
                    PullRole::Author => "authored",
                    PullRole::ReviewRequested => "review requested",
                };
                let snapshot = ReferenceSnapshot::from_provider(
                    &pull.title,
                    "open",
                    vec![format!("repo {}", key_repo(&key)), role.to_string()],
                    cached_at,
                );
                Some(ImportCandidate {
                    canonical_url: key.canonical_url(),
                    key,
                    source_kind: "github".to_string(),
                    title: pull.title.clone(),
                    snapshot_json: Some(snapshot.to_json()),
                    account_id: None,
                })
            })
            .collect(),
    )
}

fn key_repo(key: &ReferenceKey) -> String {
    match key {
        ReferenceKey::Github { owner_repo, .. } => owner_repo.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn repo(owner: &str, name: &str) -> GithubRepo {
        GithubRepo {
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn pull(owner: &str, name: &str, number: u64, role: PullRole) -> FoundPull {
        FoundPull {
            repo: repo(owner, name),
            number,
            title: format!("Pull {number}"),
            url: format!("https://github.com/{owner}/{name}/pull/{number}"),
            role,
        }
    }

    #[test]
    fn remote_urls_parse_to_owner_and_name() {
        let want = Some(repo("Umbler", "Bill"));
        for url in [
            "https://github.com/Umbler/Bill",
            "https://github.com/Umbler/Bill.git\n",
            "https://github.com/Umbler/Bill/",
            "git@github.com:Umbler/Bill.git",
            "git@github.com:Umbler/Bill",
            "ssh://git@github.com/Umbler/Bill",
            "ssh://git@github.com/Umbler/Bill.git",
        ] {
            assert_eq!(parse_github_remote(url), want, "{url}");
        }
        for url in [
            "https://gitlab.com/o/r.git",
            "git@bitbucket.org:o/r.git",
            "https://github.com/o",
            "https://github.com/o/r/extra",
            "https://notgithub.com/o/r",
            "C:\\local\\bare",
            "",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("relay-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn init_repo(path: &Path, origin: Option<&str>) {
        fs::create_dir_all(path).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "--quiet"]);
        if let Some(origin) = origin {
            git(&["remote", "add", "origin", origin]);
        }
    }

    #[test]
    fn scan_finds_github_checkouts_one_level_down() {
        let root = temp_dir("scan");
        init_repo(&root.join("bill"), Some("git@github.com:umbler/bill.git"));
        init_repo(
            &root.join("account"),
            Some("https://github.com/umbler/account"),
        );
        init_repo(
            &root.join("gitlab"),
            Some("https://gitlab.com/umbler/x.git"),
        );
        init_repo(&root.join("local"), None);
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::create_dir_all(root.join("account").join("nested")).unwrap();
        init_repo(
            &root.join("account").join("nested").join("deep"),
            Some("https://github.com/umbler/deep"),
        );

        init_repo(
            &root.join("a-bill-copy"),
            Some("https://github.com/Umbler/Bill.git"),
        );

        let found = scan_folder(&root).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|c| format!("{}/{}", c.repo.owner, c.repo.name))
            .collect();
        assert_eq!(names, ["umbler/account", "umbler/bill"]);
        assert!(
            found[1].path.ends_with("bill"),
            "one checkout per repo, the shorter path"
        );
        assert!(found.iter().all(|c| c.project_id.is_none()));

        init_repo(&root, Some("https://github.com/umbler/root"));
        let with_root = scan_folder(&root).unwrap();
        assert_eq!(with_root.len(), 3, "the root itself counts");
        fs::remove_dir_all(&root).ok();
    }

    /// `RELAY_SCAN_ROOT=C:\umbler cargo test -p relay-core real_scan -- --ignored --nocapture`
    #[test]
    #[ignore = "reads a real folder"]
    fn real_scan() {
        let root = std::env::var("RELAY_SCAN_ROOT").expect("RELAY_SCAN_ROOT");
        for c in scan_folder(Path::new(&root)).unwrap() {
            println!("{} {}/{}", c.path, c.repo.owner, c.repo.name);
        }
    }

    #[test]
    fn scan_of_several_folders_merges_them_and_keeps_one_checkout_per_repo() {
        let work = temp_dir("scan-work");
        let side = temp_dir("scan-side");
        init_repo(&work.join("bill"), Some("https://github.com/umbler/bill"));
        init_repo(&side.join("relay"), Some("https://github.com/ufleck/relay"));
        init_repo(
            &side.join("bill-worktree-copy"),
            Some("https://github.com/umbler/bill"),
        );

        let found = scan_folders(&[&work, &side]).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|c| format!("{}/{}", c.repo.owner, c.repo.name))
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"umbler/bill".to_string()));
        assert!(names.contains(&"ufleck/relay".to_string()));
        assert!(scan_folders(&[work.as_path(), Path::new("Z:\relay-does-not-exist")]).is_err());
        fs::remove_dir_all(&work).ok();
        fs::remove_dir_all(&side).ok();
    }

    #[test]
    fn scan_of_a_missing_folder_is_an_error() {
        assert!(scan_folder(Path::new("Z:\\relay-does-not-exist")).is_err());
    }

    #[test]
    fn import_candidates_dedupe_roles_and_keep_the_exact_review_title() {
        let registered = vec!["umbler/bill".to_string(), "UMBLER/BILL".to_string()];
        let found = vec![
            pull("umbler", "bill", 9, PullRole::ReviewRequested),
            pull("umbler", "bill", 9, PullRole::Author),
            pull("umbler", "other", 10, PullRole::Author),
        ];
        let candidates = import_candidates(&found, &registered, "2026-11-21T00:00:00.000Z");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].title, "Pull 9");
        assert_eq!(candidates[0].source_kind, "github");
        assert_eq!(candidates[0].key.identity(), "github:umbler/bill#9");
        assert_eq!(
            candidates[0].canonical_url,
            "https://github.com/umbler/bill/pull/9"
        );
    }
}
