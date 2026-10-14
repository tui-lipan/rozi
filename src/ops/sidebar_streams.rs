//! Extension sidebar tabs fed by `rozi sidebar publish` streams.

use crate::state::{State, TreeExpansionKey};

/// A row was expanded. A stream producer that lists children only on request is asked for them.
pub(crate) fn expanded(_state: &mut State, _key: &TreeExpansionKey) {}
