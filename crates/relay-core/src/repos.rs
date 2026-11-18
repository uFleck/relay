//! Local clones on GitHub: scan a folder for them, turn them into projects, and decide
//! which open pull requests become tasks. Pure policy; the adapters only fetch and call.

use std::collections::HashSet;
use std::path::Path;

use crate::git::{self, GitError};
use crate::github::{FoundPull, PullRole};
use crate::model::{NewLink, NewProject, NewTask, StatusCategory, now};
use crate::store::{Store, StoreError};

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

/// A project and the GitHub repository its checkout's `origin` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedRepo {
    pub project_id: String,
    pub repo: GithubRepo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPullTask {
    pub project_id: String,
    pub title: String,
    pub external_id: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepoImport {
    pub imported: usize,
    pub skipped: usize,
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
    let mut dirs = vec![root.to_path_buf()];
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            dirs.push(path);
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

/// Windows paths differ by separator, trailing slash, verbatim prefix and case.
fn same_path(a: &str, b: &str) -> bool {
    fn normal(path: &str) -> String {
        let path = path.trim().trim_start_matches(r"\\?\").replace('\\', "/");
        let path = path.trim_end_matches('/');
        if cfg!(windows) {
            path.to_lowercase()
        } else {
            path.to_string()
        }
    }
    normal(a) == normal(b)
}

/// Fills `project_id` on every candidate whose path a project already uses.
pub fn annotate_projects(
    store: &Store,
    candidates: Vec<RepoCandidate>,
) -> Result<Vec<RepoCandidate>, StoreError> {
    let projects = store.list_projects()?;
    Ok(candidates
        .into_iter()
        .map(|mut candidate| {
            candidate.project_id = projects
                .iter()
                .find(|project| {
                    project
                        .repo_path
                        .as_deref()
                        .is_some_and(|path| same_path(path, &candidate.path))
                })
                .map(|project| project.id.clone());
            candidate
        })
        .collect())
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "repo".into()
    } else {
        slug.to_string()
    }
}

fn unique_slug(name: &str, taken: &HashSet<String>) -> String {
    let base = slugify(name);
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|slug| !taken.contains(slug))
        .expect("an unbounded range always yields a free slug")
}

/// One project per candidate that no project covers yet. Running it twice imports nothing
/// the second time.
pub fn import_repos(store: &Store, candidates: &[RepoCandidate]) -> Result<RepoImport, StoreError> {
    let mut count = RepoImport::default();
    let mut taken: HashSet<String> = store
        .list_projects()?
        .into_iter()
        .map(|project| project.slug)
        .collect();
    let tracked = tracked_repos(store)?;
    for candidate in annotate_projects(store, candidates.to_vec())? {
        if candidate.project_id.is_some()
            || tracked
                .iter()
                .any(|project| project.repo.same_as(&candidate.repo))
        {
            count.skipped += 1;
            continue;
        }
        let slug = unique_slug(&candidate.repo.name, &taken);
        store.create_project_with_default_workflow(NewProject::new(
            &candidate.repo.name,
            &slug,
            Some(&candidate.path),
        ))?;
        taken.insert(slug);
        count.imported += 1;
    }
    Ok(count)
}

/// Projects whose `repo_path` has a GitHub `origin`.
pub fn tracked_repos(store: &Store) -> Result<Vec<TrackedRepo>, StoreError> {
    Ok(store
        .list_projects()?
        .into_iter()
        .filter_map(|project| {
            let repo = origin_repo(Path::new(project.repo_path.as_deref()?))?;
            Some(TrackedRepo {
                project_id: project.id,
                repo,
            })
        })
        .collect())
}

/// Which found pull requests become tasks. Pull requests in repositories no project tracks
/// are ignored, as are the ones whose URL is already linked. One that is both yours and a
/// review request counts as yours.
pub fn plan_pull_tasks(
    found: &[FoundPull],
    tracked: &[TrackedRepo],
    linked_urls: &HashSet<String>,
) -> Vec<PlannedPullTask> {
    let linked: HashSet<String> = linked_urls.iter().map(|url| url.to_lowercase()).collect();
    let mut seen = HashSet::new();
    let mut ordered: Vec<&FoundPull> = found.iter().collect();
    ordered.sort_by_key(|pull| pull.role != PullRole::Author);
    ordered
        .into_iter()
        .filter_map(|pull| {
            let url = pull.url.to_lowercase();
            if linked.contains(&url) || !seen.insert(url) {
                return None;
            }
            let project = tracked.iter().find(|t| t.repo.same_as(&pull.repo))?;
            let title = match pull.role {
                PullRole::Author => pull.title.clone(),
                PullRole::ReviewRequested => format!("Review: {}", pull.title),
            };
            Some(PlannedPullTask {
                project_id: project.project_id.clone(),
                title,
                external_id: format!("{}/{}#{}", pull.repo.owner, pull.repo.name, pull.number),
                url: pull.url.clone(),
            })
        })
        .collect()
}

/// Plans and creates the tasks for `found`. Returns how many were created.
pub fn discover_pull_tasks(store: &Store, found: &[FoundPull]) -> Result<usize, StoreError> {
    let tracked = tracked_repos(store)?;
    let linked: HashSet<String> = store
        .list_links_by_kind("pr")?
        .into_iter()
        .map(|link| link.url)
        .collect();
    let mut created = 0;
    for plan in plan_pull_tasks(found, &tracked, &linked) {
        if store
            .find_link("pr", Some(&plan.external_id), &plan.url)?
            .is_some()
        {
            continue;
        }
        let todo = store
            .list_statuses(&plan.project_id)?
            .into_iter()
            .find(|status| status.category == StatusCategory::Todo)
            .map(|status| status.id);
        let mut new = NewTask::new(&plan.project_id, &plan.title);
        new.status_id = todo;
        new.source = Some("github".into());
        let task = store.create_task(new)?;
        store.add_link(NewLink {
            task_id: task.id,
            claim_id: None,
            kind: "pr".into(),
            external_id: Some(plan.external_id),
            url: plan.url,
            title: Some(plan.title),
            state: Some("open".into()),
            metadata_json: None,
            last_synced_at: Some(now()),
        })?;
        created += 1;
    }
    Ok(created)
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
    fn scan_of_a_missing_folder_is_an_error() {
        assert!(scan_folder(Path::new("Z:\\relay-does-not-exist")).is_err());
    }

    #[test]
    fn import_creates_one_project_per_new_repo_and_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_project_with_default_workflow(NewProject::new(
                "Mine",
                "bill",
                Some("C:\\Umbler\\bill\\"),
            ))
            .unwrap();
        let candidate = |name: &str, path: &str| RepoCandidate {
            path: path.into(),
            repo: repo("umbler", name),
            project_id: None,
        };
        let candidates = vec![
            candidate("bill", "c:/umbler/bill"),
            candidate("Account", "C:\\umbler\\Account"),
            candidate("account", "C:\\other\\account"),
            candidate("My App!", "C:\\umbler\\my-app"),
        ];

        let first = import_repos(&store, &candidates).unwrap();
        assert_eq!((first.imported, first.skipped), (3, 1));
        let again = import_repos(&store, &candidates).unwrap();
        assert_eq!((again.imported, again.skipped), (0, 4));

        let mut slugs: Vec<_> = store
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        slugs.sort();
        assert_eq!(slugs, ["account", "account-2", "bill", "my-app"]);
    }

    #[test]
    fn annotate_marks_candidates_that_already_have_a_project() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project_with_default_workflow(NewProject::new(
                "Bill",
                "bill",
                Some("C:\\umbler\\bill"),
            ))
            .unwrap()
            .project;
        let candidates = vec![
            RepoCandidate {
                path: "C:\\umbler\\bill".into(),
                repo: repo("umbler", "bill"),
                project_id: None,
            },
            RepoCandidate {
                path: "C:\\umbler\\app".into(),
                repo: repo("umbler", "app"),
                project_id: None,
            },
        ];
        let out = annotate_projects(&store, candidates).unwrap();
        assert_eq!(out[0].project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(out[1].project_id, None);
    }

    #[test]
    fn plan_covers_mapped_repos_only_and_skips_linked() {
        let tracked = vec![TrackedRepo {
            project_id: "p1".into(),
            repo: repo("Umbler", "Bill"),
        }];
        let found = vec![
            pull("umbler", "bill", 1, PullRole::Author),
            pull("umbler", "bill", 2, PullRole::ReviewRequested),
            pull("umbler", "bill", 3, PullRole::Author),
            pull("umbler", "other", 4, PullRole::Author),
        ];
        let linked: HashSet<String> = ["https://github.com/umbler/bill/pull/3".to_string()].into();

        let plan = plan_pull_tasks(&found, &tracked, &linked);
        let got: Vec<_> = plan
            .iter()
            .map(|p| {
                (
                    p.title.as_str(),
                    p.external_id.as_str(),
                    p.project_id.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("Pull 1", "umbler/bill#1", "p1"),
                ("Review: Pull 2", "umbler/bill#2", "p1"),
            ]
        );
    }

    #[test]
    fn a_pull_that_is_yours_and_a_review_request_counts_as_yours() {
        let tracked = vec![TrackedRepo {
            project_id: "p1".into(),
            repo: repo("o", "r"),
        }];
        let found = vec![
            pull("o", "r", 9, PullRole::ReviewRequested),
            pull("o", "r", 9, PullRole::Author),
        ];
        let plan = plan_pull_tasks(&found, &tracked, &HashSet::new());
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].title, "Pull 9");
    }

    #[test]
    fn discovery_creates_todo_tasks_with_pr_links_once() {
        let store = Store::open_in_memory().unwrap();
        let root = temp_dir("discover");
        init_repo(&root, Some("https://github.com/umbler/bill.git"));
        let created = store
            .create_project_with_default_workflow(NewProject::new(
                "Bill",
                "bill",
                Some(&root.to_string_lossy()),
            ))
            .unwrap();
        let found = vec![
            pull("umbler", "bill", 5, PullRole::ReviewRequested),
            pull("umbler", "elsewhere", 6, PullRole::Author),
        ];

        assert_eq!(discover_pull_tasks(&store, &found).unwrap(), 1);
        assert_eq!(discover_pull_tasks(&store, &found).unwrap(), 0);

        let links = store.list_links_by_kind("pr").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].external_id.as_deref(), Some("umbler/bill#5"));
        let task = store.get_task(&links[0].task_id).unwrap();
        assert_eq!(task.title, "Review: Pull 5");
        assert_eq!(task.source, "github");
        let todo = created
            .statuses
            .iter()
            .find(|s| s.category == StatusCategory::Todo)
            .unwrap();
        assert_eq!(task.status_id, todo.id);
        fs::remove_dir_all(&root).ok();
    }
}
