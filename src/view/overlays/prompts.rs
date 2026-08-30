/// Ctrl plus `letter`, case-insensitive. Overlay interceptors share this so a chord the footer
/// omitted is the same test the handler uses when it stays silent.
fn ctrl_letter(key: &KeyEvent, letter: char) -> bool {
    key.mods.ctrl && matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&letter))
}

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
            Text::new(crate::keys_display::format_keys(key))
                .overflow(Overflow::Clip)
                .style(fg_only(&theme.muted)),
        )
        .into()
}

/// The base footer row shared by every overlay hint bar: content-height with a leading gap above
/// it. Callers add [`hint_pill`] children and may override justify/gap.
fn hint_row() -> Flow {
    Flow::new().padding((1, 1, 0, 1)).gap(3).row_gap(0)
}

/// How far a dialog behind another one recedes: the same half-strength Settings fades by behind
/// the padding editor, so a stack of two dialogs reads the same wherever it occurs.
const BACKDROP_RECESSION: f32 = 0.5;

/// Footer hints shared by the single-input prompt overlays (rename pane/workspace/session, save
/// profile) so they read like the command palette instead of a bare dialog.
///
/// A prompt raised from a picker returns to it on Esc, but that goes unsaid: "back esc" is the one
/// thing every dialog does, so spelling it out only crowds the hints that carry information.
/// `submit` names the commit keys, one pill each, so a prompt with two distinct commits (capture
/// with or without naming the session) spells out what each one does instead of a bare `submit`.
fn prompt_hints(ctx: &Context<AppRoot>, submit: &[(&str, &str)], cancel: bool) -> Element {
    let theme = &ctx.state.theme;
    let mut row = hint_row();
    for (label, key) in submit {
        row = row.child(hint_pill(theme, label, key));
    }
    if cancel {
        row = row.child(hint_pill(theme, "cancel", "esc"));
    }
    row.into()
}

/// What a prompt has to say about itself, and how loudly.
///
/// The two differ only in whether the modal's own border and title change colour, and that is the
/// whole distinction: the error accent is rozi's "this destroys something" signal, so spending it
/// on news the user merely needs to read would make every rejected password look like a warning
/// about to close their sessions.
#[derive(Clone, Copy)]
enum PromptCaption<'a> {
    /// Something is about to be destroyed or overwritten and a second Enter will do it. Recolors
    /// the modal to the error accent so the whole dialog reads as armed.
    Armed(&'a str),
    /// Something the user needs to know before answering — a rejected password, a name that will
    /// not do. Stated in the warning colour; the chrome stays as it was.
    Note(&'a str),
}

impl<'a> PromptCaption<'a> {
    fn text(self) -> &'a str {
        match self {
            Self::Armed(text) | Self::Note(text) => text,
        }
    }

    fn arms_chrome(self) -> bool {
        matches!(self, Self::Armed(_))
    }
}

/// What one single-input prompt differs by. Grouped so [`prompt_overlay`] keeps a readable
/// signature as prompts grow options: the messages stay positional, the appearance does not.
struct PromptChrome<'a> {
    title: &'a str,
    placeholder: &'a str,
    /// Wrapped text between the title and the field, for a question too long to be a title.
    detail: Option<&'a str>,
    /// Glyph standing in for every typed character. `Some` for anything the user would not want
    /// on screen — or in a capture.
    mask: Option<char>,
    /// An inline caption above the hints, so what the modal has to say reads off the modal itself
    /// rather than a separate toast. See [`PromptCaption`] for what each kind costs the chrome.
    caption: Option<PromptCaption<'a>>,
    submit_hints: &'a [(&'a str, &'a str)],
    /// Spell out `cancel esc` even when a nested-dialog return is set. A prompt raised by a
    /// background event is not part of that chain, so the escape hatch has to be stated.
    always_cancel_hint: bool,
    /// Fade whatever is already on screen behind this prompt, so a dialog it lands on top of
    /// recedes instead of competing with it. For a prompt that arrives over another dialog rather
    /// than replacing it; the modal's own backdrop does the work, so it covers the dialog's border
    /// and title too, not just the body.
    dim_behind: bool,
}

impl<'a> PromptChrome<'a> {
    fn new(title: &'a str, placeholder: &'a str, submit_hints: &'a [(&'a str, &'a str)]) -> Self {
        Self {
            title,
            placeholder,
            detail: None,
            mask: None,
            caption: None,
            submit_hints,
            always_cancel_hint: false,
            dim_behind: false,
        }
    }
}

/// Shared chrome for the single-input prompt overlays so they all read like the command palette:
/// palette placement/border, no inner input border, a leading gap, and a submit/cancel hint footer.
/// Callers supply only what differs (see [`PromptChrome`], plus the bound state, focus key, and
/// messages).
fn prompt_overlay(
    ctx: &Context<AppRoot>,
    chrome: PromptChrome<'_>,
    input_state: &TextInput,
    input_key: &'static str,
    on_change: impl Fn(InputEvent) -> Msg + 'static,
    close: Msg,
    submit: Msg,
) -> Element {
    let PromptChrome {
        title,
        placeholder,
        detail,
        mask,
        caption,
        submit_hints,
        always_cancel_hint,
        dim_behind,
    } = chrome;
    let theme = &ctx.state.theme;
    let close_on_key = close.clone();
    let input = Input::bound(input_state)
        .placeholder(placeholder)
        .mask(mask)
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

    let mut body = VStack::new().height(Length::Auto).padding((1, 0, 0, 0));
    if let Some(detail) = detail {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((0, 1, 1, 1))
                .child(
                    Text::new(detail)
                        .overflow(Overflow::Wrap)
                        // Flex, not intrinsic: an intrinsically-sized child is measured unwrapped,
                        // so a multi-line ssh prompt would be given three rows and render five —
                        // silently clipping the question it ends with.
                        .width(Length::Flex(1))
                        .style(fg_only(&theme.muted)),
                ),
        );
    }
    body = body.child(input.key(input_key));
    if let Some(caption) = caption {
        let accent = if caption.arms_chrome() {
            theme.status.error
        } else {
            theme.status.warning
        };
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(caption.text())
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(Style::new().fg(accent).italic()),
                ),
        );
    }
    body = body.child(prompt_hints(
        ctx,
        submit_hints,
        always_cancel_hint || ctx.state.overlay_return.is_none(),
    ));

    let mut modal = action_palette_modal(ctx, title)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(body));
    if dim_behind {
        // The same recession the workspace makes for any dialog, applied by the overlay stack, so
        // every layer already on screen fades together rather than one panel at a time.
        modal = modal.backdrop_style(
            Style::new().tint_by(theme.surface.backdrop, BACKDROP_RECESSION),
        );
    }
    if caption.is_some_and(PromptCaption::arms_chrome) {
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

pub(crate) fn rename_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(rename) = ctx.state.rename.as_ref() else {
        return Text::new("").into();
    };
    prompt_overlay(
        ctx,
        PromptChrome::new(
            "Rename pane",
            "Pane name, empty clears custom title",
            &[("submit", "enter")],
        ),
        &rename.input,
        rename_input_key(),
        Msg::RenamePaneChanged,
        Msg::CloseRenamePane,
        Msg::SubmitRenamePane,
    )
}

pub(crate) fn rename_session_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(rename) = ctx.state.rename_session.as_ref() else {
        return Text::new("").into();
    };
    let (title, placeholder) = match rename.mode {
        crate::state::NamingMode::CreateSession => {
            let title = match &rename.host_target {
                Some(target) => format!("New session on {}", target.display_label()),
                None => "Create session".to_string(),
            };
            (title, "Session name".to_string())
        }
        crate::state::NamingMode::OpenProfileAs => (
            "Launch profile as".to_string(),
            "Name (empty: ephemeral)".to_string(),
        ),
        // The same ephemeral-naming prompt serves in-place naming and the leave prompt. The latter
        // is the last chance to keep the session, so it says what each answer does. It never shows
        // the generated `eph-<pid>` name: everywhere else calls these "temporary" sessions, and a
        // name the user has never seen is no help when deciding whether to close one.
        crate::state::NamingMode::NameEphemeralSession if rename.leave.is_some() => (
            "Keep this session?".to_string(),
            "Name it to keep it running (empty closes it)".to_string(),
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
    // The armed line is the whole safety step for closing a temporary session, so it says how many
    // go and stays on screen under the finger — not in a toast that may already have faded.
    // A rejected name takes the same slot and wins it: the two never apply at once (arming needs an
    // empty submit, rejection needs a bad one), and the reason belongs beside the field it is about.
    let confirm = rename.error.clone().or_else(|| {
        rename
            .leave
            .filter(|leave| leave.armed)
            .map(|leave| match leave.temporary {
                1 => "Enter again closes this temporary session and quits".to_string(),
                count => format!("Enter again closes {count} temporary sessions and quits"),
            })
    });
    prompt_overlay(
        ctx,
        PromptChrome {
            caption: confirm.as_deref().map(PromptCaption::Armed),
            ..PromptChrome::new(&title, &placeholder, &[("submit", "enter")])
        },
        &rename.input,
        rename_session_input_key(),
        Msg::RenameSessionChanged,
        Msg::CloseRenameSession,
        Msg::SubmitRenameSession,
    )
}

pub(crate) fn save_profile_overlay(ctx: &Context<AppRoot>) -> Element {
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
        PromptChrome {
            caption: confirm.as_deref().map(PromptCaption::Armed),
            ..PromptChrome::new(
                "Capture session as profile",
                "Profile name",
                &[("capture", "enter")],
            )
        },
        &prompt.input,
        save_profile_key(),
        Msg::SaveProfileNameChanged,
        Msg::CloseSaveProfile,
        Msg::SubmitSaveProfile,
    )
}

/// The longest question that still fits the palette's title border beside the `SSH · ` prefix.
/// Past it the question moves into the body, where it can wrap instead of being truncated.
const ASKPASS_TITLE_QUESTION_MAX: usize = 48;

/// The modal standing in for the terminal password prompt `ssh` would otherwise write over the UI.
///
/// A password prompt is one short line naming an account or a key, so it becomes the modal's own
/// label rather than a sentence floating above the field — the same shape every other dialog uses
/// for "which thing is this about". A host-key prompt is several lines ending in a fingerprint,
/// which has to stay in the body where it has room to wrap; it is also the one prompt shown
/// unmasked, because reading back what you typed is half of confirming a fingerprint.
pub(crate) fn askpass_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(askpass) = ctx.state.askpass.as_ref() else {
        return Text::new("").into();
    };
    let secret = askpass.current.kind.is_secret();
    // Trailing prompt punctuation reads as a stray colon once the text is a label rather than
    // something a cursor sits after.
    let question = askpass
        .current
        .prompt
        .trim()
        .trim_end_matches(':')
        .trim_end();
    let inline_title = secret
        && !question.contains('\n')
        && question.chars().count() <= ASKPASS_TITLE_QUESTION_MAX;
    let title = if inline_title {
        format!("SSH · {question}")
    } else if secret {
        "SSH authentication".to_string()
    } else {
        "SSH confirmation".to_string()
    };
    let placeholder = if secret {
        "Password or passphrase"
    } else {
        "yes / no"
    };
    prompt_overlay(
        ctx,
        PromptChrome {
            detail: (!inline_title).then_some(question),
            mask: secret.then_some('•'),
            always_cancel_hint: true,
            dim_behind: true,
            // A rejected password is news, not a warning: the error accent is reserved for a
            // dialog that is about to destroy something.
            caption: askpass.current.error.as_deref().map(PromptCaption::Note),
            ..PromptChrome::new(&title, placeholder, &[("send", "enter")])
        },
        &askpass.input,
        askpass_input_key(),
        Msg::RemoteAskpassChanged,
        Msg::CancelRemoteAskpass,
        Msg::SubmitRemoteAskpass,
    )
}

/// Assemble a palette-style overlay: shared modal chrome, a borderless frame, a close handler, and
/// the overlay's focus key. `content` is the palette itself, or a body wrapping a palette plus a
/// hint footer.
fn action_palette(
    ctx: &Context<AppRoot>,
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
