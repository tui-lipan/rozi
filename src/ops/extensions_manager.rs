use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{
    ExtensionInfo, ExtensionSettings, ExtensionStatus, ReportKind, ReportRow, ReportSection,
    ReportTone,
};
use crate::state::{
    CatalogExtensionDetailState, ExtensionDetailState, ExtensionPickerRow, ExtensionsState,
    ExtensionsTab,
};

pub(crate) const EXTENSION_UPDATING_LABEL: &str = "updating…";

static NEXT_UPDATE_CHECK_EPOCH: AtomicU64 = AtomicU64::new(1);
static NEXT_CATALOG_EPOCH: AtomicU64 = AtomicU64::new(1);

struct ManagerScan {
    entries: Vec<ExtensionInfo>,
    merged: BTreeMap<String, ExtensionSettings>,
    manifest_entries: BTreeSet<String>,
    removable_entries: BTreeSet<String>,
    installation_kinds: BTreeMap<String, crate::extension_installation::InstallKind>,
}

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    let scan = scan(ctx);
    let update_check_epoch = next_update_check_epoch();
    let catalog_epoch = next_catalog_epoch();
    let git_ids = git_installation_ids(&scan.installation_kinds);
    ctx.state.show_palette = false;
    ctx.state.keybindings = None;
    ctx.state.show_settings = false;
    ctx.state.settings_selected = None;
    if let Some(delay) = crate::state::abandon_settings_choice(&mut ctx.state) {
        ctx.set_command_chord_reveal_delay(delay);
    }
    ctx.state.pane_padding_editor = None;
    ctx.state.extensions = Some(ExtensionsState {
        tab: ExtensionsTab::Installed,
        entries: scan.entries,
        merged: scan.merged,
        selected: 0,
        catalog_selected: None,
        query: TextInput::new(""),
        pending_remove: None,
        detail: None,
        install_prompt: None,
        catalog_entries: Vec::new(),
        catalog_error: None,
        catalog_epoch,
        catalog_loading: false,
        catalog_detail: None,
        installation_kinds: scan.installation_kinds,
        available_updates: BTreeSet::new(),
        update_check_epoch,
        updating_id: None,
        manifest_entries: scan.manifest_entries,
        removable_entries: scan.removable_entries,
    });
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_extensions_focus(ctx);
    request_update_checks(ctx, update_check_epoch, git_ids);
    // Loaded with the picker rather than the tab, so Discover is ready when it is first shown.
    load_catalog(ctx);
    normalize_selection(ctx);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.extensions = None;
    ctx.state.commands_dirty = true;
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn tab_selected(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    let tab = ExtensionsTab::from_index(index);
    if state.tab == tab {
        return Update::none();
    }
    state.tab = tab;
    state.pending_remove = None;
    normalize_selection(ctx);
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    event.apply_to(&mut state.query);
    normalize_selection(ctx);
    Update::full()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, row: ExtensionPickerRow) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    // A row is only ever chosen from the tab showing it.
    let changed = match row {
        ExtensionPickerRow::Installed(index) => {
            let changed = state.tab != ExtensionsTab::Installed || state.selected != index;
            state.tab = ExtensionsTab::Installed;
            state.selected = index.min(state.entries.len().saturating_sub(1));
            changed
        }
        ExtensionPickerRow::Catalog(index) => {
            let index = index.min(state.catalog_entries.len().saturating_sub(1));
            let changed =
                state.tab != ExtensionsTab::Discover || state.catalog_selected != Some(index);
            state.tab = ExtensionsTab::Discover;
            state.catalog_selected = Some(index);
            changed
        }
    };
    if changed {
        state.pending_remove = None;
    }
    Update::full()
}

pub(crate) fn toggle_selected(ctx: &mut Context<AppRoot>) -> Update {
    if ctx
        .state
        .extensions
        .as_ref()
        .is_some_and(|state| state.tab == ExtensionsTab::Discover)
    {
        return open_detail(ctx);
    }
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    if !matches!(
        entry.status,
        ExtensionStatus::Loaded | ExtensionStatus::Disabled
    ) {
        return Update::none();
    }
    let Some(id) = entry.id.clone() else {
        return Update::none();
    };
    let mut user = match crate::config::read_user_extension_config() {
        Ok(user) => user,
        Err(error) => {
            notify_error(ctx, "Extension not changed", error);
            return Update::full();
        }
    };
    let disabled = entry.status == ExtensionStatus::Loaded;
    if disabled {
        if !user.disabled.iter().any(|candidate| candidate.trim() == id) {
            user.disabled.push(id.clone());
        }
    } else {
        user.disabled.retain(|candidate| candidate.trim() != id);
    }
    user.disabled.sort();
    user.disabled.dedup();
    if let Err(error) = crate::config::persist_extensions_disabled(&user.disabled) {
        notify_error(ctx, "Extension not changed", error);
        return Update::full();
    }

    // The row moves between the Active and Disabled groups and the enter hint flips, so the
    // toggle needs no toast. Only newly suppressed suggestions are worth interrupting for.
    let update = crate::ops::config::reload_extensions_quiet(ctx);
    if !disabled {
        warn_about_key_conflicts(ctx, &id);
    }
    update
}

/// Ctrl+R reloads what the active tab shows: the index on Discover, installed manifests on
/// Installed. The spinner or the rescanned rows are the confirmation; problems still toast.
pub(crate) fn reload(ctx: &mut Context<AppRoot>) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if state.tab == ExtensionsTab::Discover {
        start_catalog_load(ctx);
        return Update::full();
    }
    crate::ops::config::reload_extensions_quiet(ctx)
}

pub(crate) fn open_install(ctx: &mut Context<AppRoot>) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.install_prompt = Some(crate::state::ExtensionInstallPromptState {
        input: TextInput::new(""),
        error: None,
        error_scroll_offset: 0,
        error_scroll_max: None,
        installing: false,
    });
    crate::ops::focus::request_extension_install_focus(ctx);
    Update::full()
}

pub(crate) fn install_source_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    let Some(prompt) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.install_prompt.as_mut())
    else {
        return Update::none();
    };
    if prompt.installing {
        return Update::none();
    }
    event.apply_to(&mut prompt.input);
    prompt.error = None;
    prompt.error_scroll_offset = 0;
    prompt.error_scroll_max = None;
    Update::full()
}

pub(crate) fn scroll_install_error_by(ctx: &mut Context<AppRoot>, delta: isize) -> Update {
    let Some(prompt) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.install_prompt.as_mut())
        .filter(|prompt| prompt.error.is_some())
    else {
        return Update::none();
    };
    let offset = if delta < 0 {
        prompt
            .error_scroll_offset
            .saturating_sub(delta.unsigned_abs())
    } else {
        prompt.error_scroll_offset.saturating_add(delta as usize)
    };
    let offset = prompt
        .error_scroll_max
        .map_or(offset, |max| offset.min(max));
    if offset == prompt.error_scroll_offset {
        return Update::none();
    }
    prompt.error_scroll_offset = offset;
    Update::full()
}

pub(crate) fn install_error_scrolled(
    ctx: &mut Context<AppRoot>,
    offset: usize,
    max_offset: usize,
) -> Update {
    let Some(prompt) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.install_prompt.as_mut())
        .filter(|prompt| prompt.error.is_some())
    else {
        return Update::none();
    };
    let offset = offset.min(max_offset);
    if prompt.error_scroll_offset == offset && prompt.error_scroll_max == Some(max_offset) {
        return Update::none();
    }
    prompt.error_scroll_offset = offset;
    prompt.error_scroll_max = Some(max_offset);
    Update::full()
}

pub(crate) fn close_install(ctx: &mut Context<AppRoot>) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if state
        .install_prompt
        .as_ref()
        .is_some_and(|prompt| prompt.installing)
    {
        return Update::none();
    }
    state.install_prompt = None;
    crate::ops::focus::request_extensions_focus(ctx);
    Update::full()
}

pub(crate) fn submit_install(ctx: &mut Context<AppRoot>) -> Update {
    let Some(prompt) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.install_prompt.as_mut())
    else {
        return Update::none();
    };
    if prompt.installing {
        return Update::none();
    }
    let source = prompt.input.text().trim().to_string();
    if source.is_empty() {
        prompt.error = Some("Enter a local path or Git URL".to_string());
        prompt.error_scroll_offset = 0;
        prompt.error_scroll_max = None;
        return Update::full();
    }
    prompt.error = None;
    prompt.error_scroll_offset = 0;
    prompt.error_scroll_max = None;
    prompt.installing = true;
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            let result = crate::extension_installation::install(
                crate::extension_installation::InstallRequest::Source(source),
            )
            .map(|installed| installed.id);
            link.send(crate::Msg::ExtensionsInstallFinished(result));
        });
    }))
}

pub(crate) fn install_finished(
    ctx: &mut Context<AppRoot>,
    result: std::result::Result<String, String>,
) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if let Some(prompt) = state.install_prompt.as_mut() {
        prompt.installing = false;
    }
    match result {
        Ok(id) => {
            state.install_prompt = None;
            // The prompt closes onto the new row, selected, with its version, source, and
            // keybinding counts in the description. Only the conflicts need a toast.
            let update = crate::ops::config::reload_extensions_quiet(ctx);
            show_installed(ctx, &id);
            warn_about_key_conflicts(ctx, &id);
            update
        }
        Err(error) => {
            if let Some(prompt) = ctx
                .state
                .extensions
                .as_mut()
                .and_then(|state| state.install_prompt.as_mut())
            {
                prompt.error = Some(error);
                prompt.error_scroll_offset = 0;
                prompt.error_scroll_max = None;
                crate::ops::focus::request_extension_install_focus(ctx);
                Update::full()
            } else {
                notify_error(ctx, "Extension not installed", error);
                Update::full()
            }
        }
    }
}

pub(crate) fn catalog_loaded(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    result: std::result::Result<Vec<crate::extension_catalog::CatalogEntry>, String>,
) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if state.catalog_epoch != epoch {
        return Update::none();
    }
    state.catalog_loading = false;
    match result {
        Ok(entries) => {
            // Rows are addressed by position, and a refresh may reorder or drop them, so the
            // selection follows its repository rather than its old index. An open report holds its
            // own snapshot and is unaffected.
            let selected = state
                .catalog_selected
                .and_then(|index| state.catalog_entries.get(index))
                .map(|entry| entry.repository.clone());
            state.catalog_entries = entries;
            state.catalog_error = None;
            state.catalog_selected = selected.and_then(|repository| {
                state
                    .catalog_entries
                    .iter()
                    .position(|entry| entry.repository == repository)
            });
        }
        Err(error) => {
            // Rows already listed, from the cache or an earlier fetch, stay usable offline.
            state.catalog_error = Some(error);
        }
    }
    normalize_selection(ctx);
    Update::full()
}

pub(crate) fn submit_catalog_install(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.extension_catalog_install.is_some() {
        return Update::none();
    }
    let Some(detail) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.catalog_detail.as_mut())
    else {
        return Update::none();
    };
    if detail.entry.incompatibility().is_some() {
        return Update::none();
    }
    let entry = detail.entry.clone();
    if ctx
        .state
        .extensions
        .as_ref()
        .is_some_and(|state| catalog_entry_installed(state, &entry))
    {
        return Update::none();
    }
    let Some(detail) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.catalog_detail.as_mut())
    else {
        return Update::none();
    };
    detail.error = None;
    let entry = detail.entry.clone();
    let repository = entry.repository.clone();
    ctx.state.extension_catalog_install = Some(repository.clone());
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            let result = crate::extension_installation::install(
                crate::extension_installation::InstallRequest::Catalog {
                    source: entry.source,
                    commit: entry.commit,
                },
            )
            .map(|installed| installed.id);
            link.send(crate::Msg::ExtensionsCatalogInstallFinished { repository, result });
        });
    }))
}

/// Finishes a discovery installation whatever became of the UI that started it.
///
/// The installation is already on disk when this runs, so a success always reloads extensions,
/// even when the user closed the report or the whole manager meanwhile. Only the presentation
/// depends on what is still open.
pub(crate) fn catalog_install_finished(
    ctx: &mut Context<AppRoot>,
    repository: String,
    result: std::result::Result<String, String>,
) -> Update {
    if ctx.state.extension_catalog_install.as_deref() != Some(repository.as_str()) {
        return Update::none();
    }
    ctx.state.extension_catalog_install = None;
    let detail_open = ctx.state.extensions.as_ref().is_some_and(|state| {
        state
            .catalog_detail
            .as_ref()
            .is_some_and(|detail| detail.entry.repository == repository)
    });
    match result {
        Ok(installed_id) => {
            if detail_open && let Some(state) = ctx.state.extensions.as_mut() {
                state.catalog_detail = None;
                crate::ops::focus::request_extensions_focus(ctx);
            }
            let update = crate::ops::config::reload_extensions_quiet(ctx);
            if ctx.state.extensions.is_some() {
                // The user waiting on the report is taken to the new row. Anyone who moved on
                // stays where they are; the toast below tells them.
                if detail_open {
                    show_installed(ctx, &installed_id);
                }
                warn_about_key_conflicts(ctx, &installed_id);
            }
            if !detail_open {
                // The user moved on before this finished, so the new row alone may go unseen.
                notify_info(ctx, &format!("Installed {installed_id}"));
            }
            update
        }
        Err(error) => {
            if detail_open
                && let Some(detail) = ctx
                    .state
                    .extensions
                    .as_mut()
                    .and_then(|state| state.catalog_detail.as_mut())
            {
                detail.error = Some(error);
            } else {
                notify_error(ctx, "Extension not installed", error);
            }
            Update::full()
        }
    }
}

pub(crate) fn update_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    let Some(id) = entry.id.clone() else {
        return Update::none();
    };
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if state.updating_id.is_some()
        || state.installation_kinds.get(&id)
            != Some(&crate::extension_installation::InstallKind::Git)
    {
        return Update::none();
    }
    state.updating_id = Some(id.clone());
    state.update_check_epoch = next_update_check_epoch();
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            let result = crate::extension_installation::update(&id).map(|updated| updated.changed);
            link.send(crate::Msg::ExtensionsUpdateFinished { id, result });
        });
    }))
}

pub(crate) fn update_finished(
    ctx: &mut Context<AppRoot>,
    id: String,
    result: std::result::Result<bool, String>,
) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return match result {
            Ok(true) => {
                notify_info(ctx, &format!("Updated {id}"));
                crate::ops::config::reload_extensions_quiet(ctx)
            }
            Ok(false) => {
                notify_info(ctx, &format!("{id} is up to date"));
                Update::full()
            }
            Err(error) => {
                notify_error(ctx, "Extension not updated", error);
                Update::full()
            }
        };
    };
    if state.updating_id.as_deref() != Some(&id) {
        return Update::none();
    }
    state.updating_id = None;
    state.available_updates.remove(&id);
    match result {
        Ok(true) => {
            let update = crate::ops::config::reload_extensions_quiet(ctx);
            select_by_id(ctx, &id);
            update
        }
        Ok(false) => {
            notify_info(ctx, &format!("{id} is up to date"));
            Update::full()
        }
        Err(error) => {
            notify_error(ctx, "Extension not updated", error);
            Update::full()
        }
    }
}

pub(crate) fn updates_checked(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    available: Vec<String>,
) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    if state.update_check_epoch != epoch {
        return Update::none();
    }
    state.available_updates = available.into_iter().collect();
    Update::full()
}

pub(crate) fn open_manifest(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    let path = match installation_path(&entry) {
        Ok(path) => path.join("extension.toml"),
        Err(error) => {
            notify_error(ctx, "Manifest not opened", error);
            return Update::full();
        }
    };
    if !path.is_file() {
        return Update::none();
    }
    ctx.state.extensions = None;
    ctx.state.overlay_return = None;
    ctx.state.commands_dirty = true;
    crate::ops::config::open_file_in_editor(
        ctx,
        path.clone(),
        crate::state::PendingSessionAction::OpenFile(path),
    )
}

/// Opens the open report's web link: a discovery entry's source at the exact indexed commit, or an
/// installed extension's declared homepage.
pub(crate) fn open_link(ctx: &mut Context<AppRoot>) -> Update {
    let Some(url) = report_link(&ctx.state) else {
        return Update::none();
    };
    match tui_lipan::utils::open_url(&url) {
        Ok(()) => Update::none(),
        Err(error) => {
            notify_error(ctx, "Could not open link", error.to_string());
            Update::full()
        }
    }
}

/// The web link the open report offers, if any.
pub(crate) fn report_link(state: &crate::state::State) -> Option<String> {
    let extensions = state.extensions.as_ref()?;
    if let Some(detail) = extensions.catalog_detail.as_ref() {
        return Some(catalog_source_url(&detail.entry));
    }
    let detail = extensions.detail.as_ref()?;
    extensions
        .entries
        .iter()
        .find(|entry| entry.path == detail.path)?
        .homepage
        .clone()
}

/// The repository at the commit Rozi would install, not its moving default branch, so the source
/// a user inspects is the source they get. The index only admits canonical GitHub repositories
/// and full commit ids, so this is always a well-formed GitHub URL.
pub(crate) fn catalog_source_url(entry: &crate::extension_catalog::CatalogEntry) -> String {
    format!(
        "https://github.com/{}/tree/{}",
        entry.repository, entry.commit
    )
}

pub(crate) fn copy_report(ctx: &mut Context<AppRoot>) -> Update {
    let sections = if let Some(detail) = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.detail.as_ref())
    {
        detail.sections.clone()
    } else {
        let Some(entry) = selected_entry(&ctx.state) else {
            return Update::none();
        };
        let merged = merged_for(&ctx.state, entry);
        crate::config::report_sections(entry, &merged)
    };
    copy(
        ctx,
        &crate::config::report_text(&sections),
        "Copied extension report",
    )
}

pub(crate) fn remove_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    let path = match installation_path(&entry) {
        Ok(path) => path,
        Err(error) => {
            notify_error(ctx, "Extension not removed", error);
            return Update::full();
        }
    };
    let row = identity(&entry);
    let armed = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.pending_remove.as_deref())
        == Some(row.as_str());
    if !armed {
        if let Some(state) = ctx.state.extensions.as_mut() {
            state.pending_remove = Some(row);
        }
        return crate::ops::confirm::arm(ctx);
    }

    if let Some(state) = ctx.state.extensions.as_mut() {
        state.pending_remove = None;
    }
    let stopped = match stop_before_removal(ctx, &entry) {
        Ok(update) => update,
        Err(error) => {
            notify_error(ctx, "Extension not removed", error);
            return Update::full();
        }
    };
    if let Err(error) = crate::platform::extensions::remove_installation(
        &crate::config::extensions_dir_path(),
        &path,
    ) {
        refresh(ctx, Some(row));
        notify_error(
            ctx,
            if stopped.is_some() {
                "Extension disabled; removal failed"
            } else {
                "Extension not removed"
            },
            error,
        );
        return stopped.unwrap_or_else(Update::full);
    }
    if let Some(id) = entry.id.as_deref()
        && !duplicate_installation_remains(&ctx.state, &entry)
        && let Err(error) = crate::extension_installation::forget_installation_record(id)
    {
        notify_error(ctx, "Extension metadata not removed", error);
    }
    cleanup_disabled_after_removal(ctx, &entry);

    // The row leaves the list under the armed confirmation the user just pressed twice.
    crate::ops::config::reload_extensions_quiet(ctx)
}

pub(crate) fn open_detail(ctx: &mut Context<AppRoot>) -> Update {
    if ctx
        .state
        .extensions
        .as_ref()
        .is_some_and(|state| state.tab == ExtensionsTab::Discover)
    {
        let Some(entry) = selected_catalog_entry(&ctx.state).cloned() else {
            return Update::none();
        };
        let Some(state) = ctx.state.extensions.as_mut() else {
            return Update::none();
        };
        state.catalog_detail = Some(CatalogExtensionDetailState { entry, error: None });
        crate::ops::focus::request_extension_detail_focus(ctx);
        return Update::full();
    }
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    let path = identity(&entry);
    let merged = merged_for(&ctx.state, &entry);
    let sections = crate::config::report_sections(&entry, &merged);
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.detail = Some(ExtensionDetailState { path, sections });
    crate::ops::focus::request_extension_detail_focus(ctx);
    Update::full()
}

pub(crate) fn close_detail(ctx: &mut Context<AppRoot>) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.detail = None;
    state.catalog_detail = None;
    crate::ops::focus::request_extensions_focus(ctx);
    Update::full()
}

pub(crate) fn config_reloaded(ctx: &mut Context<AppRoot>) {
    if ctx.state.extensions.is_none() {
        return;
    }
    let selected = selected_entry(&ctx.state).map(identity);
    refresh(ctx, selected);
    start_update_check(ctx);
}

fn refresh(ctx: &mut Context<AppRoot>, selected: Option<String>) {
    let detail_path = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.detail.as_ref())
        .map(|detail| detail.path.clone());
    let scan = scan(ctx);
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    state.entries = scan.entries;
    state.merged = scan.merged;
    state.manifest_entries = scan.manifest_entries;
    state.removable_entries = scan.removable_entries;
    state.installation_kinds = scan.installation_kinds;
    state
        .available_updates
        .retain(|id| state.installation_kinds.contains_key(id));
    state.selected = selected
        .as_deref()
        .and_then(|selected| {
            state
                .entries
                .iter()
                .position(|entry| identity(entry) == selected)
        })
        .unwrap_or(0)
        .min(state.entries.len().saturating_sub(1));
    state.pending_remove = None;
    state.detail = detail_path.and_then(|detail_path| {
        let entry = state
            .entries
            .iter()
            .find(|entry| identity(entry) == detail_path)?;
        let merged = state
            .merged
            .get(&detail_path)
            .cloned()
            .unwrap_or_else(|| entry.settings.clone());
        Some(ExtensionDetailState {
            path: detail_path,
            sections: crate::config::report_sections(entry, &merged),
        })
    });
    normalize_selection(ctx);
}

fn scan(ctx: &mut Context<AppRoot>) -> ManagerScan {
    let user: crate::config::UserExtensionConfig = match crate::config::read_user_extension_config()
    {
        Ok(user) => user,
        Err(error) => {
            notify_error(ctx, "Extension config unreadable", error);
            Default::default()
        }
    };
    let scan = crate::config::scan_extensions_with_user_config(&user);
    for error in &scan.root_errors {
        notify_error(ctx, "Extension scan failed", error.clone());
    }
    let mut entries = scan.entries();
    crate::config::apply_suggested_keybinding_resolutions(
        &mut entries,
        &ctx.state.config.suggested_keybinding_resolutions,
    );
    let mut merged = BTreeMap::new();
    let manifest_entries = entries
        .iter()
        .filter(|entry| {
            installation_path(entry).is_ok_and(|path| path.join("extension.toml").is_file())
        })
        .map(identity)
        .collect();
    let removable_entries = entries
        .iter()
        .filter(|entry| installation_path(entry).is_ok())
        .map(identity)
        .collect();
    let installation_kinds = entries
        .iter()
        .filter_map(|entry| {
            let id = entry.id.as_ref()?;
            crate::extension_installation::installation_kind(id).map(|kind| (id.clone(), kind))
        })
        .collect();
    for entry in &mut entries {
        let Some(id) = entry.id.clone() else {
            continue;
        };
        let mut warnings = Vec::new();
        let effective = crate::config::merge_extension_settings(
            &entry.settings,
            &id,
            user.settings.get(&id),
            &mut warnings,
        );
        entry.errors.extend(
            warnings
                .into_iter()
                .map(|warning| format!("Config warning: {warning}")),
        );
        merged.insert(identity(entry), effective);
    }
    ManagerScan {
        entries,
        merged,
        manifest_entries,
        removable_entries,
        installation_kinds,
    }
}

fn next_update_check_epoch() -> u64 {
    NEXT_UPDATE_CHECK_EPOCH.fetch_add(1, Ordering::Relaxed)
}

fn next_catalog_epoch() -> u64 {
    NEXT_CATALOG_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// Lists the cached index at once, then fetches unless that copy is fresh.
fn load_catalog(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    let cached = crate::extension_catalog::cached();
    let refresh = !cached.as_ref().is_some_and(|cached| cached.fresh);
    if let Some(cached) = cached {
        state.catalog_entries = cached.entries;
    }
    if refresh {
        start_catalog_load(ctx);
    }
}

fn start_catalog_load(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    let epoch = next_catalog_epoch();
    state.catalog_epoch = epoch;
    state.catalog_error = None;
    let loading = request_catalog(ctx, epoch);
    if let Some(state) = ctx.state.extensions.as_mut() {
        state.catalog_loading = loading;
    }
}

/// Starts a background fetch, reporting whether one is now running.
fn request_catalog(ctx: &Context<AppRoot>, epoch: u64) -> bool {
    if crate::platform::paths::user_dirs_are_isolated() {
        return false;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return false;
    };
    std::thread::spawn(move || {
        let result = crate::extension_catalog::fetch();
        link.send(crate::Msg::ExtensionsCatalogLoaded { epoch, result });
    });
    true
}

fn git_installation_ids(
    kinds: &BTreeMap<String, crate::extension_installation::InstallKind>,
) -> Vec<String> {
    kinds
        .iter()
        .filter(|(_, kind)| **kind == crate::extension_installation::InstallKind::Git)
        .map(|(id, _)| id.clone())
        .collect()
}

fn start_update_check(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    let epoch = next_update_check_epoch();
    state.update_check_epoch = epoch;
    let ids = git_installation_ids(&state.installation_kinds);
    request_update_checks(ctx, epoch, ids);
}

fn request_update_checks(ctx: &Context<AppRoot>, epoch: u64, ids: Vec<String>) {
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    std::thread::spawn(move || {
        let available = ids
            .into_iter()
            .filter(|id| crate::extension_installation::update_available(id).unwrap_or(false))
            .collect();
        link.send(crate::Msg::ExtensionsUpdatesChecked { epoch, available });
    });
}

fn select_by_id(ctx: &mut Context<AppRoot>, id: &str) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    if let Some(index) = state
        .entries
        .iter()
        .position(|entry| entry.id.as_deref() == Some(id))
    {
        state.selected = index;
    }
}

/// Lands on a newly installed extension: the Installed tab, unfiltered, with its row selected.
fn show_installed(ctx: &mut Context<AppRoot>, id: &str) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    state.tab = ExtensionsTab::Installed;
    state.query = TextInput::new("");
    select_by_id(ctx, id);
    normalize_selection(ctx);
}

/// Installed rows visible under the query, by group in display order. Empty groups are left out.
pub(crate) fn installed_groups(state: &ExtensionsState) -> Vec<(&'static str, Vec<usize>)> {
    let visible = matching(
        state.query.text(),
        state
            .entries
            .iter()
            .map(|entry| (entry.display_name(), extension_description(entry, state))),
    );
    [
        ("Active", ExtensionStatus::Loaded),
        ("Disabled", ExtensionStatus::Disabled),
    ]
    .into_iter()
    .map(|(title, status)| (title, Some(status)))
    .chain([("Problems", None)])
    .map(|(title, status)| {
        let rows = (0..state.entries.len())
            .filter(|index| visible[*index])
            .filter(|index| {
                let entry_status = state.entries[*index].status;
                match status {
                    Some(status) => entry_status == status,
                    None => !matches!(
                        entry_status,
                        ExtensionStatus::Loaded | ExtensionStatus::Disabled
                    ),
                }
            })
            .collect::<Vec<_>>();
        (title, rows)
    })
    .filter(|(_, rows)| !rows.is_empty())
    .collect()
}

/// Discovery rows visible under the query, in index order.
pub(crate) fn visible_catalog(state: &ExtensionsState) -> Vec<usize> {
    let visible = matching(
        state.query.text(),
        state.catalog_entries.iter().map(|entry| {
            // The id and the extension's own description are searchable without being shown.
            let description = format!(
                "{} {} {}",
                catalog_description(entry, catalog_entry_installed(state, entry)),
                entry.id,
                entry.description
            );
            (entry.title.as_str(), description)
        }),
    );
    (0..state.catalog_entries.len())
        .filter(|index| visible[*index])
        .collect()
}

/// Whether each row matches `query`, matched the way every Rozi picker matches.
fn matching<'a>(query: &str, rows: impl Iterator<Item = (&'a str, String)>) -> Vec<bool> {
    let items: Vec<_> = rows
        .map(|(label, description)| {
            SearchItem::new(label, ()).description(ItemDescription::new().right(description))
        })
        .collect();
    let mut visible = vec![query.trim().is_empty(); items.len()];
    if query.trim().is_empty() {
        return visible;
    }
    for index in tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    ) {
        visible[index] = true;
    }
    visible
}

/// Keeps each tab's selection on a visible row, falling back to its first one.
fn normalize_selection(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    let installed: Vec<usize> = installed_groups(state)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .collect();
    if !installed.contains(&state.selected)
        && let Some(first) = installed.first()
    {
        state.selected = *first;
    }
    let catalog = visible_catalog(state);
    if !state
        .catalog_selected
        .is_some_and(|index| catalog.contains(&index))
    {
        state.catalog_selected = catalog.first().copied();
    }
}

pub(crate) fn catalog_entry_installed(
    state: &ExtensionsState,
    entry: &crate::extension_catalog::CatalogEntry,
) -> bool {
    state
        .entries
        .iter()
        .any(|installed| installed.id.as_deref() == Some(entry.id.as_str()))
}

/// The right-aligned picker description for one row.
///
/// The manager renders this and matches queries against it, so a row the palette surfaced on a
/// description word stays actionable. The `Disabled` and `Problems` groups already name a status,
/// but a problem row's detail also carries its first error, so only the bare `disabled` label is
/// left out.
pub(crate) fn extension_description(entry: &ExtensionInfo, state: &ExtensionsState) -> String {
    let id = entry.id.as_deref();
    if id.is_some_and(|id| state.updating_id.as_deref() == Some(id)) {
        return EXTENSION_UPDATING_LABEL.to_string();
    }
    let mut parts = Vec::new();
    if let Some(version) = entry.version.as_deref() {
        parts.push(version.to_string());
    }
    parts.push(
        installation_kind_label(id.and_then(|id| state.installation_kinds.get(id))).to_string(),
    );
    if !matches!(
        entry.status,
        ExtensionStatus::Loaded | ExtensionStatus::Disabled
    ) {
        parts.push(entry.status_detail());
    }
    if id.is_some_and(|id| state.available_updates.contains(id)) {
        parts.push("update available".to_string());
    }
    let active = suggested_keybindings(
        entry,
        crate::config::ExtensionSuggestedKeybindingStatus::Active,
    );
    let conflicts = suggested_keybindings(
        entry,
        crate::config::ExtensionSuggestedKeybindingStatus::Conflict,
    );
    if active > 0 {
        parts.push(format!(
            "{active} key{} active",
            if active == 1 { "" } else { "s" }
        ));
    }
    if conflicts > 0 {
        parts.push(format!(
            "{conflicts} key conflict{}",
            if conflicts == 1 { "" } else { "s" }
        ));
    }
    parts.join(" · ")
}

pub(crate) fn catalog_description(
    entry: &crate::extension_catalog::CatalogEntry,
    installed: bool,
) -> String {
    let mut parts = Vec::new();
    if installed {
        parts.push("installed".to_string());
    }
    parts.extend([entry.version.clone(), entry.repository.clone()]);
    if let Some(reason) = entry.incompatibility() {
        parts.push(reason);
    }
    parts.join(" · ")
}

pub(crate) fn catalog_report_sections(
    entry: &crate::extension_catalog::CatalogEntry,
    installed: bool,
    error: Option<&str>,
) -> Vec<ReportSection> {
    let row = |label: &str, value: String, tone| ReportRow {
        label: label.to_string(),
        value,
        tone,
        kind: ReportKind::Info,
    };
    let platforms = if entry.platforms.is_empty() {
        "all".to_string()
    } else {
        entry.platforms.join(", ")
    };
    let mut sections = vec![
        ReportSection {
            title: "Overview",
            rows: [
                installed.then(|| row("Installed", "yes".to_string(), ReportTone::Success)),
                Some(row("Version", entry.version.clone(), ReportTone::Plain)),
                Some(row(
                    "Compatibility",
                    entry
                        .incompatibility()
                        .unwrap_or_else(|| "compatible".to_string()),
                    if entry.incompatibility().is_some() {
                        ReportTone::Warning
                    } else {
                        ReportTone::Success
                    },
                )),
                Some(row("Platforms", platforms, ReportTone::Plain)),
                Some(row(
                    "Description",
                    entry.description.clone(),
                    ReportTone::Muted,
                )),
            ]
            .into_iter()
            .flatten()
            .collect(),
        },
        ReportSection {
            title: "Source",
            rows: vec![
                row("Repository", entry.repository.clone(), ReportTone::Accent),
                row("Commit", entry.commit.clone(), ReportTone::Plain),
            ],
        },
        ReportSection {
            title: "Contributions",
            rows: vec![
                row("Commands", entry.commands.to_string(), ReportTone::Plain),
                row("Services", entry.services.to_string(), ReportTone::Plain),
                row("Agents", entry.agents.to_string(), ReportTone::Plain),
                row(
                    "Sidebar tabs",
                    entry.sidebar_tabs.to_string(),
                    ReportTone::Plain,
                ),
                row(
                    "Navigation targets",
                    entry.navigation_targets.to_string(),
                    ReportTone::Plain,
                ),
                row(
                    "Suggested keys",
                    entry.suggested_keybindings.to_string(),
                    ReportTone::Plain,
                ),
            ],
        },
        ReportSection {
            title: "Trust",
            rows: vec![
                row(
                    "Review",
                    "Not audited; inspect source before installing".to_string(),
                    ReportTone::Warning,
                ),
                row(
                    "Setup",
                    "External tools may require separate setup".to_string(),
                    ReportTone::Muted,
                ),
            ],
        },
    ];
    if let Some(error) = error {
        sections.push(ReportSection {
            title: "Installation failed",
            rows: vec![row("Error", error.to_string(), ReportTone::Error)],
        });
    }
    sections
}

fn installation_kind_label(
    kind: Option<&crate::extension_installation::InstallKind>,
) -> &'static str {
    match kind {
        Some(crate::extension_installation::InstallKind::Git) => "git",
        Some(crate::extension_installation::InstallKind::Local) => "copied",
        Some(crate::extension_installation::InstallKind::Link) => "linked",
        None => "manual",
    }
}

fn suggested_keybindings(
    entry: &ExtensionInfo,
    status: crate::config::ExtensionSuggestedKeybindingStatus,
) -> usize {
    entry
        .suggested_keybindings
        .iter()
        .filter(|binding| binding.status == status)
        .count()
}

/// The selected installed extension, when the Installed tab shows it.
fn selected_entry(state: &crate::state::State) -> Option<&ExtensionInfo> {
    let extensions = state.extensions.as_ref()?;
    if extensions.tab != ExtensionsTab::Installed {
        return None;
    }
    installed_groups(extensions)
        .iter()
        .any(|(_, rows)| rows.contains(&extensions.selected))
        .then(|| extensions.entries.get(extensions.selected))?
}

/// The selected discovery entry, when the Discover tab shows it.
fn selected_catalog_entry(
    state: &crate::state::State,
) -> Option<&crate::extension_catalog::CatalogEntry> {
    let extensions = state.extensions.as_ref()?;
    if extensions.tab != ExtensionsTab::Discover {
        return None;
    }
    let index = extensions.catalog_selected?;
    visible_catalog(extensions)
        .contains(&index)
        .then(|| extensions.catalog_entries.get(index))?
}

fn merged_for(state: &crate::state::State, entry: &ExtensionInfo) -> ExtensionSettings {
    state
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.merged.get(&identity(entry)))
        .cloned()
        .unwrap_or_else(|| entry.settings.clone())
}

fn identity(entry: &ExtensionInfo) -> String {
    entry.path.clone()
}

fn installation_path(entry: &ExtensionInfo) -> std::result::Result<PathBuf, String> {
    crate::platform::extensions::resolve_installation_path(
        &crate::config::extensions_dir_path(),
        &entry.path,
    )
}

fn cleanup_disabled_after_removal(ctx: &mut Context<AppRoot>, entry: &ExtensionInfo) {
    let Some(id) = entry.id.as_deref() else {
        return;
    };
    if duplicate_installation_remains(&ctx.state, entry) {
        return;
    }
    let mut user = match crate::config::read_user_extension_config() {
        Ok(user) => user,
        Err(error) => {
            notify_error(ctx, "Disabled list not updated", error);
            return;
        }
    };
    let before = user.disabled.len();
    user.disabled.retain(|candidate| candidate.trim() != id);
    if user.disabled.len() != before
        && let Err(error) = crate::config::persist_extensions_disabled(&user.disabled)
    {
        notify_error(ctx, "Disabled list not updated", error);
    }
}

fn duplicate_installation_remains(state: &crate::state::State, entry: &ExtensionInfo) -> bool {
    let Some(id) = entry.id.as_deref() else {
        return false;
    };
    state.extensions.as_ref().is_some_and(|state| {
        state
            .entries
            .iter()
            .any(|candidate| candidate.path != entry.path && candidate.id.as_deref() == Some(id))
    })
}

fn stop_before_removal(
    ctx: &mut Context<AppRoot>,
    entry: &ExtensionInfo,
) -> std::result::Result<Option<Update>, String> {
    if entry.status != ExtensionStatus::Loaded {
        return Ok(None);
    }
    let Some(id) = entry.id.as_ref() else {
        return Ok(None);
    };
    let mut user = crate::config::read_user_extension_config()?;
    if !user.disabled.iter().any(|candidate| candidate.trim() == id) {
        user.disabled.push(id.clone());
        user.disabled.sort();
        user.disabled.dedup();
        crate::config::persist_extensions_disabled(&user.disabled)?;
    }
    let update = crate::ops::config::reload_extensions_quiet(ctx);
    Ok(Some(update))
}

fn copy(ctx: &mut Context<AppRoot>, text: &str, success: &str) -> Update {
    match ctx.clipboard().copy(text) {
        Ok(()) => notify_info(ctx, success),
        Err(error) => notify_error(ctx, "Copy failed", error.to_string()),
    }
    Update::full()
}

fn notify_info(ctx: &mut Context<AppRoot>, message: &str) {
    crate::pane::pty_events::notify_info(ctx, message);
}

/// A suggested keybinding an existing binding already owns is silently dropped, so installing or
/// enabling an extension that carries one warns rather than leaving the row description to say it.
fn warn_about_key_conflicts(ctx: &mut Context<AppRoot>, id: &str) {
    let conflicts = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|extensions| {
            extensions
                .entries
                .iter()
                .find(|entry| entry.id.as_deref() == Some(id))
        })
        .map(|entry| {
            suggested_keybindings(
                entry,
                crate::config::ExtensionSuggestedKeybindingStatus::Conflict,
            )
        })
        .unwrap_or_default();
    if conflicts == 0 {
        return;
    }
    crate::pane::pty_events::notify_warning(
        ctx,
        format!("{id} keybindings not bound"),
        format!(
            "{conflicts} suggested key{} already bound",
            if conflicts == 1 { "" } else { "s" }
        ),
    );
}

fn notify_error(ctx: &mut Context<AppRoot>, title: &str, detail: impl Into<String>) {
    crate::pane::pty_events::notify_error(ctx, title, detail.into());
}

#[cfg(test)]
mod tests {
    use super::{catalog_source_url, installation_kind_label};
    use crate::extension_installation::InstallKind;

    #[test]
    fn catalog_source_links_the_indexed_commit() {
        let entry: crate::extension_catalog::CatalogEntry =
            serde_json::from_value(serde_json::json!({
                "repository": "tui-lipan/vim-rozi-navigator",
                "source": "https://github.com/tui-lipan/vim-rozi-navigator.git",
                "commit": "5b5c8b9323e260a7c10a63d792274ca155d51e26",
                "manifest_path": "extension.toml",
                "id": "vim-rozi-navigator",
                "title": "Vim and Neovim navigator",
                "description": "Split-aware navigation",
                "version": "0.2.1",
                "api": 1,
                "min_rozi": null,
                "platforms": [],
                "homepage": null,
                "stars": 0,
                "updated_at": "2026-09-21T00:00:00Z",
                "commands": 0,
                "services": 0,
                "agents": 0,
                "sidebar_tabs": 0,
                "navigation_targets": 0,
                "suggested_keybindings": 0
            }))
            .unwrap();
        assert_eq!(
            catalog_source_url(&entry),
            "https://github.com/tui-lipan/vim-rozi-navigator/tree/\
             5b5c8b9323e260a7c10a63d792274ca155d51e26"
        );
    }

    #[test]
    fn installation_kinds_have_compact_distinct_picker_labels() {
        assert_eq!(installation_kind_label(Some(&InstallKind::Git)), "git");
        assert_eq!(installation_kind_label(Some(&InstallKind::Local)), "copied");
        assert_eq!(installation_kind_label(Some(&InstallKind::Link)), "linked");
        assert_eq!(installation_kind_label(None), "manual");
    }
}
