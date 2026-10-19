use tui_lipan::prelude::TextInput;

/// The Sidebar tabs picker, present while it is open. Its query and highlighted tab survive a trip
/// into a tab's settings, so coming back lands on the same row.
#[derive(Clone, Debug, Default)]
pub struct SidebarManagerState {
    pub query: String,
    /// Id of the highlighted tab.
    pub selected: Option<String>,
}

/// A tab's settings picker, present while it is open.
///
/// It reads the settings themselves from the loaded config on every render rather than holding a
/// copy, so a change written to `config.toml` and reloaded shows without being mirrored here.
pub struct TabSettingsState {
    /// Id of the tab whose settings these are.
    pub tab: String,
    pub query: String,
    /// Key of the highlighted setting.
    pub selected: Option<String>,
    /// The choice setting whose values are listed in place of the settings, if one is open.
    pub choosing: Option<String>,
    /// The typed value being entered for an integer or text setting, if one is open.
    pub prompt: Option<TabSettingPrompt>,
}

impl TabSettingsState {
    pub fn new(tab: String) -> Self {
        Self {
            tab,
            query: String::new(),
            selected: None,
            choosing: None,
            prompt: None,
        }
    }
}

/// A typed value for one setting. The answer is checked on submit; a rejected one stays in the
/// field with the reason beside it.
pub struct TabSettingPrompt {
    pub key: String,
    pub input: TextInput,
    pub error: Option<String>,
}

impl TabSettingPrompt {
    pub fn new(key: String, value: String) -> Self {
        let mut input = TextInput::new(value);
        // Selected, so the first keystroke replaces the current value.
        input.set_anchor(Some(0));
        Self {
            key,
            input,
            error: None,
        }
    }
}
