/// A single `label key` footer hint (e.g. `submit enter`), styled like the palette hint bar.
fn hint_pill(theme: &Theme, label: &str, key: &str) -> Element {
    HStack::new()
        .gap(1)
        .width(Length::Auto)
        .height(Length::Auto)
        .child(
            Text::new(label)
                .overflow(Overflow::Clip)
                .style(fg_only(&theme.primary).bold()),
        )
        .child(
            Text::new(key)
                .overflow(Overflow::Clip)
                .style(fg_only(&theme.muted)),
        )
        .into()
}

/// The base footer row shared by every overlay hint bar: content-height with a leading gap above
/// it. Callers add [`hint_pill`] children and may override justify/gap.
fn hint_row() -> Flow {
    Flow::new()
        .padding((1, 1, 0, 1))
        .gap(3)
        .row_gap(0)
}

/// Footer hints shared by the single-input prompt overlays (rename pane/workspace/session, save
/// profile) so they read like the command palette instead of a bare dialog.
fn prompt_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    hint_row()
        .child(hint_pill(theme, "submit", "enter"))
        .child(hint_pill(theme, "cancel", "esc"))
        .into()
}

/// Shared chrome for the single-input prompt overlays so they all read like the command palette:
/// palette placement/border, no inner input border, a leading gap, and a submit/cancel hint footer.
/// Callers supply only what differs (title, placeholder, bound state, focus key, and messages).
///
/// `confirm` arms an in-modal destructive warning: when `Some(note)` the modal border and title turn
/// the error color and `note` renders as an inline caption above the hints, so a re-submit
/// confirmation reads off the modal itself instead of a separate toast.
#[allow(clippy::too_many_arguments)]
fn prompt_overlay(
    ctx: &Context<HyprmuxApp>,
    title: &str,
    placeholder: &str,
    input_state: &TextInput,
    input_key: &'static str,
    on_change: impl Fn(InputEvent) -> Msg + 'static,
    close: Msg,
    submit: Msg,
    confirm: Option<&str>,
) -> Element {
    let theme = &ctx.state.theme;
    let close_on_key = close.clone();
    let input = Input::bound(input_state)
        .placeholder(placeholder)
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .border(false)
        .padding((0, 1))
        .on_change(ctx.link().callback(on_change))
        .on_key(ctx.link().key_handler(move |key| {
            if key.is(KeyCode::Esc) {
                Some(close_on_key.clone())
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(submit.clone())
            } else {
                None
            }
        }));

    let mut body = VStack::new()
        .height(Length::Auto)
        .padding((1, 0, 0, 0))
        .child(input.key(input_key));
    if let Some(note) = confirm {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(note)
                        .overflow(Overflow::Wrap)
                        .style(Style::new().fg(theme.status.error).italic()),
                ),
        );
    }
    body = body.child(prompt_hints(ctx));

    let mut modal = action_palette_modal(ctx, title)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(body));
    if confirm.is_some() {
        // Recolor the shared modal chrome to the error accent so the whole dialog reads as "armed"
        // (border + title), matching the inline caption. The modal captures focus the moment it
        // opens, so `focus_style` must repeat the accent or the theme focus role repaints the border.
        let armed_frame = Style::new()
            .bg(theme.surface.element)
            .fg(theme.status.error);
        modal = modal
            .frame_style(armed_frame)
            .focus_style(armed_frame)
            .title_style(Style::new().fg(theme.status.error).bold());
    }
    modal.into()
}

pub(crate) fn rename_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename.as_ref() else {
        return Text::new("").into();
    };
    let display_number = ctx
        .state
        .current().workspaces
        .iter()
        .find_map(|workspace| workspace.pane_display_number(rename.target))
        .unwrap_or(rename.target as usize);
    prompt_overlay(
        ctx,
        &format!("Rename pane {display_number}"),
        "Pane name, empty clears custom title",
        &rename.input,
        rename_input_key(),
        Msg::RenamePaneChanged,
        Msg::CloseRenamePane,
        Msg::SubmitRenamePane,
        None,
    )
}

pub(crate) fn rename_session_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename_session.as_ref() else {
        return Text::new("").into();
    };
    let (title, placeholder) = match rename.mode {
        crate::state::NamingMode::CreateSession => {
            ("Create session".to_string(), "Session name".to_string())
        }
        crate::state::NamingMode::OpenProfileAs => {
            (
                "Open profile as".to_string(),
                "Name (empty: ephemeral)".to_string(),
            )
        }
        // The same ephemeral-naming prompt serves in-place naming and detach-and-name; the latter
        // keeps the server running for reattach, so it names the action to make that clear.
        crate::state::NamingMode::NameEphemeralSession if rename.detach_after => (
            "Detach session".to_string(),
            "Name to keep it running".to_string(),
        ),
        crate::state::NamingMode::NameEphemeralSession => {
            ("Name session".to_string(), "Session name".to_string())
        }
        crate::state::NamingMode::RenameSession => {
            ("Rename session".to_string(), "Session name".to_string())
        }
        crate::state::NamingMode::RenameWorkspace { index } => (
            format!("Rename workspace {}", index + 1),
            "Workspace name, empty clears it".to_string(),
        ),
    };
    // A create-session prompt that would discard the current disposable ephemeral session shows an
    // in-modal warning once armed (see `SessionRenameState::pending_confirm`).
    let confirm = rename
        .pending_confirm
        .then_some("again to confirm (ends ephemeral session)");
    prompt_overlay(
        ctx,
        &title,
        &placeholder,
        &rename.input,
        rename_session_input_key(),
        Msg::RenameSessionChanged,
        Msg::CloseRenameSession,
        Msg::SubmitRenameSession,
        confirm,
    )
}

pub(crate) fn save_profile_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(prompt) = ctx.state.save_profile_prompt.as_ref() else {
        return Text::new("").into();
    };
    let confirm = prompt.pending_overwrite.then(|| {
        format!(
            "`{}` exists - Enter again overwrites it",
            prompt.input.text().trim()
        )
    });
    prompt_overlay(
        ctx,
        "Capture session as profile",
        "Profile name",
        &prompt.input,
        save_profile_key(),
        Msg::SaveProfileNameChanged,
        Msg::CloseSaveProfile,
        Msg::SubmitSaveProfile,
        confirm.as_deref(),
    )
}

/// Assemble a palette-style overlay: shared modal chrome, a borderless frame, a close handler, and
/// the overlay's focus key. `content` is the palette itself, or a body wrapping a palette plus a
/// hint footer.
fn action_palette(
    ctx: &Context<HyprmuxApp>,
    title: &str,
    key: &'static str,
    close: Msg,
    content: impl Into<Element>,
    width: u16,
) -> Element {
    action_palette_modal_with_width(ctx, title, width)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(content))
        .key(key)
}
