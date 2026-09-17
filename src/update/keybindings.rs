//! Keybinding editor transactions.
//!
//! Every change follows one path: build a [`KeymapEdit`] against the source config, load the
//! candidate document through the same pipeline a reload uses, reject new hard collisions, then
//! persist the source and reload. Nothing here edits resolved chords and infers intent back.

#[cfg(test)]
use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::config::{
    BindingExpr, Config, KeyOverrideSpec, KeymapCollision, KeymapEdit, KeymapOwner, WmModifier,
};
use crate::state::{
    HelpTab, KeybindingConflict, KeybindingEditorStage, KeybindingTarget, LiteralConversion,
    ModifierChoice,
};
use crate::{AppRoot, Msg};

pub(super) fn keybinding_select(ctx: &mut Context<AppRoot>, id: String) -> Update {
    let Some(keybindings) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    keybindings.selected = Some(id);
    // The list's `selected` prop is controlled, so the move only paints on a re-render.
    Update::full()
}

pub(super) fn keybinding_capture(ctx: &mut Context<AppRoot>, id: String) -> Update {
    if let Some(keybindings) = ctx.state.keybindings.as_mut() {
        keybindings.selected = Some(id.clone());
    }
    start_capture(ctx, KeybindingTarget::Action(id))
}

pub(super) fn keybinding_capture_prefix(ctx: &mut Context<AppRoot>) -> Update {
    start_capture(ctx, KeybindingTarget::Prefix)
}

fn start_capture(ctx: &mut Context<AppRoot>, target: KeybindingTarget) -> Update {
    let Some(keybindings) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    keybindings.stage = KeybindingEditorStage::Capture { target };
    keybindings.held_modifiers = KeyMods::NONE;
    ctx.request_focus(crate::view::keybinding_capture_key());
    Update::full()
}

pub(super) fn keybinding_cancel_capture(ctx: &mut Context<AppRoot>) -> Update {
    let Some(keybindings) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    keybindings.stage = KeybindingEditorStage::List;
    keybindings.held_modifiers = KeyMods::NONE;
    ctx.request_focus(crate::view::help_filter_key());
    Update::full()
}

/// The binding a recorded key press stands for.
///
/// Modifiers come only from what the terminal reported. A character's case is never read as
/// Shift: Caps Lock produces uppercase without it, and legacy terminal encodings cannot tell
/// `Ctrl+A` from `Ctrl+Shift+A` at all. Where the terminal cannot report Shift, the recorded
/// binding simply has none, so a displayed `Shift` always reflects a modifier Rozi observed.
fn captured_binding(key: KeyEvent) -> KeyBinding {
    KeyBinding::from_key_event(key)
}

pub(super) fn keybinding_captured(ctx: &mut Context<AppRoot>, key: KeyEvent) -> Update {
    let Some(KeybindingEditorStage::Capture { target }) = current_stage(ctx) else {
        return Update::none();
    };
    let binding = captured_binding(key);

    // A complete binding equal to the prefix occupies the prefix itself. Every scheme command's
    // prefix-half starts with that key, so a chord-level check would list them all. The conflict
    // is Prefix; those commands follow it.
    if matches!(target, KeybindingTarget::Action(_)) && binding == ctx.state.config.input.prefix {
        if let Some(editor) = ctx.state.keybindings.as_mut() {
            editor.stage = KeybindingEditorStage::Conflict {
                target,
                binding,
                conflicts: vec![KeybindingConflict::prefix()],
                replaceable: false,
            };
            editor.held_modifiers = KeyMods::NONE;
        }
        ctx.request_focus(crate::view::keybinding_capture_key());
        return Update::full();
    }

    let (edit, owner) = match &target {
        KeybindingTarget::Action(id) => (
            action_edit(id, &binding),
            Some(KeymapOwner::Override(id.clone())),
        ),
        KeybindingTarget::Prefix => (
            KeymapEdit {
                prefix: Some(binding.clone()),
                ..KeymapEdit::default()
            },
            None,
        ),
    };
    let collisions = match candidate_collisions(&edit) {
        Ok(collisions) => collisions,
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Keybinding not checked", error);
            return Update::full();
        }
    };

    let stage = if collisions.is_empty() {
        let conversion = match target {
            KeybindingTarget::Prefix if binding != ctx.state.config.input.prefix => {
                let input = ctx.state.config.input.clone();
                offered_conversion(crate::config::convertible_literal_count(
                    &ctx.state.config,
                    |expr| expr.prefix_equivalent(&input),
                ))
            }
            _ => None,
        };
        KeybindingEditorStage::Review {
            target,
            binding,
            conversion,
        }
    } else {
        let conflicts = conflict_rows(ctx, &collisions, owner.as_ref());
        let replaceable = owner.is_some() && conflicts.iter().all(|row| row.config_id.is_some());
        KeybindingEditorStage::Conflict {
            target,
            binding,
            conflicts,
            replaceable,
        }
    };
    if let Some(editor) = ctx.state.keybindings.as_mut() {
        editor.stage = stage;
        editor.held_modifiers = KeyMods::NONE;
    }
    ctx.request_focus(crate::view::keybinding_capture_key());
    Update::full()
}

/// A recorded binding saved the ordinary way: a bare command key follows the scheme, a modified
/// chord stays literal. `prefix:` and `mod:` are never guessed from what the chord contains.
fn action_edit(id: &str, binding: &KeyBinding) -> KeymapEdit {
    KeymapEdit {
        overrides: vec![(
            id.to_string(),
            Some(KeyOverrideSpec::replace(vec![BindingExpr::from_binding(
                binding.clone(),
            )])),
        )],
        ..KeymapEdit::default()
    }
}

pub(super) fn keybinding_save_captured(ctx: &mut Context<AppRoot>) -> Update {
    let Some(KeybindingEditorStage::Review {
        target,
        binding,
        conversion,
    }) = current_stage(ctx)
    else {
        return Update::none();
    };
    match target {
        KeybindingTarget::Action(id) => persist_edit(ctx, &action_edit(&id, &binding)),
        KeybindingTarget::Prefix => {
            if binding == ctx.state.config.input.prefix {
                return keybinding_cancel_capture(ctx);
            }
            let input = ctx.state.config.input.clone();
            let mut edit = KeymapEdit {
                prefix: Some(binding.clone()),
                ..KeymapEdit::default()
            };
            if conversion.is_some_and(|conversion| conversion.enabled) {
                edit.overrides =
                    converted(&ctx.state.config, |expr| expr.prefix_equivalent(&input));
            }
            // Converting literals can itself collide, so the final edit is checked again.
            match candidate_collisions(&edit) {
                Ok(collisions) if collisions.is_empty() => persist_edit(ctx, &edit),
                Ok(collisions) => {
                    let conflicts = conflict_rows(ctx, &collisions, None);
                    set_stage(
                        ctx,
                        KeybindingEditorStage::Conflict {
                            target: KeybindingTarget::Prefix,
                            binding,
                            conflicts,
                            replaceable: false,
                        },
                    )
                }
                Err(error) => {
                    crate::pane::pty_events::notify_error(ctx, "Prefix not saved", error);
                    Update::full()
                }
            }
        }
    }
}

pub(super) fn keybinding_toggle_conversion(ctx: &mut Context<AppRoot>) -> Update {
    let Some(editor) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    match &mut editor.stage {
        KeybindingEditorStage::Review {
            conversion: Some(conversion),
            ..
        }
        | KeybindingEditorStage::Modifier {
            conversion: Some(conversion),
            ..
        } => {
            conversion.enabled = !conversion.enabled;
            Update::full()
        }
        _ => Update::none(),
    }
}

pub(super) fn keybinding_unbind(ctx: &mut Context<AppRoot>, id: String) -> Update {
    // Global hides empty-key rows, so keeping `id` would miss in the list and jump to Prefix.
    let selected = selection_after_unbind(ctx, &id);
    if let Some(editor) = ctx.state.keybindings.as_mut() {
        editor.selected = selected;
    }
    persist_edit(
        ctx,
        &KeymapEdit {
            overrides: vec![(id, Some(KeyOverrideSpec::replace(Vec::new())))],
            ..KeymapEdit::default()
        },
    )
}

/// After unbind, stay on the same row when it remains visible (All / Unbound); otherwise highlight
/// the next remaining row so the list does not scroll to the top.
fn selection_after_unbind(ctx: &Context<AppRoot>, id: &str) -> Option<String> {
    let tab = ctx.state.keybindings.as_ref()?.tab;
    match tab {
        HelpTab::All | HelpTab::Unbound => Some(id.to_string()),
        HelpTab::Global | HelpTab::Modes => crate::view::neighbor_keybinding_id(ctx, id),
    }
}

pub(super) fn keybinding_reset(ctx: &mut Context<AppRoot>, id: String) -> Update {
    if let Some(editor) = ctx.state.keybindings.as_mut() {
        editor.selected = Some(id.clone());
    }
    if !ctx.state.config.key_sources.contains_key(&id) {
        return Update::none();
    }
    validated_persist(
        ctx,
        KeymapEdit {
            overrides: vec![(id, None)],
            ..KeymapEdit::default()
        },
        "Keybinding not reset",
    )
}

pub(super) fn keybinding_reset_all(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.config.key_sources.is_empty() {
        return Update::none();
    }
    let Some(editor) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    editor.stage = KeybindingEditorStage::ResetAll;
    ctx.request_focus(crate::view::dialog_answer_key(crate::view::DIALOG_AFFIRM));
    Update::full()
}

pub(super) fn keybinding_focus_answer(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    ctx.request_focus(crate::view::dialog_answer_key(index));
    Update::full()
}

pub(super) fn keybinding_confirm_reset_all(ctx: &mut Context<AppRoot>, reset: bool) -> Update {
    if !reset {
        return keybinding_cancel_capture(ctx);
    }
    let mut ids = ctx
        .state
        .config
        .key_sources
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    ids.sort();
    validated_persist(
        ctx,
        KeymapEdit {
            overrides: ids.into_iter().map(|id| (id, None)).collect(),
            ..KeymapEdit::default()
        },
        "Keybindings not reset",
    )
}

/// Take the recorded binding and give every conflicting command what it has left.
///
/// Losing part of a scheme binding keeps the rest scheme-relative (`enter` minus its Mod chord is
/// `prefix:enter`), so nothing is frozen as a physical chord that would stop following the prefix.
pub(super) fn keybinding_resolve_conflict(ctx: &mut Context<AppRoot>, replace: bool) -> Update {
    let Some(KeybindingEditorStage::Conflict {
        target: KeybindingTarget::Action(id),
        binding,
        conflicts,
        replaceable,
    }) = current_stage(ctx)
    else {
        return if replace {
            Update::none()
        } else {
            keybinding_retry_capture(ctx)
        };
    };
    if !replace {
        return keybinding_retry_capture(ctx);
    }
    if !replaceable {
        return Update::none();
    }

    let config = &ctx.state.config;
    let expr = BindingExpr::from_binding(binding.clone());
    let taken = expr.resolve(&config.input);
    let mut owners = conflicts
        .iter()
        .filter_map(|conflict| conflict.config_id.clone())
        .filter(|owner| *owner != id)
        .collect::<Vec<_>>();
    owners.sort();
    owners.dedup();
    let mut edit = KeymapEdit::default();
    for owner in owners {
        let remaining = crate::config::effective_expressions(config, &owner)
            .iter()
            .filter_map(|expr| expr.without(&config.input, &taken))
            .collect();
        edit.overrides
            .push((owner, Some(KeyOverrideSpec::replace(remaining))));
    }
    edit.overrides
        .push((id, Some(KeyOverrideSpec::replace(vec![expr]))));
    validated_persist(ctx, edit, "Keybinding not replaced")
}

pub(super) fn keybinding_retry_capture(ctx: &mut Context<AppRoot>) -> Update {
    let target = match ctx.state.keybindings.as_ref().map(|editor| &editor.stage) {
        Some(KeybindingEditorStage::Review { target, .. })
        | Some(KeybindingEditorStage::Conflict { target, .. }) => target.clone(),
        _ => return Update::none(),
    };
    start_capture(ctx, target)
}

pub(super) fn keybinding_edit_modifier(ctx: &mut Context<AppRoot>) -> Update {
    let choice = ModifierChoice::from_input(&ctx.state.config.input);
    ctx.request_focus(crate::view::keybinding_capture_key());
    set_stage(
        ctx,
        KeybindingEditorStage::Modifier {
            choice,
            conversion: None,
            conflicts: Vec::new(),
        },
    )
}

pub(super) fn keybinding_step_modifier(ctx: &mut Context<AppRoot>, steps: isize) -> Update {
    let Some(KeybindingEditorStage::Modifier { choice, .. }) = current_stage(ctx) else {
        return Update::none();
    };
    let choice = choice.stepped(steps);
    let conversion = modifier_conversion(&ctx.state.config, choice);
    set_stage(
        ctx,
        KeybindingEditorStage::Modifier {
            choice,
            conversion,
            conflicts: Vec::new(),
        },
    )
}

pub(super) fn keybinding_save_modifier(ctx: &mut Context<AppRoot>) -> Update {
    let Some(KeybindingEditorStage::Modifier {
        choice, conversion, ..
    }) = current_stage(ctx)
    else {
        return Update::none();
    };
    let input = ctx.state.config.input.clone();
    if choice == ModifierChoice::from_input(&input) {
        return keybinding_cancel_capture(ctx);
    }
    let mut edit = match choice {
        ModifierChoice::Off => KeymapEdit {
            modifier_shortcuts: Some(false),
            ..KeymapEdit::default()
        },
        ModifierChoice::Alt | ModifierChoice::Super => KeymapEdit {
            modifier: Some(if choice == ModifierChoice::Alt {
                WmModifier::Alt
            } else {
                WmModifier::Super
            }),
            modifier_shortcuts: Some(true),
            ..KeymapEdit::default()
        },
    };
    if conversion.is_some_and(|conversion| conversion.enabled) {
        edit.overrides = converted(&ctx.state.config, |expr| expr.mod_equivalent(&input));
    }
    match candidate_collisions(&edit) {
        Ok(collisions) if collisions.is_empty() => persist_edit(ctx, &edit),
        Ok(collisions) => {
            let conflicts = conflict_rows(ctx, &collisions, None);
            set_stage(
                ctx,
                KeybindingEditorStage::Modifier {
                    choice,
                    conversion,
                    conflicts,
                },
            )
        }
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Modifier not saved", error);
            Update::full()
        }
    }
}

/// Literals that already spell the current Mod can follow a new one. Turning the layer off, or
/// keeping the same modifier, has nothing to convert.
fn modifier_conversion(config: &Config, choice: ModifierChoice) -> Option<LiteralConversion> {
    let input = &config.input;
    let modifier = match choice {
        ModifierChoice::Alt => WmModifier::Alt,
        ModifierChoice::Super => WmModifier::Super,
        ModifierChoice::Off => return None,
    };
    if !input.modifier_shortcuts || modifier == input.modifier {
        return None;
    }
    offered_conversion(crate::config::convertible_literal_count(config, |expr| {
        expr.mod_equivalent(input)
    }))
}

fn offered_conversion(count: usize) -> Option<LiteralConversion> {
    (count > 0).then_some(LiteralConversion {
        count,
        enabled: false,
    })
}

fn converted(
    config: &Config,
    equivalent: impl Fn(&BindingExpr) -> Option<BindingExpr>,
) -> Vec<(String, Option<KeyOverrideSpec>)> {
    crate::config::converted_literals(config, equivalent)
        .into_iter()
        .map(|(id, spec)| (id, Some(spec)))
        .collect()
}

/// Hard collisions `edit` would introduce, judged against the current document through the same
/// load pipeline a reload runs. Soft extension claims resolve away inside that load.
fn candidate_collisions(edit: &KeymapEdit) -> std::result::Result<Vec<KeymapCollision>, String> {
    let text = crate::config::read_config_text()?;
    let baseline = crate::config::load_config_candidate(&text).config;
    let candidate =
        crate::config::load_config_candidate(&crate::config::apply_keymap_edit(&text, edit)).config;
    Ok(crate::config::new_collisions(&baseline, &candidate))
}

fn validated_persist(ctx: &mut Context<AppRoot>, edit: KeymapEdit, failure: &str) -> Update {
    match candidate_collisions(&edit) {
        Ok(collisions) if collisions.is_empty() => persist_edit(ctx, &edit),
        Ok(collisions) => {
            let labels = conflict_rows(ctx, &collisions, None)
                .into_iter()
                .map(|row| row.label)
                .collect::<Vec<_>>()
                .join(" · ");
            crate::pane::pty_events::notify_error(ctx, failure, format!("Collides: {labels}"));
            Update::full()
        }
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, failure, error);
            Update::full()
        }
    }
}

fn persist_edit(ctx: &mut Context<AppRoot>, edit: &KeymapEdit) -> Update {
    let selected = ctx
        .state
        .keybindings
        .as_ref()
        .and_then(|editor| editor.selected.clone());
    if let Err(error) = crate::config::persist_keymap_edit(edit) {
        crate::pane::pty_events::notify_error(ctx, "Keybinding not saved", error);
        return keybinding_cancel_capture(ctx);
    }
    let update = crate::ops::config::reload_config_quiet(ctx);
    if let Some(editor) = ctx.state.keybindings.as_mut() {
        editor.selected = selected;
        editor.stage = KeybindingEditorStage::List;
    }
    ctx.request_focus(crate::view::help_filter_key());
    ctx.state.commands_dirty = true;
    update
}

fn current_stage(ctx: &Context<AppRoot>) -> Option<KeybindingEditorStage> {
    ctx.state
        .keybindings
        .as_ref()
        .map(|editor| editor.stage.clone())
}

fn set_stage(ctx: &mut Context<AppRoot>, stage: KeybindingEditorStage) -> Update {
    if let Some(editor) = ctx.state.keybindings.as_mut() {
        editor.stage = stage;
        editor.held_modifiers = KeyMods::NONE;
    }
    Update::full()
}

/// Collisions as rows the capture card lists. With `target`, each row names the command the
/// target would collide with and whether the editor can rewrite it; without one (a Prefix or Mod
/// change), each row names both sides.
fn conflict_rows(
    ctx: &Context<AppRoot>,
    collisions: &[KeymapCollision],
    target: Option<&KeymapOwner>,
) -> Vec<KeybindingConflict> {
    let config = &ctx.state.config;
    let mut rows: Vec<KeybindingConflict> = Vec::new();
    for collision in collisions {
        let row = match target {
            Some(target)
                if collision.first.owner == *target || collision.second.owner == *target =>
            {
                let other = if collision.first.owner == *target {
                    &collision.second.owner
                } else {
                    &collision.first.owner
                };
                KeybindingConflict {
                    registry_id: other.registry_id(config),
                    config_id: other.config_id(config),
                    label: owner_label(ctx, other),
                }
            }
            _ => KeybindingConflict {
                registry_id: collision.first.owner.registry_id(config),
                config_id: None,
                label: format!(
                    "{} ↔ {}",
                    owner_label(ctx, &collision.first.owner),
                    owner_label(ctx, &collision.second.owner)
                ),
            },
        };
        if !rows.iter().any(|seen| seen.label == row.label) {
            rows.push(row);
        }
    }
    rows
}

fn owner_label(ctx: &Context<AppRoot>, owner: &KeymapOwner) -> String {
    let registry_id = owner.registry_id(&ctx.state.config);
    if let Some(label) = ctx
        .command_registry()
        .entries()
        .into_iter()
        .find(|entry| entry.id.as_str() == registry_id)
        .map(|entry| entry.label.to_string())
        .filter(|label| !label.is_empty())
    {
        return label;
    }
    if registry_id == crate::commands::FORWARD_PREFIX_COMMAND_ID {
        return "Send prefix to pane".to_string();
    }
    let workspace = |kind: &str| {
        registry_id
            .strip_prefix(&format!("workspace.{kind}."))
            .map(str::to_string)
    };
    if let Some(index) = workspace("switch") {
        return format!("Switch to workspace {index}");
    }
    if let Some(index) = workspace("move") {
        return format!("Move pane to workspace {index}");
    }
    if let Some(index) = workspace("relocate") {
        return format!("Move workspace to workspace {index}");
    }
    registry_id
}

/// The message a capture card sends for a key, by stage. Only a listening card records: once a
/// candidate is on screen the card answers to its own keys and ignores the rest, so a stray key
/// cannot silently replace what is being reviewed. Esc returns to listening.
pub(crate) fn capture_key_msg(stage: &KeybindingEditorStage, key: KeyEvent) -> Option<Msg> {
    let plain = key.mods == KeyMods::NONE;
    Some(match stage {
        KeybindingEditorStage::Capture { .. } if key.is(KeyCode::Esc) => {
            Msg::KeybindingCancelCapture
        }
        KeybindingEditorStage::Capture { .. } => Msg::KeybindingCaptured(key),
        KeybindingEditorStage::Review { conversion, .. } => match key.code {
            KeyCode::Esc => Msg::KeybindingRetryCapture,
            KeyCode::Enter if plain => Msg::KeybindingSaveCaptured,
            KeyCode::Tab if plain && conversion.is_some() => Msg::KeybindingToggleConversion,
            _ => return None,
        },
        KeybindingEditorStage::Conflict { replaceable, .. } => match key.code {
            KeyCode::Esc => Msg::KeybindingRetryCapture,
            KeyCode::Enter if plain && *replaceable => Msg::KeybindingResolveConflict(true),
            _ => return None,
        },
        KeybindingEditorStage::Modifier { conversion, .. } => match key.code {
            KeyCode::Esc => Msg::KeybindingCancelCapture,
            KeyCode::Enter if plain => Msg::KeybindingSaveModifier,
            KeyCode::Left if plain => Msg::KeybindingStepModifier(-1),
            KeyCode::Right if plain => Msg::KeybindingStepModifier(1),
            KeyCode::Tab if plain && conversion.is_some() => Msg::KeybindingToggleConversion,
            _ => return None,
        },
        KeybindingEditorStage::List | KeybindingEditorStage::ResetAll => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(code: KeyCode, mods: KeyMods) -> String {
        captured_binding(KeyEvent { code, mods }).canonical_lowercase()
    }

    #[test]
    fn captured_bindings_take_modifiers_only_from_the_terminal_report() {
        let ctrl_shift = KeyMods {
            ctrl: true,
            shift: true,
            ..KeyMods::NONE
        };
        // Reported Shift is kept exactly.
        assert_eq!(captured(KeyCode::Char('A'), ctrl_shift), "ctrl+shift+a");
        assert_eq!(
            captured(KeyCode::Char('a'), KeyMods::SHIFT),
            captured(KeyCode::Char('A'), KeyMods::SHIFT)
        );
        // Case alone is not Shift: Caps Lock, or a legacy encoding that cannot report it.
        assert_eq!(captured(KeyCode::Char('A'), KeyMods::CTRL), "ctrl+a");
        assert_eq!(captured(KeyCode::Char('A'), KeyMods::NONE), "a");
        assert_eq!(captured(KeyCode::Char('?'), KeyMods::NONE), "?");
    }

    #[test]
    fn occupying_the_prefix_conflicts_with_prefix_not_every_command() {
        let input = crate::config::InputConfig::default();
        assert_eq!(input.prefix.canonical_lowercase(), "ctrl+a");
        assert!(
            captured_binding(KeyEvent {
                code: KeyCode::Char('a'),
                mods: KeyMods::CTRL,
            }) == input.prefix
        );
        let conflict = KeybindingConflict::prefix();
        assert_eq!(conflict.label, "Prefix");
        assert!(conflict.is_prefix());
        assert!(conflict.config_id.is_none());
        assert_ne!(
            captured_binding(KeyEvent {
                code: KeyCode::Enter,
                mods: KeyMods::NONE,
            }),
            input.prefix
        );
    }

    #[test]
    fn recorded_keys_save_as_scheme_or_literal_never_as_a_guessed_half() {
        let edit = |text: &str| {
            action_edit("close", &KeyBinding::from_str(text).unwrap()).overrides[0]
                .1
                .clone()
                .unwrap()
                .bindings
        };
        assert_eq!(edit("enter"), vec![BindingExpr::parse("enter").unwrap()]);
        // The default prefix and Mod, recorded, stay literal.
        assert_eq!(
            edit("alt-w"),
            vec![BindingExpr::Literal(KeyBinding::from_str("alt-w").unwrap())]
        );
        assert_eq!(
            edit("ctrl-a"),
            vec![BindingExpr::Literal(
                KeyBinding::from_str("ctrl-a").unwrap()
            )]
        );
    }

    /// Only a listening card records. With a candidate on screen, stray keys do nothing, Enter
    /// and Tab act only where offered, and Esc goes back to listening.
    #[test]
    fn only_a_listening_card_records_keys() {
        let key = |code: KeyCode| KeyEvent {
            code,
            mods: KeyMods::NONE,
        };
        let binding = KeyBinding::from_str("ctrl-f11").unwrap();
        let target = KeybindingTarget::Action("close".to_string());
        let capture = KeybindingEditorStage::Capture {
            target: target.clone(),
        };
        let review = KeybindingEditorStage::Review {
            target: target.clone(),
            binding: binding.clone(),
            conversion: None,
        };
        let blocked = KeybindingEditorStage::Conflict {
            target,
            binding,
            conflicts: Vec::new(),
            replaceable: false,
        };

        assert!(matches!(
            capture_key_msg(&capture, key(KeyCode::Char('x'))),
            Some(Msg::KeybindingCaptured(_))
        ));
        assert!(matches!(
            capture_key_msg(&capture, key(KeyCode::Tab)),
            Some(Msg::KeybindingCaptured(_))
        ));
        for stage in [&review, &blocked] {
            for code in [KeyCode::Char('x'), KeyCode::Tab, KeyCode::F(5)] {
                assert!(capture_key_msg(stage, key(code)).is_none(), "{code:?}");
            }
            assert!(matches!(
                capture_key_msg(stage, key(KeyCode::Esc)),
                Some(Msg::KeybindingRetryCapture)
            ));
        }
        assert!(matches!(
            capture_key_msg(&review, key(KeyCode::Enter)),
            Some(Msg::KeybindingSaveCaptured)
        ));
        assert!(capture_key_msg(&blocked, key(KeyCode::Enter)).is_none());
    }

    #[test]
    fn modifier_choices_cycle_and_map_from_input() {
        assert_eq!(ModifierChoice::Alt.stepped(1), ModifierChoice::Super);
        assert_eq!(ModifierChoice::Alt.stepped(-1), ModifierChoice::Off);
        assert_eq!(ModifierChoice::Off.stepped(1), ModifierChoice::Alt);
        let off = crate::config::InputConfig {
            modifier: WmModifier::Super,
            modifier_shortcuts: false,
            ..crate::config::InputConfig::default()
        };
        assert_eq!(ModifierChoice::from_input(&off), ModifierChoice::Off);
    }
}
