//! GitHub work status, fetched on the repository host independently of checkout operations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{command, worktrees};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkStatus {
    Open,
    Draft,
    Merged,
    Closed,
    Passed,
    Failed,
    Running,
}

impl WorkStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Running => "running",
        }
    }

    pub fn picker_label(self) -> &'static str {
        match self {
            Self::Passed => "✓",
            Self::Failed => "✕",
            Self::Running => "◌",
            other => other.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestStatus {
    pub number: u64,
    pub status: WorkStatus,
}

/// Keyed by the host's checkout path. An error means status is unavailable, not that no PR exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeStatuses {
    pub checkouts: BTreeMap<String, PullRequestStatus>,
    pub unavailable: bool,
    pub error: Option<String>,
    pub retryable: bool,
}

/// Each server's status worker retains at most eight repositories, including failed lookups.
#[derive(Default)]
pub(crate) struct StatusCache {
    entries: Vec<CacheEntry>,
}

struct CacheEntry {
    path: PathBuf,
    time: Instant,
    local: Vec<u8>,
    value: WorktreeStatuses,
}

impl StatusCache {
    pub fn get(&mut self, cwd: &Path, refresh: bool) -> WorktreeStatuses {
        self.lookup(cwd, refresh)
            .unwrap_or_else(|error| WorktreeStatuses {
                unavailable: true,
                retryable: transient_error(&error),
                error: Some(error),
                ..Default::default()
            })
    }

    fn lookup(&mut self, cwd: &Path, refresh: bool) -> Result<WorktreeStatuses, String> {
        self.lookup_with(cwd, refresh, fetch)
    }

    fn lookup_with(
        &mut self,
        cwd: &Path,
        refresh: bool,
        fetch: impl FnOnce(
            &Path,
            &[worktrees::WorktreeInfo],
            &BTreeMap<&str, &str>,
        ) -> Result<WorktreeStatuses, String>,
    ) -> Result<WorktreeStatuses, String> {
        let trees = worktrees::list(cwd)?;
        let key = trees
            .first()
            .map(|tree| PathBuf::from(&tree.path))
            .unwrap_or_else(|| cwd.to_path_buf());
        let refs = command::checked(
            cwd,
            &[
                "for-each-ref".into(),
                "--format=%(refname:strip=2)%00%(objectname)".into(),
                "refs/heads/".into(),
            ],
            command::WORKTREE_LIST_TIMEOUT,
        )?;
        let mut local = serde_json::to_vec(&trees).map_err(|e| e.to_string())?;
        local.extend_from_slice(&refs);
        let previous = self
            .entries
            .iter()
            .find(|entry| entry.path == key && entry.local == local);
        if let Some(entry) = previous {
            let ttl = if entry.value.unavailable && entry.value.retryable {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(60)
            };
            // Manual refreshes must fetch even immediately after a completed lookup.
            if !refresh && entry.time.elapsed() < ttl {
                return Ok(entry.value.clone());
            }
        }
        let refs = String::from_utf8_lossy(&refs);
        let heads = refs
            .lines()
            .filter_map(|line| line.split_once('\0'))
            .collect();
        let value = fetch(cwd, &trees, &heads).unwrap_or_else(|error| {
            let mut value = previous
                .map(|entry| entry.value.clone())
                .unwrap_or_default();
            value.unavailable = true;
            value.retryable = transient_error(&error);
            value.error = Some(error);
            value
        });
        self.entries.retain(|entry| entry.path != key);
        self.entries.insert(
            0,
            CacheEntry {
                path: key,
                time: Instant::now(),
                local,
                value: value.clone(),
            },
        );
        self.entries.truncate(8);
        Ok(value)
    }
}

/// Retry transport failures and busy workers, but do not repeatedly retry missing tools,
/// authentication, permission errors, or an absent repository.
pub(crate) fn transient_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "connection",
        "network",
        "temporary",
        "temporarily",
        "pending",
        "resolve host",
        "tls",
        "http 500",
        "http 429",
        "unexpected eof",
        "http 502",
        "http 503",
        "http 504",
        "rate limit",
    ]
    .iter()
    .any(|word| error.contains(word))
}

fn query(branches: &[&str], heads: &BTreeMap<&str, &str>) -> String {
    let mut query =
        String::from("query($owner:String!,$name:String!){repository(owner:$owner,name:$name){");
    for (index, branch) in branches.iter().enumerate() {
        // JSON string escaping is also GraphQL string escaping; branch names are never query code.
        let head = heads.get(branch).copied();
        let branch = serde_json::to_string(branch).expect("serialize branch");
        query.push_str(&format!(
            "b{index}:pullRequests(headRefName:{branch},first:10,orderBy:{{field:CREATED_AT,direction:DESC}}){{nodes{{number state isDraft isCrossRepository headRefOid}}}}"
        ));
        if let Some(head) = head {
            let head = serde_json::to_string(head).expect("serialize commit");
            query.push_str(&format!(
                "c{index}:object(oid:{head}){{... on Commit{{statusCheckRollup{{state}}}}}}"
            ));
        }
    }
    query.push_str("}}");
    query
}

fn fetch(
    cwd: &Path,
    trees: &[worktrees::WorktreeInfo],
    heads: &BTreeMap<&str, &str>,
) -> Result<WorktreeStatuses, String> {
    if !crate::platform::command::program_exists("gh") {
        return Err("Install gh on the session host to read PR status".into());
    }
    let branches: Vec<&str> = trees
        .iter()
        .filter_map(|tree| tree.branch.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if branches.is_empty() {
        return Ok(WorktreeStatuses::default());
    }
    let mut result = WorktreeStatuses::default();
    // Bound both query size and API cost for repositories with many checkouts.
    for (chunk_index, chunk) in branches.chunks(40).enumerate() {
        if chunk_index >= 8 {
            result.unavailable = true;
            break;
        }
        let output = crate::platform::command::run_bounded_argv_command_with_env(
            "gh",
            &[
                "api".to_string(),
                "graphql".into(),
                "-F".into(),
                "owner={owner}".into(),
                "-F".into(),
                "name={repo}".into(),
                "-f".into(),
                format!("query={}", query(chunk, heads)),
            ],
            &[("GH_PROMPT_DISABLED".into(), "1".into())],
            Some(cwd),
            Duration::from_secs(10),
            1024 * 1024,
        )
        .map_err(|error| error.to_string())?;
        if output.timed_out {
            return Err("GitHub status timed out".into());
        }
        if output.status != Some(0) {
            let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if error.is_empty() {
                "GitHub status unavailable".into()
            } else {
                error
            });
        }
        let data: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        let repository = &data["data"]["repository"];
        if !repository.is_object() || data.get("errors").is_some() {
            return Err(format!("GitHub status unavailable: {}", data["errors"]));
        }
        for (index, branch) in chunk.iter().enumerate() {
            let nodes = repository[format!("b{index}")]["nodes"]
                .as_array()
                .ok_or("invalid GitHub status")?;
            let Some(status) = find_status(
                nodes,
                heads.get(branch).copied(),
                repository[format!("c{index}")]["statusCheckRollup"]["state"].as_str(),
            ) else {
                continue;
            };
            for tree in trees
                .iter()
                .filter(|tree| tree.branch.as_deref() == Some(branch))
            {
                result.checkouts.insert(tree.path.clone(), status.clone());
            }
        }
    }
    Ok(result)
}

fn find_status(
    nodes: &[serde_json::Value],
    local_head: Option<&str>,
    checks: Option<&str>,
) -> Option<PullRequestStatus> {
    nodes.iter().find_map(|pr| {
        // Fork branches and terminal PRs must identify the current work by commit.
        if pr["isCrossRepository"] != false
            && (local_head.is_none() || local_head != pr["headRefOid"].as_str())
        {
            return None;
        }
        parse_status(pr, local_head, checks)
    })
}

fn parse_status(
    pr: &serde_json::Value,
    local_head: Option<&str>,
    checks: Option<&str>,
) -> Option<PullRequestStatus> {
    if matches!(pr["state"].as_str(), Some("MERGED" | "CLOSED"))
        && (local_head.is_none() || local_head != pr["headRefOid"].as_str())
    {
        return None;
    }
    let status = match pr["state"].as_str()? {
        "MERGED" => WorkStatus::Merged,
        "CLOSED" => WorkStatus::Closed,
        "OPEN" if pr["isDraft"] == true => WorkStatus::Draft,
        "OPEN" => {
            if local_head.is_none() || local_head != pr["headRefOid"].as_str() {
                WorkStatus::Open
            } else {
                match checks {
                    Some("SUCCESS") => WorkStatus::Passed,
                    Some("FAILURE" | "ERROR") => WorkStatus::Failed,
                    Some("PENDING" | "EXPECTED") => WorkStatus::Running,
                    _ => WorkStatus::Open,
                }
            }
        }
        _ => return None,
    };
    Some(PullRequestStatus {
        number: pr["number"].as_u64()?,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_status(pr: &serde_json::Value, head: Option<&str>) -> Option<PullRequestStatus> {
        super::parse_status(
            pr,
            head,
            pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str(),
        )
    }

    fn find_status(nodes: &[serde_json::Value], head: Option<&str>) -> Option<PullRequestStatus> {
        let checks = nodes
            .iter()
            .find(|pr| pr["headRefOid"].as_str() == head)
            .and_then(|pr| {
                pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str()
            });
        super::find_status(nodes, head, checks)
    }

    fn pr(state: &str, draft: bool, ci: Option<&str>) -> serde_json::Value {
        serde_json::json!({"number":114,"state":state,"isDraft":draft,"headRefOid":"abc","isCrossRepository":false,
            "commits":{"nodes":[{"commit":{"statusCheckRollup":ci.map(|state| serde_json::json!({"state":state}))}}]}})
    }

    #[test]
    fn lifecycle_precedes_ci_and_unknown_checks_stay_open() {
        for (state, draft, ci, expected) in [
            ("MERGED", false, Some("FAILURE"), WorkStatus::Merged),
            ("CLOSED", false, Some("SUCCESS"), WorkStatus::Closed),
            ("OPEN", true, Some("SUCCESS"), WorkStatus::Draft),
            ("OPEN", false, Some("SUCCESS"), WorkStatus::Passed),
            ("OPEN", false, Some("ERROR"), WorkStatus::Failed),
            ("OPEN", false, Some("PENDING"), WorkStatus::Running),
            ("OPEN", false, None, WorkStatus::Open),
        ] {
            assert_eq!(
                parse_status(&pr(state, draft, ci), Some("abc"))
                    .unwrap()
                    .status,
                expected
            );
        }
    }

    #[test]
    fn checks_for_an_older_commit_are_not_applied_to_local_work() {
        let value = pr("OPEN", false, Some("SUCCESS"));
        assert_eq!(
            parse_status(&value, Some("new")).unwrap().status,
            WorkStatus::Open
        );
        assert_eq!(parse_status(&value, None).unwrap().status, WorkStatus::Open);
    }

    #[test]
    fn branch_names_are_graphql_strings() {
        assert!(query(&["fix/a\"b"], &BTreeMap::new()).contains("headRefName:\"fix/a\\\"b\""));
    }

    #[test]
    fn reused_branches_drop_terminal_prs_but_keep_open_and_draft_associations() {
        for state in ["MERGED", "CLOSED"] {
            let value = pr(state, false, Some("SUCCESS"));
            assert!(parse_status(&value, Some("new")).is_none());
            assert!(parse_status(&value, None).is_none());
            assert!(parse_status(&value, Some("abc")).is_some());
        }
        for (draft, expected) in [(false, WorkStatus::Open), (true, WorkStatus::Draft)] {
            assert_eq!(
                parse_status(&pr("OPEN", draft, Some("SUCCESS")), Some("new"))
                    .unwrap()
                    .status,
                expected
            );
        }
    }

    #[test]
    fn an_unrelated_terminal_pr_does_not_hide_a_matching_candidate() {
        for state in ["MERGED", "CLOSED"] {
            let terminal = pr(state, false, None);
            let mut open = pr("OPEN", false, Some("SUCCESS"));
            open["number"] = 113.into();
            open["headRefOid"] = "new".into();
            let nodes = [terminal, open];
            let status = find_status(&nodes, Some("new")).unwrap();
            assert_eq!(status.number, 113);
            assert_eq!(status.status, WorkStatus::Passed);
        }
        let mut fork = pr("OPEN", false, Some("SUCCESS"));
        fork["isCrossRepository"] = true.into();
        assert!(find_status(&[fork.clone()], Some("new")).is_none());
        assert_eq!(
            find_status(&[fork], Some("abc")).unwrap().status,
            WorkStatus::Passed
        );
    }
    #[test]
    fn forced_status_refresh_fetches_again_immediately_after_completion() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        command::checked(
            repo.path(),
            &["init".into(), "-q".into()],
            command::WORKTREE_LIST_TIMEOUT,
        )
        .unwrap();
        for failed in [false, true] {
            let mut cache = StatusCache::default();
            let calls = std::cell::Cell::new(0);
            cache
                .lookup_with(repo.path(), false, |_, _, _| {
                    calls.set(calls.get() + 1);
                    if failed {
                        Err("unexpected EOF".into())
                    } else {
                        Ok(WorktreeStatuses::default())
                    }
                })
                .unwrap();
            // Keep the entry within the former 500 ms window throughout bounded Git reads.
            cache.entries[0].time = Instant::now() + Duration::from_secs(60);
            let fresh = WorktreeStatuses {
                checkouts: [(
                    "checkout".into(),
                    PullRequestStatus {
                        number: 42,
                        status: WorkStatus::Passed,
                    },
                )]
                .into(),
                ..Default::default()
            };
            assert_eq!(
                cache
                    .lookup_with(repo.path(), true, |_, _, _| {
                        calls.set(calls.get() + 1);
                        Ok(fresh.clone())
                    })
                    .unwrap(),
                fresh
            );
            assert_eq!(calls.get(), 2, "forced refresh must execute the fetch");
            assert_eq!(
                cache
                    .lookup_with(repo.path(), false, |_, _, _| {
                        panic!("automatic refresh must reuse fresh results")
                    })
                    .unwrap(),
                fresh
            );
        }
    }

    #[test]
    fn failed_status_refresh_keeps_good_data_and_retries_without_waiting_a_minute() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        command::checked(
            repo.path(),
            &["init".into(), "-q".into()],
            command::WORKTREE_LIST_TIMEOUT,
        )
        .unwrap();
        let mut cache = StatusCache::default();
        let good = WorktreeStatuses {
            checkouts: [(
                "checkout".into(),
                PullRequestStatus {
                    number: 1,
                    status: WorkStatus::Passed,
                },
            )]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            cache
                .lookup_with(repo.path(), false, |_, _, _| Ok(good.clone()))
                .unwrap(),
            good
        );
        cache.entries[0].time -= Duration::from_secs(61);
        let failed = cache
            .lookup_with(repo.path(), false, |_, _, _| {
                Err("connection timed out".into())
            })
            .unwrap();
        assert_eq!(failed.checkouts, good.checkouts);
        assert!(failed.unavailable && failed.retryable);
        cache.entries[0].time -= Duration::from_secs(2);
        assert_eq!(
            cache
                .lookup_with(repo.path(), false, |_, _, _| Ok(good.clone()))
                .unwrap(),
            good
        );
        cache
            .lookup_with(repo.path(), false, |_, _, _| {
                panic!("an automatic refresh must share the completed lookup")
            })
            .unwrap();
    }

    #[test]
    fn query_fetches_checks_once_per_local_head_outside_pr_candidates() {
        let query = query(
            &["feat/a", "feat/b"],
            &[("feat/a", "abc"), ("feat/b", "def")].into(),
        );
        assert_eq!(query.matches("statusCheckRollup").count(), 2);
        assert!(!query.contains("commits(last:"));
        assert!(query.contains("c0:object(oid:\"abc\")"));
        assert!(query.contains("first:10"));
    }

    #[test]
    fn only_recoverable_read_errors_are_retried() {
        for error in [
            "git timed out",
            "too many worktree requests are pending",
            "connection reset",
            "gh: HTTP 503",
            "HTTP 500: Internal Server Error",
            "gh: HTTP 429",
            "unexpected EOF",
        ] {
            assert!(transient_error(error), "{error}");
        }
        for error in [
            "not a git repository",
            "gh auth login",
            "git was not found",
            "permission denied",
            "HTTP 401: Bad credentials",
            "HTTP 403: Resource not accessible by integration",
        ] {
            assert!(!transient_error(error), "{error}");
        }
    }
}
