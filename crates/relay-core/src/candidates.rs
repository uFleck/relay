//! Which tasks stage B of the Slack analysis may choose from. A task whose ClickUp or
//! GitHub link was pasted into the conversation ranks first, then tasks matching the
//! conversation's most frequent words, then a few active tasks. Policy only: the
//! adapter calls `pick` and passes the result to the classifier.

use std::collections::{HashMap, HashSet};

use crate::analysis::CandidateTask;
use crate::model::{StatusCategory, TaskFilter};
use crate::store::Store;

/// Most candidates stage B sees.
pub const CAP: usize = 20;
const KEYWORDS: usize = 8;
const PER_KEYWORD: usize = 10;
const ACTIVE: usize = 5;

/// A pasted link that can name a task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LinkRef {
    ClickUp {
        id: String,
    },
    /// `repo` is `owner/name`, lowercase. PRs and issues share one number space.
    GitHub {
        repo: String,
        number: u64,
    },
}

impl LinkRef {
    /// The `external_id` the importers store, lowercase.
    fn external_id(&self) -> String {
        match self {
            LinkRef::ClickUp { id } => id.to_lowercase(),
            LinkRef::GitHub { repo, number } => format!("{repo}#{number}"),
        }
    }

    /// The URLs a link row may hold for this reference, lowercase.
    fn urls(&self) -> Vec<String> {
        match self {
            LinkRef::ClickUp { id } => {
                vec![format!("https://app.clickup.com/t/{}", id.to_lowercase())]
            }
            LinkRef::GitHub { repo, number } => ["pull", "issues"]
                .iter()
                .map(|kind| format!("https://github.com/{repo}/{kind}/{number}"))
                .collect(),
        }
    }
}

fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_')
}

/// The path-like run that starts `rest`, split into non-empty segments.
fn segments(rest: &str) -> Vec<&str> {
    let end = rest
        .find(|ch: char| !is_path_char(ch))
        .unwrap_or(rest.len());
    rest[..end]
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

/// ClickUp (`.../t/<id>` and `.../t/<team>/<id>`) and GitHub (`github.com/<owner>/<repo>/
/// (pull|issues)/<n>`) task links in `text`, in order of first appearance.
pub fn extract_link_refs(text: &str) -> Vec<LinkRef> {
    let lower = text.to_lowercase();
    let mut found: Vec<(usize, LinkRef)> = Vec::new();

    for (at, marker) in lower.match_indices("clickup.com/t/") {
        let rest = &text[at + marker.len()..];
        // The id is the last of at most two segments: `<id>` or `<team>/<id>`.
        let parts = segments(rest);
        if let Some(id) = parts.get(1).or(parts.first()) {
            found.push((
                at,
                LinkRef::ClickUp {
                    id: (*id).to_string(),
                },
            ));
        }
    }
    for (at, marker) in lower.match_indices("github.com/") {
        let parts = segments(&text[at + marker.len()..]);
        if let [owner, repo, kind, number, ..] = parts.as_slice()
            && matches!(*kind, "pull" | "issues")
            && let Ok(number) = number.parse::<u64>()
        {
            let repo = format!("{owner}/{repo}").to_lowercase();
            found.push((at, LinkRef::GitHub { repo, number }));
        }
    }

    found.sort_by_key(|(at, _)| *at);
    let mut seen = HashSet::new();
    found
        .into_iter()
        .map(|(_, link)| link)
        .filter(|link| seen.insert(link.clone()))
        .collect()
}

const STOP_EN: &[&str] = &[
    "that", "this", "with", "have", "from", "your", "about", "there", "would", "could", "should",
    "they", "what", "when", "will", "just", "then", "than", "them", "were", "been", "being",
    "into", "also", "some", "more", "like", "need", "does", "doing", "done", "here", "where",
    "which", "their", "these", "those", "going", "okay", "yeah", "thanks", "thank", "please",
    "think", "know", "want", "make", "sure", "only", "very", "much", "still", "other", "after",
    "before", "because", "https", "http",
];

const STOP_PT: &[&str] = &[
    "para",
    "como",
    "mais",
    "isso",
    "esse",
    "essa",
    "esses",
    "essas",
    "este",
    "esta",
    "isto",
    "aqui",
    "aquele",
    "aquela",
    "ainda",
    "pelo",
    "pela",
    "pelos",
    "pelas",
    "entre",
    "sobre",
    "quando",
    "onde",
    "porque",
    "porém",
    "porem",
    "então",
    "entao",
    "também",
    "tambem",
    "muito",
    "muita",
    "muitos",
    "você",
    "voce",
    "vocês",
    "voces",
    "nosso",
    "nossa",
    "nossos",
    "dele",
    "dela",
    "deles",
    "delas",
    "seja",
    "sendo",
    "foram",
    "fazer",
    "feito",
    "fazendo",
    "preciso",
    "precisa",
    "precisamos",
    "tenho",
    "temos",
    "tinha",
    "tinham",
    "vamos",
    "pode",
    "podemos",
    "podia",
    "posso",
    "depois",
    "antes",
    "agora",
    "coisa",
    "coisas",
    "algum",
    "alguma",
    "alguns",
    "algumas",
    "outro",
    "outra",
    "outros",
    "outras",
    "mesmo",
    "mesma",
    "estou",
    "está",
    "estão",
    "estamos",
    "estava",
    "estavam",
    "esta",
    "tudo",
    "todo",
    "toda",
    "todos",
    "todas",
    "gente",
    "obrigado",
    "obrigada",
    "valeu",
    "blz",
    "beleza",
    "certo",
    "claro",
    "queria",
    "quero",
    "quer",
    "vezes",
    "vez",
    "faz",
    "fiz",
    "bom",
    "dia",
    "tarde",
    "noite",
    "oi",
    "olá",
    "ola",
    "hoje",
    "amanhã",
    "amanha",
    "ontem",
    "que",
    "uma",
    "com",
    "nao",
    "não",
    "sim",
    "ver",
    "dar",
    "ser",
    "tem",
    "são",
    "sao",
    "vai",
    "vou",
    "aí",
    "dado",
    "dada",
    "pois",
    "cada",
    "qual",
    "quais",
    "quem",
    "assim",
    "ficou",
    "ficar",
    "fica",
    "link",
    "pra",
    "pro",
    "numa",
    "num",
    "nele",
    "nela",
    "disso",
    "disto",
    "nisso",
    "aquilo",
    "porra",
    "tipo",
    "acho",
    "falar",
];

fn is_stop(word: &str) -> bool {
    STOP_EN.contains(&word) || STOP_PT.contains(&word)
}

/// The `limit` most frequent non-stop words (4+ characters), ties broken by first
/// appearance. URLs are skipped: their hosts and slugs say nothing about the topic.
pub fn keywords(text: &str, limit: usize) -> Vec<String> {
    let mut count: HashMap<String, (usize, usize)> = HashMap::new();
    let mut order = 0;
    for chunk in text.split_whitespace() {
        if chunk.contains("://") || chunk.contains("www.") {
            continue;
        }
        for word in chunk.split(|ch: char| !ch.is_alphanumeric()) {
            let word = word.to_lowercase();
            if word.chars().count() < 4
                || is_stop(&word)
                || word.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            let entry = count.entry(word).or_insert_with(|| {
                order += 1;
                (0, order)
            });
            entry.0 += 1;
        }
    }
    let mut words: Vec<(String, (usize, usize))> = count.into_iter().collect();
    words.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    words
        .into_iter()
        .take(limit)
        .map(|(word, _)| word)
        .collect()
}

/// Tasks whose link matches a pasted one, in the order the links appear.
fn linked_tasks(store: &Store, transcript: &str) -> Vec<String> {
    let refs = extract_link_refs(transcript);
    if refs.is_empty() {
        return Vec::new();
    }
    let ids: Vec<String> = refs.iter().map(LinkRef::external_id).collect();
    let urls: Vec<String> = refs.iter().flat_map(LinkRef::urls).collect();
    let Ok(links) = store.find_links_matching(&ids, &urls) else {
        return Vec::new();
    };
    let position = |link: &crate::model::TaskLink| {
        let external = link.external_id.as_deref().map(str::to_lowercase);
        let url = link.url.trim().trim_end_matches('/').to_lowercase();
        refs.iter().position(|reference| {
            external.as_deref() == Some(reference.external_id().as_str())
                || reference.urls().contains(&url)
        })
    };
    let mut ranked: Vec<(usize, String)> = links
        .iter()
        .filter_map(|link| Some((position(link)?, link.task_id.clone())))
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, id)| id).collect()
}

/// Tasks matching any of the top keywords, those matching the most keywords first.
fn keyword_tasks(store: &Store, transcript: &str) -> Vec<String> {
    let mut score: HashMap<String, (usize, usize)> = HashMap::new();
    for word in keywords(transcript, KEYWORDS) {
        let Ok(found) = store.search_tasks(&word, PER_KEYWORD) else {
            continue;
        };
        for (rank, task) in found.into_iter().enumerate() {
            let entry = score.entry(task.id).or_insert((0, rank));
            entry.0 += 1;
            entry.1 = entry.1.min(rank);
        }
    }
    let mut ranked: Vec<(String, (usize, usize))> = score.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.0
            .cmp(&a.1.0)
            .then(a.1.1.cmp(&b.1.1))
            .then(a.0.cmp(&b.0))
    });
    ranked.into_iter().map(|(id, _)| id).collect()
}

/// The shortlist for one transcript: linked tasks, keyword matches, then active tasks;
/// deduplicated and capped at [`CAP`].
pub fn pick(store: &Store, transcript: &str) -> Vec<CandidateTask> {
    let mut ids = linked_tasks(store, transcript);
    ids.extend(keyword_tasks(store, transcript));
    if let Ok(active) = store.list_tasks(TaskFilter {
        category: Some(StatusCategory::Active),
        limit: Some(ACTIVE),
        ..TaskFilter::default()
    }) {
        ids.extend(active.into_iter().map(|task| task.id));
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Ok(detail) = store.get_task_detail(&id, None) else {
            continue;
        };
        out.push(CandidateTask {
            id,
            title: detail.task.title,
            status: detail.status.name,
        });
        if out.len() == CAP {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewLink, NewProject, NewTask};

    fn store_with_project() -> (Store, String) {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        (store, project.project.id)
    }

    fn task(store: &Store, project: &str, title: &str) -> String {
        store.create_task(NewTask::new(project, title)).unwrap().id
    }

    fn link(store: &Store, task_id: &str, kind: &str, external_id: &str, url: &str) {
        store
            .add_link(NewLink {
                task_id: task_id.into(),
                claim_id: None,
                kind: kind.into(),
                external_id: Some(external_id.into()),
                url: url.into(),
                title: None,
                state: None,
                metadata_json: None,
                last_synced_at: None,
            })
            .unwrap();
    }

    #[test]
    fn extracts_clickup_and_github_links_in_order() {
        let text = "see <https://app.clickup.com/t/17tw8yvca0h|card> and \
                    https://app.clickup.com/t/9012345/ABC-12, then \
                    https://github.com/Umbler/Bill/pull/42. Also github.com/umbler/bill/issues/7?x=1 \
                    and https://github.com/umbler/bill/tree/main and a repeat \
                    https://app.clickup.com/t/17tw8yvca0h";
        assert_eq!(
            extract_link_refs(text),
            vec![
                LinkRef::ClickUp {
                    id: "17tw8yvca0h".into()
                },
                LinkRef::ClickUp {
                    id: "ABC-12".into()
                },
                LinkRef::GitHub {
                    repo: "umbler/bill".into(),
                    number: 42
                },
                LinkRef::GitHub {
                    repo: "umbler/bill".into(),
                    number: 7
                },
            ]
        );
    }

    #[test]
    fn keywords_rank_by_frequency_and_skip_stop_words_and_urls() {
        let text = "Precisamos do relatorio de faturamento. https://faturamento.example/x \
                    O relatorio de faturamento esta quebrado, relatorio errado para o cliente";
        let words = keywords(text, 3);
        assert_eq!(words[0], "relatorio");
        assert_eq!(words[1], "faturamento");
        assert!(
            !words
                .iter()
                .any(|w| w == "para" || w == "precisamos" || w == "esta")
        );
        assert_eq!(keywords("ties alpha beta gamma", 2), vec!["ties", "alpha"]);
    }

    #[test]
    fn a_pasted_clickup_link_ranks_its_task_first() {
        let (store, project) = store_with_project();
        for n in 0..6 {
            task(&store, &project, &format!("deploy pipeline {n}"));
        }
        let target = task(&store, &project, "Unrelated title");
        link(
            &store,
            &target,
            "clickup",
            "17tw8yvca0h",
            "https://app.clickup.com/t/9000/17tw8yvca0h",
        );
        let transcript = "me: deploy pipeline broke again\nana: new reqs in https://app.clickup.com/t/17tw8yvca0h";
        let picked = pick(&store, transcript);
        assert_eq!(picked[0].id, target);
        assert!(picked.len() > 1);
        assert_eq!(picked.iter().filter(|c| c.id == target).count(), 1);
    }

    #[test]
    fn github_issue_and_pr_links_match_either_form_and_archived_tasks_are_skipped() {
        let (store, project) = store_with_project();
        let pr = task(&store, &project, "Fix thing");
        link(
            &store,
            &pr,
            "pr",
            "umbler/bill#42",
            "https://github.com/umbler/bill/pull/42",
        );
        let issue = task(&store, &project, "Issue thing");
        link(
            &store,
            &issue,
            "github",
            "umbler/bill#7",
            "https://github.com/umbler/bill/issues/7",
        );
        let gone = task(&store, &project, "Archived thing");
        link(
            &store,
            &gone,
            "clickup",
            "zzz999",
            "https://app.clickup.com/t/zzz999",
        );
        let rev = store.get_task(&gone).unwrap().rev;
        store
            .update_task(
                &gone,
                crate::model::TaskPatch {
                    archived: Some(true),
                    ..Default::default()
                },
                rev,
            )
            .unwrap();

        let text = "https://github.com/Umbler/bill/issues/42 https://github.com/umbler/bill/pull/7 \
                    https://app.clickup.com/t/zzz999";
        let ids: Vec<String> = pick(&store, text).into_iter().map(|c| c.id).collect();
        assert_eq!(&ids[..2], [pr.as_str(), issue.as_str()]);
        assert!(!ids.contains(&gone));
    }

    #[test]
    fn candidates_are_deduplicated_and_capped() {
        let (store, project) = store_with_project();
        for n in 0..15 {
            task(&store, &project, &format!("faturamento item {n}"));
            task(&store, &project, &format!("relatorio item {n}"));
        }
        let picked = pick(&store, "faturamento faturamento relatorio");
        assert_eq!(picked.len(), CAP);
        let unique: HashSet<_> = picked.iter().map(|c| &c.id).collect();
        assert_eq!(unique.len(), CAP);
    }

    #[test]
    fn keyword_matches_that_hit_more_words_rank_higher() {
        let (store, project) = store_with_project();
        let one = task(&store, &project, "faturamento mensal");
        let both = task(&store, &project, "faturamento relatorio");
        let picked = keyword_tasks(&store, "relatorio relatorio faturamento");
        assert_eq!(picked[0], both);
        assert!(picked.contains(&one));
    }
}
