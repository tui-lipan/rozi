//! A bounded walk of one process's descendants over a captured process table.
//!
//! Platform-independent so it is tested on every target; Windows is the backend that answers a
//! pane's agent processes from it, because Windows has no foreground process group.

use std::collections::{HashMap, HashSet, VecDeque};

/// The most processes one walk examines, root included.
pub const MAX_TREE_PROCESSES: usize = 128;

/// One row of a process table: what a cheap host-wide pass reports without opening the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableEntry {
    pub pid: u32,
    /// The PID the process reported as its parent when it started. That process may have exited
    /// since and the PID been reused, so this is a hint, not a link.
    pub parent_pid: u32,
    pub name: String,
}

/// A process table indexed by PID and by parent PID.
#[derive(Debug, Default)]
pub struct ProcessTable {
    entries: Vec<TableEntry>,
    by_pid: HashMap<u32, usize>,
    /// Indexes into `entries`, ordered by PID.
    children: HashMap<u32, Vec<usize>>,
}

/// The result of one walk: its members, and whether every descendant was examined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk<T> {
    pub members: Vec<TreeMember<T>>,
    /// `false` when the walk stopped at [`MAX_TREE_PROCESSES`] with descendants left unexamined,
    /// so what it holds is only a prefix of the tree.
    pub complete: bool,
}

/// One process reached by [`ProcessTable::descendants`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeMember<T> {
    pub pid: u32,
    /// The parent inside the walked tree; `None` for the root.
    pub parent_pid: Option<u32>,
    pub observed: T,
}

impl ProcessTable {
    pub fn new(entries: Vec<TableEntry>) -> Self {
        let mut by_pid = HashMap::with_capacity(entries.len());
        let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            by_pid.insert(entry.pid, index);
            if entry.parent_pid != entry.pid {
                children.entry(entry.parent_pid).or_default().push(index);
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by_key(|&index| entries[index].pid);
        }
        Self {
            entries,
            by_pid,
            children,
        }
    }

    pub fn entries(&self) -> &[TableEntry] {
        &self.entries
    }

    /// The entries outside `excluded` that `accept` takes, or `None` when there are more than
    /// `cap` of them. Only names are looked at, so nothing is opened to find out. Anything chosen
    /// from part of them could pass over the one that matters, so too many is no answer at all.
    pub fn candidates(
        &self,
        excluded: &HashSet<u32>,
        accept: impl Fn(&TableEntry) -> bool,
        cap: usize,
    ) -> Option<Vec<&TableEntry>> {
        let candidates: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| !excluded.contains(&entry.pid) && accept(entry))
            .take(cap + 1)
            .collect();
        (candidates.len() <= cap).then_some(candidates)
    }

    /// Each of `tops` with its descendants, every process once, or `None` when together they hold
    /// more than `cap` processes. Part of them could leave out the one that matters.
    pub fn trees_of<T>(
        &self,
        tops: &[u32],
        mut observe: impl FnMut(&TableEntry) -> (T, Option<u64>),
        cap: usize,
    ) -> Option<Vec<TreeMember<T>>> {
        let mut seen = HashSet::new();
        let mut members = Vec::new();
        for &top in tops {
            let Some(walk) = self.descendants(top, &mut observe) else {
                continue;
            };
            if !walk.complete {
                return None;
            }
            for member in walk.members {
                if seen.insert(member.pid) {
                    if members.len() == cap {
                        return None;
                    }
                    members.push(member);
                }
            }
        }
        Some(members)
    }

    /// Those of `pids` that no other of them has among its descendants, by PID: the roots of the
    /// trees they belong to. In a parent cycle the lowest PID stands for the rest.
    ///
    /// `started` gives a process's start time, as for [`Self::descendants`], so a parent link
    /// through a reused PID does not put one of `pids` under another.
    pub fn outermost(
        &self,
        pids: &[u32],
        mut started: impl FnMut(&TableEntry) -> Option<u64>,
    ) -> Vec<u32> {
        let mut sorted = pids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut taken: Vec<(u32, HashSet<u32>)> = Vec::new();
        for pid in sorted {
            if taken.iter().any(|(_, covered)| covered.contains(&pid)) {
                continue;
            }
            let covered: HashSet<u32> = self
                .descendants(pid, |entry| ((), started(entry)))
                .map(|walk| walk.members.iter().map(|member| member.pid).collect())
                .unwrap_or_else(|| HashSet::from([pid]));
            taken.retain(|(earlier, _)| !covered.contains(earlier));
            taken.push((pid, covered));
        }
        let mut outermost: Vec<u32> = taken.into_iter().map(|(pid, _)| pid).collect();
        outermost.sort_unstable();
        outermost
    }

    /// `root` and its descendants, parents before children and siblings by PID, or `None` when
    /// `root` is not in the table.
    ///
    /// `observe` is called once for each process the walk examines and returns what to keep for
    /// it along with its start time, in any unit that orders. A process that started before the
    /// process named as its parent cannot be that process's child: its parent exited and the PID
    /// went to a later process. Such a process is left out with its own descendants. A process
    /// is examined at most once, so a parent cycle ends the walk, and at most
    /// [`MAX_TREE_PROCESSES`] are examined, after which the walk says it is incomplete.
    pub fn descendants<T>(
        &self,
        root: u32,
        mut observe: impl FnMut(&TableEntry) -> (T, Option<u64>),
    ) -> Option<Walk<T>> {
        let root_entry = &self.entries[*self.by_pid.get(&root)?];
        let mut seen = HashSet::from([root]);
        let mut examined = 1;
        let (observed, started) = observe(root_entry);
        let mut members = vec![TreeMember {
            pid: root,
            parent_pid: None,
            observed,
        }];
        let mut queue = VecDeque::from([(root, started)]);
        while let Some((parent, parent_started)) = queue.pop_front() {
            for &index in self.children.get(&parent).into_iter().flatten() {
                if examined == MAX_TREE_PROCESSES {
                    return Some(Walk {
                        members,
                        complete: false,
                    });
                }
                let entry = &self.entries[index];
                if !seen.insert(entry.pid) {
                    continue;
                }
                examined += 1;
                let (observed, started) = observe(entry);
                if let (Some(parent_started), Some(started)) = (parent_started, started)
                    && started < parent_started
                {
                    continue;
                }
                members.push(TreeMember {
                    pid: entry.pid,
                    parent_pid: Some(parent),
                    observed,
                });
                queue.push_back((entry.pid, started));
            }
        }
        Some(Walk {
            members,
            complete: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(u32, u32)]) -> ProcessTable {
        ProcessTable::new(
            rows.iter()
                .map(|&(pid, parent_pid)| TableEntry {
                    pid,
                    parent_pid,
                    name: format!("p{pid}"),
                })
                .collect(),
        )
    }

    /// `(pid, parent)` pairs of the walk from `root`, every process started at time 0.
    fn walk(table: &ProcessTable, root: u32) -> Option<Vec<(u32, Option<u32>)>> {
        table.descendants(root, |_| ((), Some(0))).map(|walk| {
            assert!(walk.complete);
            walk.members
                .into_iter()
                .map(|member| (member.pid, member.parent_pid))
                .collect()
        })
    }

    #[test]
    fn walks_descendants_breadth_first_with_siblings_by_pid() {
        // 1 is unrelated; 10 is the root with children 30 and 20 (listed out of order), and 20
        // has a grandchild 40.
        let table = table(&[(1, 0), (10, 1), (30, 10), (40, 20), (20, 10), (50, 1)]);
        assert_eq!(
            walk(&table, 10),
            Some(vec![
                (10, None),
                (20, Some(10)),
                (30, Some(10)),
                (40, Some(20))
            ])
        );
    }

    #[test]
    fn a_missing_root_has_no_tree() {
        assert_eq!(walk(&table(&[(1, 0), (2, 1)]), 9), None);
    }

    #[test]
    fn parent_cycles_and_self_parents_end_the_walk() {
        // 10 -> 20 -> 30 -> 10 is a cycle, and 40 names itself as its parent.
        let table = table(&[(10, 30), (20, 10), (30, 20), (40, 40)]);
        assert_eq!(
            walk(&table, 10),
            Some(vec![(10, None), (20, Some(10)), (30, Some(20))])
        );
        assert_eq!(walk(&table, 40), Some(vec![(40, None)]));
    }

    #[test]
    fn examines_at_most_the_cap() {
        let rows: Vec<_> = (2..1000).map(|pid| (pid, 1)).chain([(1, 0)]).collect();
        let table = table(&rows);
        let mut examined = 0;
        let walk = table
            .descendants(1, |_| {
                examined += 1;
                ((), None)
            })
            .unwrap();
        assert_eq!(examined, MAX_TREE_PROCESSES);
        assert_eq!(walk.members.len(), MAX_TREE_PROCESSES);
        assert_eq!(walk.members.last().unwrap().pid, 128);
        assert!(!walk.complete, "a walk cut at the cap says so");
    }

    #[test]
    fn more_candidates_than_the_cap_are_none_at_all() {
        let rows: Vec<_> = (1..=66).map(|pid| (pid, 0)).collect();
        let table = table(&rows);
        let excluded = HashSet::from([1]);
        let picked = |cap| {
            table
                .candidates(&excluded, |entry| entry.pid != 2, cap)
                .map(|entries| entries.len())
        };
        // 64 accepted outside the excluded one: exactly the cap, then one over it.
        assert_eq!(picked(64), Some(64));
        assert_eq!(picked(63), None);
    }

    #[test]
    fn trees_too_large_together_are_none_at_all() {
        // 10 has children 11 and 12; 20 has 21.
        let table = table(&[(10, 1), (11, 10), (12, 10), (20, 1), (21, 20)]);
        let pids = |cap| {
            table
                .trees_of(&[10, 20, 11], |_| ((), None), cap)
                .map(|members| members.iter().map(|member| member.pid).collect::<Vec<_>>())
        };
        assert_eq!(pids(5), Some(vec![10, 11, 12, 20, 21]));
        assert_eq!(pids(4), None);
    }

    #[test]
    fn only_the_outermost_of_related_processes_is_kept() {
        // 20 descends from 10 through 15; 30 is unrelated, with a child 5 of lower PID; 40 and 41
        // form a parent cycle.
        let table = table(&[
            (10, 1),
            (15, 10),
            (20, 15),
            (30, 1),
            (5, 30),
            (40, 41),
            (41, 40),
        ]);
        let outermost = |pids: &[u32]| table.outermost(pids, |_| None);
        assert_eq!(outermost(&[20, 30, 10]), [10, 30]);
        assert_eq!(outermost(&[5, 30]), [30]);
        assert_eq!(outermost(&[41, 40]), [40]);
    }

    /// The Git Bash case the fallback exists for: an agent whose shim parent exited, and whose
    /// parent PID a later process now holds. Under that process is not where it is.
    #[test]
    fn a_link_through_a_reused_pid_keeps_both_outermost() {
        // 600 names 500 as its parent, but started before 500 did.
        let table = table(&[(500, 1), (600, 500)]);
        let started = HashMap::from([(500, 300), (600, 200)]);
        assert_eq!(
            table.outermost(&[500, 600], |entry| started.get(&entry.pid).copied()),
            [500, 600]
        );
        assert_eq!(table.outermost(&[500, 600], |_| None), [500]);
    }

    #[test]
    fn a_process_older_than_its_parent_belongs_to_a_reused_pid() {
        // 30 started before 20, so its real parent was an earlier process that held PID 20; its
        // child 40 goes with it. 50 started later and is 20's.
        let table = table(&[(10, 1), (20, 10), (30, 20), (40, 30), (50, 20)]);
        let started = HashMap::from([(10, 100), (20, 200), (30, 150), (40, 300), (50, 250)]);
        let pids: Vec<_> = table
            .descendants(10, |entry| ((), started.get(&entry.pid).copied()))
            .unwrap()
            .members
            .into_iter()
            .map(|member| member.pid)
            .collect();
        assert_eq!(pids, [10, 20, 50]);
    }
}
