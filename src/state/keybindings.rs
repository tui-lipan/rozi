use tui_lipan::prelude::{KeyBinding, KeyMods, TextInput};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HelpTab {
    #[default]
    Global,
    Modes,
    Unbound,
    All,
}

impl HelpTab {
    /// The tab strip's order, shared by the strip, the keyboard cycle, and `HelpTabSelected`, so
    /// the three cannot disagree about which index is which tab.
    pub const ORDER: [Self; 4] = [Self::Global, Self::Modes, Self::Unbound, Self::All];

    pub fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ORDER.get(index).copied().unwrap_or_default()
    }

    /// The tab `steps` places along the strip, wrapping at both ends.
    pub fn stepped(self, steps: isize) -> Self {
        let count = Self::ORDER.len();
        Self::from_index((self.index() + count.wrapping_add_signed(steps)) % count)
    }
}

/// Occupying the prefix key itself, as opposed to one command that follows it.
pub const PREFIX_CONFLICT_ID: &str = "input.prefix";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub registry_id: String,
    pub config_id: Option<String>,
    pub label: String,
}

impl KeybindingConflict {
    pub fn prefix() -> Self {
        Self {
            registry_id: PREFIX_CONFLICT_ID.to_string(),
            config_id: None,
            label: "Prefix".to_string(),
        }
    }

    pub fn is_prefix(&self) -> bool {
        self.registry_id == PREFIX_CONFLICT_ID
    }
}

/// What a recorded key becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingTarget {
    /// A built-in action or named command, by its `[keys]` id.
    Action(String),
    /// `[input] prefix`.
    Prefix,
}

/// The Mod row's three states. `Off` writes `modifier_shortcuts = false` and keeps
/// `[input] modifier` as written, so which modifier and whether the layer is on stay separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierChoice {
    Alt,
    Super,
    Off,
}

impl ModifierChoice {
    pub const ORDER: [Self; 3] = [Self::Alt, Self::Super, Self::Off];

    pub fn from_input(input: &crate::config::InputConfig) -> Self {
        match (input.modifier_shortcuts, input.modifier) {
            (false, _) => Self::Off,
            (true, crate::config::WmModifier::Alt) => Self::Alt,
            (true, crate::config::WmModifier::Super) => Self::Super,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Alt => "Alt",
            Self::Super => "Super",
            Self::Off => "Off",
        }
    }

    /// The choice `steps` places along [`Self::ORDER`], wrapping at both ends.
    pub fn stepped(self, steps: isize) -> Self {
        let count = Self::ORDER.len();
        let index = Self::ORDER
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or_default();
        Self::ORDER[(index + count.wrapping_add_signed(steps % count as isize)) % count]
    }
}

/// Literal bindings a Prefix or Mod change could re-express against the new scheme. Offered, never
/// applied unless the user turns it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiteralConversion {
    pub count: usize,
    pub enabled: bool,
}

/// What the Keybindings overlay is doing on top of its always-visible list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeybindingEditorStage {
    #[default]
    List,
    Capture {
        target: KeybindingTarget,
    },
    Review {
        target: KeybindingTarget,
        binding: KeyBinding,
        conversion: Option<LiteralConversion>,
    },
    /// The recorded binding would collide. `replaceable` when every other owner is a command the
    /// editor can rewrite; a Prefix change is never replaceable.
    Conflict {
        target: KeybindingTarget,
        binding: KeyBinding,
        conflicts: Vec<KeybindingConflict>,
        replaceable: bool,
    },
    Modifier {
        choice: ModifierChoice,
        conversion: Option<LiteralConversion>,
        conflicts: Vec<KeybindingConflict>,
    },
    ResetAll,
}

/// Everything the Keybindings overlay owns. It exists exactly while the overlay is open, so closing
/// it from anywhere drops the query, tab, selection, and any half-finished capture together.
#[derive(Default)]
pub struct KeybindingsState {
    pub query: TextInput,
    pub tab: HelpTab,
    /// Stable id of the highlighted row. `None`, or an id the current tab and query hide, highlights
    /// the first row.
    pub selected: Option<String>,
    pub stage: KeybindingEditorStage,
    pub held_modifiers: KeyMods,
}

impl KeybindingsState {
    pub fn is_capturing(&self) -> bool {
        matches!(self.stage, KeybindingEditorStage::Capture { .. })
    }
}
