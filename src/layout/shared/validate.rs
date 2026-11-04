//! Structural validation of a shared layout document.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedLayoutValidationError {
    UnsupportedVersion(u32),
    ZeroCanvasDimensions,
    InvalidPaneGeometry,
    DuplicateWorkspaceIndex(usize),
    InvalidWorkspaceIndex(usize),
    ReservedPaneId(PaneId),
    DuplicatePaneId(PaneId),
    TreeLeafNotInWorkspace {
        workspace: usize,
        pane_id: PaneId,
    },
    DuplicateTreeLeaf {
        workspace: usize,
        pane_id: PaneId,
    },
    InvalidSplitRatio {
        workspace: usize,
    },
    InvalidTreeSplitRatio {
        workspace: usize,
    },
    MissingFloatingRect(PaneId),
    InvalidPin(PaneId),
    InvalidFloatingRect {
        pane_id: PaneId,
        reason: &'static str,
    },
    InvalidScrollableWidth(PaneId),
    InvalidGeneration {
        pane_id: PaneId,
        generation: u64,
    },
}

impl std::fmt::Display for SharedLayoutValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported shared layout version {v}"),
            Self::ZeroCanvasDimensions => write!(f, "canvas dimensions must be positive"),
            Self::InvalidPaneGeometry => write!(
                f,
                "pane padding must not exceed 8 and the workbar gap must be -1, 0, or 1"
            ),
            Self::DuplicateWorkspaceIndex(i) => write!(f, "duplicate workspace index {i}"),
            Self::InvalidWorkspaceIndex(i) => write!(f, "workspace index {i} out of bounds"),
            Self::ReservedPaneId(id) => {
                write!(f, "reserved pane id {id} not allowed in shared layout")
            }
            Self::DuplicatePaneId(id) => write!(f, "duplicate pane id {id}"),
            Self::TreeLeafNotInWorkspace { workspace, pane_id } => {
                write!(
                    f,
                    "tree leaf {pane_id} not found in workspace {workspace} panes"
                )
            }
            Self::DuplicateTreeLeaf { workspace, pane_id } => {
                write!(f, "duplicate tree leaf {pane_id} in workspace {workspace}")
            }
            Self::InvalidSplitRatio { workspace } => {
                write!(f, "invalid split ratio in workspace {workspace}")
            }
            Self::InvalidTreeSplitRatio { workspace } => {
                write!(f, "invalid tree split ratio in workspace {workspace}")
            }
            Self::MissingFloatingRect(id) => write!(f, "floating pane {id} missing rect"),
            Self::InvalidPin(id) => {
                write!(f, "pinned pane {id} must float and not be fullscreen")
            }
            Self::InvalidFloatingRect { pane_id, reason } => {
                write!(f, "floating pane {pane_id} has invalid rect: {reason}")
            }
            Self::InvalidScrollableWidth(id) => {
                write!(
                    f,
                    "pane {id} has non-positive or non-finite scrollable width"
                )
            }
            Self::InvalidGeneration {
                pane_id,
                generation,
            } => {
                write!(f, "pane {pane_id} has invalid generation {generation}")
            }
        }
    }
}

impl std::error::Error for SharedLayoutValidationError {}

impl SharedLayout {
    /// Validate that the shared layout document satisfies all structural and security invariants:
    /// - Protocol/layout version matches [`SHARED_LAYOUT_VERSION`].
    /// - Positive canvas dimensions.
    /// - Unique workspace indexes within `0..WORKSPACE_COUNT` and pane identities across all workspaces.
    /// - Tree leaves belong to their workspace's pane list and do not repeat.
    /// - Split ratios in workspace lists and tree split nodes are finite and within `(0.0, 1.0)`.
    /// - Floating panes carry finite, positive fractional bounds; tiled panes carry no rect.
    /// - Scrollable column widths are finite and positive.
    /// - Pane generations are non-zero.
    pub fn validate(&self) -> std::result::Result<(), SharedLayoutValidationError> {
        if self.version != SHARED_LAYOUT_VERSION {
            return Err(SharedLayoutValidationError::UnsupportedVersion(
                self.version,
            ));
        }
        if self.canvas_cols == 0 || self.canvas_rows == 0 {
            return Err(SharedLayoutValidationError::ZeroCanvasDimensions);
        }

        let geometry = self.pane_geometry;
        if [
            geometry.padding.0,
            geometry.padding.1,
            geometry.padding.2,
            geometry.padding.3,
        ]
        .into_iter()
        .any(|side| side > 8)
            || !(-1..=1).contains(&geometry.top_gap)
        {
            return Err(SharedLayoutValidationError::InvalidPaneGeometry);
        }

        let mut seen_workspaces = std::collections::HashSet::new();
        let mut seen_panes = std::collections::HashSet::new();

        for ws in &self.workspaces {
            if ws.index >= crate::layout::model::WORKSPACE_COUNT {
                return Err(SharedLayoutValidationError::InvalidWorkspaceIndex(ws.index));
            }
            if !seen_workspaces.insert(ws.index) {
                return Err(SharedLayoutValidationError::DuplicateWorkspaceIndex(
                    ws.index,
                ));
            }

            let mut ws_pane_ids = std::collections::HashSet::new();
            for pane in &ws.panes {
                if pane.pane_id == crate::pane::id::POPUP_PANE_ID {
                    return Err(SharedLayoutValidationError::ReservedPaneId(pane.pane_id));
                }
                if !seen_panes.insert(pane.pane_id) {
                    return Err(SharedLayoutValidationError::DuplicatePaneId(pane.pane_id));
                }
                ws_pane_ids.insert(pane.pane_id);

                if pane.generation == 0 {
                    return Err(SharedLayoutValidationError::InvalidGeneration {
                        pane_id: pane.pane_id,
                        generation: pane.generation,
                    });
                }

                if !pane.scrollable_width.is_finite() || pane.scrollable_width <= 0.0 {
                    return Err(SharedLayoutValidationError::InvalidScrollableWidth(
                        pane.pane_id,
                    ));
                }

                if pane.floating {
                    let Some(rect) = pane.rect else {
                        return Err(SharedLayoutValidationError::MissingFloatingRect(
                            pane.pane_id,
                        ));
                    };
                    if !rect.x.is_finite()
                        || !rect.y.is_finite()
                        || !rect.w.is_finite()
                        || !rect.h.is_finite()
                    {
                        return Err(SharedLayoutValidationError::InvalidFloatingRect {
                            pane_id: pane.pane_id,
                            reason: "coordinates must be finite",
                        });
                    }
                    if rect.w <= 0.0 || rect.h <= 0.0 {
                        return Err(SharedLayoutValidationError::InvalidFloatingRect {
                            pane_id: pane.pane_id,
                            reason: "dimensions must be positive",
                        });
                    }
                } else if pane.rect.is_some() {
                    return Err(SharedLayoutValidationError::InvalidFloatingRect {
                        pane_id: pane.pane_id,
                        reason: "non-floating pane must not carry a floating rect",
                    });
                }
                if pane.pinned && (!pane.floating || pane.fullscreen) {
                    return Err(SharedLayoutValidationError::InvalidPin(pane.pane_id));
                }
            }

            for &ratio in &ws.split_ratios {
                if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                    return Err(SharedLayoutValidationError::InvalidSplitRatio {
                        workspace: ws.index,
                    });
                }
            }

            if let Some(tree) = &ws.tree {
                let mut seen_leaves = std::collections::HashSet::new();
                validate_shared_tree(tree, ws.index, &ws_pane_ids, &mut seen_leaves)?;
            }
        }

        Ok(())
    }
}

fn validate_shared_tree(
    tree: &SharedTree,
    workspace_idx: usize,
    workspace_panes: &std::collections::HashSet<PaneId>,
    seen_leaves: &mut std::collections::HashSet<PaneId>,
) -> std::result::Result<(), SharedLayoutValidationError> {
    match tree {
        SerializedTree::Leaf { pane } => {
            if !workspace_panes.contains(pane) {
                return Err(SharedLayoutValidationError::TreeLeafNotInWorkspace {
                    workspace: workspace_idx,
                    pane_id: *pane,
                });
            }
            if !seen_leaves.insert(*pane) {
                return Err(SharedLayoutValidationError::DuplicateTreeLeaf {
                    workspace: workspace_idx,
                    pane_id: *pane,
                });
            }
            Ok(())
        }
        SerializedTree::Split {
            ratio,
            first,
            second,
            ..
        } => {
            if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                return Err(SharedLayoutValidationError::InvalidTreeSplitRatio {
                    workspace: workspace_idx,
                });
            }
            validate_shared_tree(first, workspace_idx, workspace_panes, seen_leaves)?;
            validate_shared_tree(second, workspace_idx, workspace_panes, seen_leaves)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::shared::test_support::layout_with_panes;

    #[test]
    fn shared_layout_validation_enforces_invariants() {
        let valid = layout_with_panes(&[(1, 1), (2, 1)]);
        assert_eq!(valid.validate(), Ok(()));

        let mut bad = valid.clone();
        bad.pane_geometry.padding.0 = 9;
        assert_eq!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidPaneGeometry)
        );
        bad.pane_geometry.padding.0 = 0;
        bad.pane_geometry.top_gap = 2;
        assert_eq!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidPaneGeometry)
        );

        // Unsupported version
        let mut bad = valid.clone();
        bad.version = 999;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::UnsupportedVersion(999))
        ));

        // Zero canvas
        let mut bad = valid.clone();
        bad.canvas_cols = 0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::ZeroCanvasDimensions)
        ));

        // Duplicate workspace index
        let mut bad = valid.clone();
        bad.workspaces.push(bad.workspaces[0].clone());
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicateWorkspaceIndex(0))
        ));

        // Duplicate pane id
        let mut bad = valid.clone();
        bad.workspaces[0].panes[1].pane_id = 1;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicatePaneId(1))
        ));

        // Invalid generation
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].generation = 0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidGeneration {
                pane_id: 1,
                generation: 0
            })
        ));

        // Invalid scrollable width
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].scrollable_width = 0.0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidScrollableWidth(1))
        ));

        // Floating pane missing rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = true;
        bad.workspaces[0].panes[0].rect = None;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::MissingFloatingRect(1))
        ));

        // Floating pane invalid rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = true;
        bad.workspaces[0].panes[0].rect = Some(FracRect {
            x: 0.1,
            y: 0.1,
            w: 0.0,
            h: 0.5,
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidFloatingRect { pane_id: 1, .. })
        ));

        // Tiled pane marked pinned
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = false;
        bad.workspaces[0].panes[0].rect = None;
        bad.workspaces[0].panes[0].pinned = true;
        assert_eq!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidPin(
                bad.workspaces[0].panes[0].pane_id
            ))
        );

        // Pinned fullscreen pane
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = true;
        bad.workspaces[0].panes[0].rect = Some(FracRect {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        bad.workspaces[0].panes[0].fullscreen = true;
        bad.workspaces[0].panes[0].pinned = true;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidPin(_))
        ));

        // Non-floating pane carrying rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = false;
        bad.workspaces[0].panes[0].rect = Some(FracRect {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidFloatingRect { pane_id: 1, .. })
        ));

        // Invalid split ratio
        let mut bad = valid.clone();
        bad.workspaces[0].split_ratios.push(1.5);
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidSplitRatio { workspace: 0 })
        ));

        // Tree leaf not in workspace
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Leaf { pane: 99 });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::TreeLeafNotInWorkspace {
                workspace: 0,
                pane_id: 99
            })
        ));

        // Duplicate tree leaf
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SerializedTree::Leaf { pane: 1 }),
            second: Box::new(SerializedTree::Leaf { pane: 1 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicateTreeLeaf {
                workspace: 0,
                pane_id: 1
            })
        ));

        // Invalid tree split ratio
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.0,
            first: Box::new(SerializedTree::Leaf { pane: 1 }),
            second: Box::new(SerializedTree::Leaf { pane: 2 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidTreeSplitRatio { workspace: 0 })
        ));

        // Workspace index out of range
        let mut bad = valid.clone();
        bad.workspaces[0].index = crate::layout::model::WORKSPACE_COUNT;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidWorkspaceIndex(_))
        ));

        // Reserved popup pane id
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].pane_id = crate::pane::id::POPUP_PANE_ID;
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SerializedTree::Leaf {
                pane: crate::pane::id::POPUP_PANE_ID,
            }),
            second: Box::new(SerializedTree::Leaf { pane: 2 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::ReservedPaneId(_))
        ));
    }
}
