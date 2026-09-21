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
    CatalogExtensionDetailState, ExtensionDetailState, ExtensionPickerRow, ExtensionUpdateCheck,
    ExtensionsState, ExtensionsTab,
};

pub(crate) const EXTENSION_UPDATING_LABEL: &str = "updating…";
/// Concurrent `git ls-remote` probes. Enough that one slow host does not stall the rest, few enough
/// not to open a burst of connections to a single forge.
const UPDATE_CHECK_WORKERS: usize = 4;
const SHORT_REVISION_LEN: usize = 7;

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
    let catalog_epoch = next_catalog_epoch();
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
        update_checks: BTreeMap::new(),
        update_check_epoch: 0,
        updating_id: None,
        manifest_entries: scan.manifest_entries,
        removable_entries: scan.removable_entries,
    });
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_extensions_focus(ctx);
    start_update_check(ctx);
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
    state.install_prompt = None;
    crate::ops::focus::request_extensions_focus(ctx);
    Update::full()
}

pub(crate) fn submit_install(ctx: &mut Context<AppRoot>) -> Update {
    let busy = ctx.state.extension_install.is_some();
    let Some(prompt) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.install_prompt.as_mut())
    else {
        return Update::none();
    };
    let source = prompt.input.text().trim().to_string();
    let rejection = if busy {
        Some("Another extension is still installing")
    } else if source.is_empty() {
        Some("Enter a local path or Git URL")
    } else {
        None
    };
    prompt.error = rejection.map(str::to_string);
    prompt.error_scroll_offset = 0;
    prompt.error_scroll_max = None;
    if rejection.is_some() {
        return Update::full();
    }
    ctx.state.extension_install = Some(crate::state::ExtensionInstall {
        repository: None,
        label: source.clone(),
        detail: None,
        hidden: false,
    });
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

/// Finishes an installation from the install prompt whatever became of the UI that started it.
pub(crate) fn install_finished(
    ctx: &mut Context<AppRoot>,
    result: std::result::Result<String, String>,
) -> Update {
    let Some(install) = ctx
        .state
        .extension_install
        .take_if(|install| install.repository.is_none())
    else {
        return Update::none();
    };
    let waiting = !install.hidden
        && ctx
            .state
            .extensions
            .as_ref()
            .is_some_and(|state| state.install_prompt.is_some());
    finish_install(ctx, waiting, result, |ctx, error| {
        let Some(prompt) = ctx
            .state
            .extensions
            .as_mut()
            .and_then(|state| state.install_prompt.as_mut())
        else {
            return;
        };
        prompt.error = Some(error);
        prompt.error_scroll_offset = 0;
        prompt.error_scroll_max = None;
        crate::ops::focus::request_extension_install_focus(ctx);
    })
}

/// The presentation shared by both installation paths. The installation is already on disk, so a
/// success always reloads extensions, even when the manager closed meanwhile. A user still
/// `waiting` on the progress modal is taken to the new row, or shown the failure where they
/// started; anyone who moved on is told in a toast.
fn finish_install(
    ctx: &mut Context<AppRoot>,
    waiting: bool,
    result: std::result::Result<String, String>,
    show_error: impl FnOnce(&mut Context<AppRoot>, String),
) -> Update {
    match result {
        Ok(id) => {
            if waiting && let Some(state) = ctx.state.extensions.as_mut() {
                state.install_prompt = None;
                state.catalog_detail = None;
                crate::ops::focus::request_extensions_focus(ctx);
            }
            let update = crate::ops::config::reload_extensions_quiet(ctx);
            if ctx.state.extensions.is_some() {
                if waiting {
                    show_installed(ctx, &id);
                }
                warn_about_key_conflicts(ctx, &id);
            }
            if !waiting {
                // The new row alone may go unseen by someone who moved on.
                notify_info(ctx, &format!("Installed {id}"));
            }
            update
        }
        Err(error) if waiting => {
            show_error(ctx, error);
            Update::full()
        }
        Err(error) => {
            notify_error(ctx, "Extension not installed", error);
            Update::full()
        }
    }
}

/// The installation whose progress modal is showing: not hidden, and its report or prompt still
/// open in the manager.
pub(crate) fn visible_install(
    state: &crate::state::State,
) -> Option<&crate::state::ExtensionInstall> {
    let install = state
        .extension_install
        .as_ref()
        .filter(|install| !install.hidden)?;
    let extensions = state.extensions.as_ref()?;
    let origin_open = match install.repository.as_deref() {
        Some(repository) => extensions
            .catalog_detail
            .as_ref()
            .is_some_and(|detail| detail.entry.repository == repository),
        None => extensions.install_prompt.is_some(),
    };
    origin_open.then_some(install)
}

/// Hides the progress modal and the dialog under it. The installation continues.
pub(crate) fn hide_install(ctx: &mut Context<AppRoot>) -> Update {
    let Some(install) = ctx.state.extension_install.as_mut() else {
        return Update::none();
    };
    install.hidden = true;
    let repository = install.repository.clone();
    if let Some(state) = ctx.state.extensions.as_mut() {
        match repository {
            Some(repository) => {
                if state
                    .catalog_detail
                    .as_ref()
                    .is_some_and(|detail| detail.entry.repository == repository)
                {
                    state.catalog_detail = None;
                }
            }
            None => state.install_prompt = None,
        }
        crate::ops::focus::request_extensions_focus(ctx);
    }
    Update::full()
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
    if ctx.state.extension_install.is_some() {
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
    ctx.state.extension_install = Some(crate::state::ExtensionInstall {
        repository: Some(repository.clone()),
        label: entry.title.clone(),
        detail: Some(format!("{repository} · {}", &entry.commit[..12])),
        hidden: false,
    });
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
    let Some(install) = ctx
        .state
        .extension_install
        .take_if(|install| install.repository.as_deref() == Some(repository.as_str()))
    else {
        return Update::none();
    };
    let waiting = !install.hidden
        && ctx.state.extensions.as_ref().is_some_and(|state| {
            state
                .catalog_detail
                .as_ref()
                .is_some_and(|detail| detail.entry.repository == repository)
        });
    finish_install(ctx, waiting, result, |ctx, error| {
        if let Some(detail) = ctx
            .state
            .extensions
            .as_mut()
            .and_then(|state| state.catalog_detail.as_mut())
        {
            detail.error = Some(error);
        }
    })
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
    match state.update_checks.get(&id) {
        Some(ExtensionUpdateCheck::Available { .. }) => {}
        Some(ExtensionUpdateCheck::Checking) => return Update::none(),
        // Without a known update the same key asks again, so a failed or stale check is one
        // keypress from an answer instead of a full clone that may change nothing.
        _ => {
            let epoch = state.update_check_epoch;
            if request_update_checks(ctx, epoch, vec![id.clone()])
                && let Some(state) = ctx.state.extensions.as_mut()
            {
                state
                    .update_checks
                    .insert(id, ExtensionUpdateCheck::Checking);
            }
            refresh_detail_report(ctx);
            return Update::full();
        }
    }
    state.updating_id = Some(id.clone());
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
    match result {
        Ok(true) => {
            state.update_checks.remove(&id);
            let update = crate::ops::config::reload_extensions_quiet(ctx);
            select_by_id(ctx, &id);
            update
        }
        Ok(false) => {
            state
                .update_checks
                .insert(id.clone(), ExtensionUpdateCheck::Current);
            refresh_detail_report(ctx);
            notify_info(ctx, &format!("{id} is up to date"));
            Update::full()
        }
        Err(error) => {
            notify_error(ctx, "Extension not updated", error);
            Update::full()
        }
    }
}

pub(crate) fn update_checked(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    id: String,
    check: ExtensionUpdateCheck,
) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    // A running update owns the row, and its reload starts a fresh check.
    if state.update_check_epoch != epoch
        || state.updating_id.as_deref() == Some(id.as_str())
        || !state.update_checks.contains_key(&id)
    {
        return Update::none();
    }
    state.update_checks.insert(id, check);
    refresh_detail_report(ctx);
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
        let Some(state) = ctx.state.extensions.as_ref() else {
            return Update::none();
        };
        installed_report_sections(state, entry, &merged)
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
        if let Some(install) = ctx.state.extension_install.as_mut()
            && install.repository.as_deref() == Some(entry.repository.as_str())
        {
            install.hidden = false;
        }
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
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    let sections = installed_report_sections(state, &entry, &merged);
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
    state.update_checks.retain(|id, _| {
        state.installation_kinds.get(id) == Some(&crate::extension_installation::InstallKind::Git)
    });
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
            sections: installed_report_sections(state, entry, &merged),
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

/// Rechecks every Git installation, replacing whatever an earlier check found.
fn start_update_check(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    let epoch = next_update_check_epoch();
    state.update_check_epoch = epoch;
    let ids = git_installation_ids(&state.installation_kinds);
    let checking = request_update_checks(ctx, epoch, ids.clone());
    if let Some(state) = ctx.state.extensions.as_mut() {
        state.update_checks = if checking {
            ids.into_iter()
                .map(|id| (id, ExtensionUpdateCheck::Checking))
                .collect()
        } else {
            BTreeMap::new()
        };
    }
}

/// Checks `ids` on a few worker threads, each answer sent as soon as it is known, so one slow
/// remote holds back only its own row. Reports whether any check started.
fn request_update_checks(ctx: &Context<AppRoot>, epoch: u64, ids: Vec<String>) -> bool {
    if ids.is_empty() {
        return false;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return false;
    };
    let workers = ids.len().min(UPDATE_CHECK_WORKERS);
    let queue = std::sync::Arc::new(std::sync::Mutex::new(ids.into_iter()));
    for _ in 0..workers {
        let queue = queue.clone();
        let link = link.clone();
        std::thread::spawn(move || {
            // The guard is consumed by the closure, so the lock is released before each check.
            while let Some(id) = queue.lock().ok().and_then(|mut queue| queue.next()) {
                let check = update_check(crate::extension_installation::check_update(&id));
                link.send(crate::Msg::ExtensionUpdateChecked { epoch, id, check });
            }
        });
    }
    true
}

fn update_check(
    result: std::result::Result<crate::extension_installation::UpdateCheck, String>,
) -> ExtensionUpdateCheck {
    match result {
        Ok(crate::extension_installation::UpdateCheck::Current) => ExtensionUpdateCheck::Current,
        Ok(crate::extension_installation::UpdateCheck::Available { revision, version }) => {
            ExtensionUpdateCheck::Available { revision, version }
        }
        Err(error) => ExtensionUpdateCheck::Failed(error),
    }
}

/// Rebuilds an open installed report, whose update row follows the background check.
fn refresh_detail_report(ctx: &mut Context<AppRoot>) {
    let Some(state) = ctx.state.extensions.as_ref() else {
        return;
    };
    let Some(path) = state.detail.as_ref().map(|detail| detail.path.clone()) else {
        return;
    };
    let Some(entry) = state.entries.iter().find(|entry| identity(entry) == path) else {
        return;
    };
    let merged = state
        .merged
        .get(&path)
        .cloned()
        .unwrap_or_else(|| entry.settings.clone());
    let sections = installed_report_sections(state, entry, &merged);
    if let Some(detail) = ctx
        .state
        .extensions
        .as_mut()
        .and_then(|state| state.detail.as_mut())
    {
        detail.sections = sections;
    }
}

/// The shared extension report, plus the manager's own knowledge of available updates.
fn installed_report_sections(
    state: &ExtensionsState,
    entry: &ExtensionInfo,
    merged: &ExtensionSettings,
) -> Vec<ReportSection> {
    let mut sections = crate::config::report_sections(entry, merged);
    let check = entry
        .id
        .as_deref()
        .and_then(|id| state.update_checks.get(id));
    let row = match check {
        None => None,
        Some(ExtensionUpdateCheck::Checking) => {
            Some(("Update", "checking…".to_string(), ReportTone::Muted))
        }
        Some(ExtensionUpdateCheck::Current) => {
            Some(("Update", "up to date".to_string(), ReportTone::Muted))
        }
        Some(ExtensionUpdateCheck::Available { revision, version }) => Some((
            "Update",
            match version {
                Some(version) => format!("{version} · {}", short_revision(revision)),
                None => short_revision(revision).to_string(),
            },
            ReportTone::Accent,
        )),
        Some(ExtensionUpdateCheck::Failed(error)) => {
            Some(("Update check", error.clone(), ReportTone::Warning))
        }
    };
    if let Some((label, value, tone)) = row
        && let Some(overview) = sections.first_mut()
    {
        let at = overview
            .rows
            .iter()
            .position(|row| row.label == "Version")
            .map_or(overview.rows.len().min(1), |index| index + 1);
        overview.rows.insert(
            at,
            ReportRow {
                label: label.to_string(),
                value,
                tone,
                kind: ReportKind::Info,
            },
        );
    }
    sections
}

fn short_revision(revision: &str) -> &str {
    &revision[..revision.len().min(SHORT_REVISION_LEN)]
}

/// The Installed tab's label, which counts known updates, or says a check is still running
/// until one is known.
pub(crate) fn installed_tab_label(state: &ExtensionsState) -> String {
    let label = ExtensionsTab::Installed.label();
    let checks = state.update_checks.values();
    let updates = checks
        .clone()
        .filter(|check| matches!(check, ExtensionUpdateCheck::Available { .. }))
        .count();
    if updates > 0 {
        format!(
            "{label} · {updates} update{}",
            if updates == 1 { "" } else { "s" }
        )
    } else if checks
        .into_iter()
        .any(|check| *check == ExtensionUpdateCheck::Checking)
    {
        format!("{label} · checking…")
    } else {
        label.to_string()
    }
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
    parts.extend(version_label(
        entry.version.as_deref(),
        id.and_then(|id| state.update_checks.get(id)),
    ));
    parts.push(
        installation_kind_label(id.and_then(|id| state.installation_kinds.get(id))).to_string(),
    );
    if !matches!(
        entry.status,
        ExtensionStatus::Loaded | ExtensionStatus::Disabled
    ) {
        parts.push(entry.status_detail());
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

/// The installed version, or `installed → latest` once a check found an update.
fn version_label(installed: Option<&str>, check: Option<&ExtensionUpdateCheck>) -> Option<String> {
    let Some(ExtensionUpdateCheck::Available { revision, version }) = check else {
        return installed.map(str::to_string);
    };
    // A remote that moved without bumping its version is named by commit instead.
    let latest = version
        .as_deref()
        .filter(|version| Some(*version) != installed)
        .unwrap_or_else(|| short_revision(revision));
    Some(match installed {
        Some(installed) => format!("{installed} → {latest}"),
        None => format!("→ {latest}"),
    })
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
    use super::{catalog_source_url, installation_kind_label, version_label};
    use crate::extension_installation::InstallKind;
    use crate::state::ExtensionUpdateCheck;

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
    fn version_label_names_an_update_by_version_or_commit() {
        let available = |version: Option<&str>| ExtensionUpdateCheck::Available {
            revision: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            version: version.map(str::to_string),
        };
        assert_eq!(version_label(Some("0.2.1"), None).as_deref(), Some("0.2.1"));
        assert_eq!(
            version_label(Some("0.2.1"), Some(&ExtensionUpdateCheck::Current)).as_deref(),
            Some("0.2.1")
        );
        assert_eq!(
            version_label(
                Some("0.2.1"),
                Some(&ExtensionUpdateCheck::Failed("offline".into()))
            )
            .as_deref(),
            Some("0.2.1")
        );
        assert_eq!(
            version_label(Some("0.2.1"), Some(&available(Some("0.2.2")))).as_deref(),
            Some("0.2.1 → 0.2.2")
        );
        assert_eq!(
            version_label(Some("0.2.1"), Some(&available(Some("0.2.1")))).as_deref(),
            Some("0.2.1 → 89abcde")
        );
        assert_eq!(
            version_label(Some("0.2.1"), Some(&available(None))).as_deref(),
            Some("0.2.1 → 89abcde")
        );
        assert_eq!(
            version_label(None, Some(&available(Some("0.2.2")))).as_deref(),
            Some("→ 0.2.2")
        );
        assert_eq!(version_label(None, None), None);
    }

    #[test]
    fn installation_kinds_have_compact_distinct_picker_labels() {
        assert_eq!(installation_kind_label(Some(&InstallKind::Git)), "git");
        assert_eq!(installation_kind_label(Some(&InstallKind::Local)), "copied");
        assert_eq!(installation_kind_label(Some(&InstallKind::Link)), "linked");
        assert_eq!(installation_kind_label(None), "manual");
    }
}
