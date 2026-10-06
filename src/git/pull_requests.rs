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
            .unwrap_or_else(|_| WorktreeStatuses {
                unavailable: true,
                ..Default::default()
            })
    }

    fn lookup(&mut self, cwd: &Path, refresh: bool) -> Result<WorktreeStatuses, String> {
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
        if !refresh
            && let Some(entry) = self.entries.iter().find(|entry| {
                entry.path == key
                    && entry.local == local
                    && entry.time.elapsed() < Duration::from_secs(60)
            })
        {
            return Ok(entry.value.clone());
        }
        let refs = String::from_utf8_lossy(&refs);
        let heads = refs
            .lines()
            .filter_map(|line| line.split_once('\0'))
            .collect();
        let value = fetch(cwd, &trees, &heads).unwrap_or_else(|_| WorktreeStatuses {
            unavailable: true,
            ..Default::default()
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

fn query(branches: &[&str]) -> String {
    let mut query =
        String::from("query($owner:String!,$name:String!){repository(owner:$owner,name:$name){");
    for (index, branch) in branches.iter().enumerate() {
        // JSON string escaping is also GraphQL string escaping; branch names are never query code.
        let branch = serde_json::to_string(branch).expect("serialize branch");
        query.push_str(&format!(
            "b{index}:pullRequests(headRefName:{branch},first:10,orderBy:{{field:CREATED_AT,direction:DESC}}){{nodes{{number state isDraft isCrossRepository headRefOid commits(last:1){{nodes{{commit{{statusCheckRollup{{state}}}}}}}}}}}}"
        ));
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
        return Err("gh unavailable".into());
    }
    let branches: Vec<&str> = trees
        .iter()
        .filter_map(|tree| tree.branch.as_deref())
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
                format!("query={}", query(chunk)),
            ],
            &[("GH_PROMPT_DISABLED".into(), "1".into())],
            Some(cwd),
            Duration::from_secs(10),
            1024 * 1024,
        )
        .map_err(|error| error.to_string())?;
        if output.timed_out || output.status != Some(0) {
            return Err("GitHub status unavailable".into());
        }
        let data: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        let repository = &data["data"]["repository"];
        if !repository.is_object() || data.get("errors").is_some() {
            return Err("GitHub status unavailable".into());
        }
        for (index, branch) in chunk.iter().enumerate() {
            let nodes = repository[format!("b{index}")]["nodes"]
                .as_array()
                .ok_or("invalid GitHub status")?;
            let Some(status) = find_status(nodes, heads.get(branch).copied()) else {
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

fn find_status(nodes: &[serde_json::Value], local_head: Option<&str>) -> Option<PullRequestStatus> {
    nodes.iter().find_map(|pr| {
        // Fork branches and terminal PRs must identify the current work by commit.
        if pr["isCrossRepository"] != false
            && (local_head.is_none() || local_head != pr["headRefOid"].as_str())
        {
            return None;
        }
        parse_status(pr, local_head)
    })
}

fn parse_status(pr: &serde_json::Value, local_head: Option<&str>) -> Option<PullRequestStatus> {
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
                match pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str() {
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
        assert!(query(&["fix/a\"b"]).contains("headRefName:\"fix/a\\\"b\""));
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
}
