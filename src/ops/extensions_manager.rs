use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{ExtensionInfo, ExtensionSettings, ExtensionStatus};
use crate::state::{ExtensionDetailState, ExtensionsState};

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    let (entries, merged, manifest_entries, removable_entries) = scan(ctx);
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.show_settings = false;
    if !matches!(
        ctx.state.overlay_return,
        Some(crate::state::OverlayOrigin::Settings)
    ) {
        ctx.state.settings_selected = None;
    }
    ctx.state.pane_padding_editor = None;
    ctx.state.extensions = Some(ExtensionsState {
        entries,
        merged,
        selected: 0,
        query: String::new(),
        restore_query: String::new(),
        pending_remove: None,
        detail: None,
        manifest_entries,
        removable_entries,
    });
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_extensions_focus(ctx);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.extensions = None;
    ctx.state.commands_dirty = true;
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.query = query;
    Update::none()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    let changed = state.selected != index;
    state.selected = index.min(state.entries.len().saturating_sub(1));
    if changed {
        state.pending_remove = None;
    }
    Update::full()
}

pub(crate) fn toggle_selected(ctx: &mut Context<AppRoot>) -> Update {
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

    let update = crate::ops::config::reload_extensions_quiet(ctx);
    notify_info(
        ctx,
        &format!(
            "{} {}",
            if disabled { "Disabled" } else { "Enabled" },
            entry.display_name()
        ),
    );
    update
}

pub(crate) fn reload(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::config::reload_extensions(ctx)
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
    cleanup_disabled_after_removal(ctx, &entry);

    let update = crate::ops::config::reload_extensions_quiet(ctx);
    notify_info(ctx, &format!("Removed {}", entry.display_name()));
    update
}

pub(crate) fn open_detail(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_entry(&ctx.state).cloned() else {
        return Update::none();
    };
    let path = identity(&entry);
    let merged = merged_for(&ctx.state, &entry);
    let sections = crate::config::report_sections(&entry, &merged);
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.restore_query = state.query.clone();
    state.detail = Some(ExtensionDetailState { path, sections });
    crate::ops::focus::request_extension_detail_focus(ctx);
    Update::full()
}

pub(crate) fn close_detail(ctx: &mut Context<AppRoot>) -> Update {
    let Some(state) = ctx.state.extensions.as_mut() else {
        return Update::none();
    };
    state.detail = None;
    crate::ops::focus::request_extensions_focus(ctx);
    Update::full()
}

pub(crate) fn config_reloaded(ctx: &mut Context<AppRoot>) {
    if ctx.state.extensions.is_none() {
        return;
    }
    let selected = selected_entry(&ctx.state).map(identity);
    refresh(ctx, selected);
}

fn refresh(ctx: &mut Context<AppRoot>, selected: Option<String>) {
    let detail_path = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.detail.as_ref())
        .map(|detail| detail.path.clone());
    let (entries, merged, manifest_entries, removable_entries) = scan(ctx);
    let Some(state) = ctx.state.extensions.as_mut() else {
        return;
    };
    state.entries = entries;
    state.merged = merged;
    state.manifest_entries = manifest_entries;
    state.removable_entries = removable_entries;
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
}

fn scan(
    ctx: &mut Context<AppRoot>,
) -> (
    Vec<ExtensionInfo>,
    BTreeMap<String, ExtensionSettings>,
    BTreeSet<String>,
    BTreeSet<String>,
) {
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
    (entries, merged, manifest_entries, removable_entries)
}

fn selected_entry(state: &crate::state::State) -> Option<&ExtensionInfo> {
    let extensions = state.extensions.as_ref()?;
    let entry = extensions.entries.get(extensions.selected)?;
    extension_matches_query(entry, &extensions.query).then_some(entry)
}

fn extension_matches_query(entry: &ExtensionInfo, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let description = match entry.version.as_deref() {
        Some(version) => format!("{version} · {}", entry.status_detail()),
        None => entry.status_detail(),
    };
    let items = [SearchItem::new(entry.display_name(), ())
        .description(ItemDescription::new().right(description))];
    !tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    )
    .is_empty()
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
    let duplicate_remains = ctx.state.extensions.as_ref().is_some_and(|state| {
        state
            .entries
            .iter()
            .any(|candidate| candidate.path != entry.path && candidate.id.as_deref() == Some(id))
    });
    if duplicate_remains {
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

fn notify_error(ctx: &mut Context<AppRoot>, title: &str, detail: impl Into<String>) {
    crate::pane::pty_events::notify_error(ctx, title, detail.into());
}
