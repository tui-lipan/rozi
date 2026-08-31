use serde::{Deserialize, Serialize};

use crate::layout::tiling::DwindleTree;
use crate::state::{LayoutKind, PaneId, SplitAxis};

/// Serde-stable tree shared by profile TOML and the session layout document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SerializedTree<L> {
    Leaf {
        pane: L,
    },
    Split {
        axis: SerializedSplitAxis,
        ratio: f32,
        first: Box<SerializedTree<L>>,
        second: Box<SerializedTree<L>>,
    },
}

impl<L: Default> Default for SerializedTree<L> {
    fn default() -> Self {
        Self::Leaf { pane: L::default() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerializedLayoutKind {
    #[default]
    Dwindle,
    Master,
    Grid,
    Columns,
    Rows,
    Scrollable,
    Monocle,
}

impl From<LayoutKind> for SerializedLayoutKind {
    fn from(layout: LayoutKind) -> Self {
        match layout {
            LayoutKind::Dwindle => Self::Dwindle,
            LayoutKind::Master => Self::Master,
            LayoutKind::Grid => Self::Grid,
            LayoutKind::Columns => Self::Columns,
            LayoutKind::Rows => Self::Rows,
            LayoutKind::Scrollable => Self::Scrollable,
            LayoutKind::Monocle => Self::Monocle,
        }
    }
}

impl From<SerializedLayoutKind> for LayoutKind {
    fn from(layout: SerializedLayoutKind) -> Self {
        match layout {
            SerializedLayoutKind::Dwindle => Self::Dwindle,
            SerializedLayoutKind::Master => Self::Master,
            SerializedLayoutKind::Grid => Self::Grid,
            SerializedLayoutKind::Columns => Self::Columns,
            SerializedLayoutKind::Rows => Self::Rows,
            SerializedLayoutKind::Scrollable => Self::Scrollable,
            SerializedLayoutKind::Monocle => Self::Monocle,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerializedSplitAxis {
    #[default]
    Horizontal,
    Vertical,
}

impl From<SplitAxis> for SerializedSplitAxis {
    fn from(axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => Self::Horizontal,
            SplitAxis::Vertical => Self::Vertical,
        }
    }
}

impl From<SerializedSplitAxis> for SplitAxis {
    fn from(axis: SerializedSplitAxis) -> Self {
        match axis {
            SerializedSplitAxis::Horizontal => Self::Horizontal,
            SerializedSplitAxis::Vertical => Self::Vertical,
        }
    }
}

pub(crate) fn from_dwindle<L, F>(tree: &DwindleTree, resolve_leaf: &F) -> Option<SerializedTree<L>>
where
    F: Fn(PaneId) -> Option<L>,
{
    match tree {
        DwindleTree::Leaf(id) => resolve_leaf(*id).map(|pane| SerializedTree::Leaf { pane }),
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => Some(SerializedTree::Split {
            axis: (*axis).into(),
            ratio: *ratio,
            first: Box::new(from_dwindle(first, resolve_leaf)?),
            second: Box::new(from_dwindle(second, resolve_leaf)?),
        }),
    }
}

/// Resolve a serialized tree back to the runtime representation.
///
/// Profiles use strict restoration (`collapse_missing = false`) because positional leaves must all
/// resolve. Shared layouts prune unknown pane ids and promote the surviving side of a split.
pub(crate) fn to_dwindle<L, F>(
    tree: &SerializedTree<L>,
    resolve_leaf: &F,
    collapse_missing: bool,
) -> Option<DwindleTree>
where
    F: Fn(&L) -> Option<PaneId>,
{
    match tree {
        SerializedTree::Leaf { pane } => resolve_leaf(pane).map(DwindleTree::Leaf),
        SerializedTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let first = to_dwindle(first, resolve_leaf, collapse_missing);
            let second = to_dwindle(second, resolve_leaf, collapse_missing);
            match (first, second) {
                (Some(first), Some(second)) => Some(DwindleTree::Split {
                    axis: (*axis).into(),
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) if collapse_missing => Some(only),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_layout_kind_uses_kebab_case_for_new_variants() {
        for (kind, label) in [
            (SerializedLayoutKind::Columns, "columns"),
            (SerializedLayoutKind::Rows, "rows"),
            (SerializedLayoutKind::Scrollable, "scrollable"),
        ] {
            let encoded = serde_json::to_value(kind).expect("encode");
            assert_eq!(encoded, label);
            let decoded: SerializedLayoutKind = serde_json::from_value(encoded).expect("decode");
            assert_eq!(decoded, kind);
            assert_eq!(LayoutKind::from(kind).label(), label);

            let toml = format!("layout = \"{label}\"\n");
            #[derive(serde::Deserialize)]
            struct Wrap {
                layout: SerializedLayoutKind,
            }
            assert_eq!(toml::from_str::<Wrap>(&toml).unwrap().layout, kind);
        }
    }
}
