//! Drag and drop between the host terminal and the programs in panes.

use std::collections::HashMap;

use tui_lipan::dnd::{CommandDecoder, Router};

use crate::pane::id::PaneId;

/// A pane, as a drag addresses it: the identity the session server routes by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropPane {
    pub pane_id: PaneId,
    /// A client-local pane (scratchpad, popup) rather than one the session shares.
    pub local: bool,
    pub generation: u64,
    pub attachment_epoch: u64,
}

/// What this client registered with the host terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostRegistration {
    /// Accepting drops, with these extra MIME types.
    pub accepting: Option<Vec<String>>,
    /// Offering drags on behalf of panes.
    pub offering: bool,
}

pub struct DragAndDropState {
    pub(crate) router: Router<DropPane>,
    /// Reads each pane's commands as the session server relays them.
    pub(crate) decoders: HashMap<DropPane, CommandDecoder>,
    /// The pane a drag from the host is over, which the view marks.
    pub hovered: Option<DropPane>,
    pub(crate) registered: HostRegistration,
    /// The attachment the router's panes belong to; another one starts it afresh.
    pub(crate) attachment_epoch: u64,
}

impl Default for DragAndDropState {
    fn default() -> Self {
        Self {
            router: Router::new(tui_lipan::dnd::MachineId::local()),
            decoders: HashMap::new(),
            hovered: None,
            registered: HostRegistration::default(),
            attachment_epoch: 0,
        }
    }
}
