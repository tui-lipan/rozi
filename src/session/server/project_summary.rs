use std::collections::BTreeMap;

use super::*;

/// What a checkout holds: a pane, or one published row of a pane whose own directory is elsewhere.
enum Place {
    Pane(PaneId),
    Row(protocol::ProjectRowRef),
}

impl SessionServer {
    /// The checkouts this session works in, for a client that will never attach. See
    /// [`protocol::ProjectSummary`] for what this deliberately leaves out.
    ///
    /// A pane belongs to the checkout its working directory is in. A published row that names a
    /// directory of its own belongs to that checkout instead, when it differs from its pane's. Panes
    /// are visited in id order and checkouts listed by root, so consecutive snapshots of an
    /// unchanged session are equal.
    pub(super) fn project_summaries(&self) -> Vec<protocol::ProjectSummary> {
        let mut ids: Vec<_> = self.panes.keys().copied().collect();
        ids.sort_unstable();
        let mut checkouts: BTreeMap<String, protocol::ProjectSummary> = BTreeMap::new();
        let mut place = |root: &str, repository: Option<&String>, branch: Option<&String>, at| {
            let summary =
                checkouts
                    .entry(root.to_string())
                    .or_insert_with(|| protocol::ProjectSummary {
                        session: self.session_name.clone(),
                        root: root.to_string(),
                        repository: repository.cloned(),
                        branch: branch.cloned(),
                        panes: Vec::new(),
                        rows: Vec::new(),
                    });
            match at {
                Place::Pane(id) => summary.panes.push(id),
                Place::Row(row) => summary.rows.push(row),
            }
        };
        for id in ids {
            let pane = &self.panes[&id];
            if pane.exited.is_some() {
                continue;
            }
            let runtime = &pane.runtime;
            if let Some(root) = runtime.project_root.as_deref() {
                place(
                    root,
                    runtime.repository.as_ref(),
                    runtime.git_branch.as_ref(),
                    Place::Pane(id),
                );
            }
            for row in &runtime.rows {
                let Some(project) = row.project.as_ref().filter(|_| row.cwd.is_some()) else {
                    continue;
                };
                if runtime.project_root.as_deref() == Some(project.root.as_str()) {
                    continue;
                }
                place(
                    &project.root,
                    project.repository.as_ref(),
                    project.branch.as_ref(),
                    Place::Row(protocol::ProjectRowRef {
                        pane: id,
                        row: row.key(),
                    }),
                );
            }
        }
        checkouts.into_values().collect()
    }
}
