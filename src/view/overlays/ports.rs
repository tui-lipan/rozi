use super::*;

use crate::ops::ports::{PortMsg, default_forward_target, sorted_keys};
use crate::session::remote::forward::ForwardKey;
use crate::state::{
    ForwardState, PortFormField, PortFormState, PortForwardEntry, PortsOverlayState,
};
use crate::view::vocabulary::{
    MARKER_FAILED, MARKER_LIVE, picker_circle_spinner_gutter, picker_marker_gutter,
};

const PORT_FORM_LABEL_WIDTH: u16 = 13;

fn forward_status_label(state: ForwardState) -> &'static str {
    match state {
        ForwardState::Ready => "Ready",
        ForwardState::Starting => "Starting…",
        ForwardState::Reconnecting => "Reconnecting…",
        ForwardState::AuthRequired => "Auth required",
        ForwardState::Failed => "Failed",
    }
}

fn forward_description(ctx: &Context<AppRoot>, entry: &PortForwardEntry) -> ItemDescription {
    let host = ctx.state.remote_target_label(&entry.key.target);
    let status = forward_status_label(entry.state);
    if matches!(
        entry.state,
        ForwardState::Failed | ForwardState::AuthRequired | ForwardState::Reconnecting
    ) && let Some(error) = entry.error.as_deref()
    {
        let reason = crate::session::discovery::probe_failure_reason(error);
        return picker_description(crate::view::vocabulary::fields([host.as_str(), reason]));
    }
    picker_description(crate::view::vocabulary::fields([host.as_str(), status]))
}

fn forward_label(entry: &PortForwardEntry) -> String {
    let remote = format!(
        "{}:{}",
        entry.key.remote_host.display_label(),
        entry.key.remote_port
    );
    if entry.local_port != 0 {
        format!("127.0.0.1:{} → {remote}", entry.local_port)
    } else {
        remote
    }
}

fn forward_gutter(ctx: &Context<AppRoot>, state: ForwardState) -> ListItemGutter {
    let theme = &ctx.state.theme;
    match state {
        ForwardState::Ready => {
            picker_marker_gutter(MARKER_LIVE, Style::new().fg(theme.status.success))
        }
        ForwardState::Starting | ForwardState::Reconnecting => {
            picker_circle_spinner_gutter(Style::new().fg(theme.status.warning))
        }
        ForwardState::AuthRequired | ForwardState::Failed => {
            picker_marker_gutter(MARKER_FAILED, Style::new().fg(theme.status.error))
        }
    }
}

fn ports_list_overlay(ctx: &Context<AppRoot>, overlay: &PortsOverlayState) -> Element {
    let keys = sorted_keys(&ctx.state);
    let entries: Vec<_> = keys
        .iter()
        .filter_map(|key| {
            let entry = ctx.state.ports.get(key)?;
            Some(
                SearchEntry::item(forward_label(entry), key.clone())
                    .description(forward_description(ctx, entry)),
            )
        })
        .collect();

    let selected_key = overlay
        .selected
        .as_ref()
        .filter(|key| ctx.state.ports.get(key).is_some())
        .cloned()
        .or_else(|| keys.first().cloned());
    let selected_index = selected_key.as_ref().and_then(|selected| {
        entries.iter().enumerate().find_map(|(index, entry)| {
            matches!(entry, SearchEntry::Item(item) if &item.value == selected).then_some(index)
        })
    });
    let selected_entry = selected_key
        .as_ref()
        .and_then(|key| ctx.state.ports.get(key));

    let enter_action = match selected_entry {
        Some(entry) if entry.state == ForwardState::Ready && entry.local_port != 0 => {
            Some(("open", true))
        }
        Some(entry)
            if matches!(
                entry.state,
                ForwardState::Failed | ForwardState::AuthRequired
            ) =>
        {
            Some(("retry", true))
        }
        _ => None,
    };
    let (enter_label, enter_enabled) = enter_action.unwrap_or(("open", false));
    let can_add = default_forward_target(&ctx.state).is_some();
    let can_retry = selected_entry.is_some();
    let can_close = selected_key.is_some();
    let armed_row = overlay
        .pending_close
        .as_ref()
        .filter(|armed| selected_key.as_ref() == Some(*armed))
        .cloned();
    let armed = armed_row.is_some();

    let empty_text = if overlay.query.text().trim().is_empty() {
        "No forwarded ports".to_string()
    } else {
        format!("No forwards match `{}`", overlay.query.text().trim())
    };
    let fallback = ctx.link().key_handler(|key| {
        key.is(KeyCode::Esc)
            .then_some(Msg::Ports(PortMsg::CloseOverlay))
    });
    let actions = vec![
        OverlayAction::new(
            "enter",
            enter_label,
            selected_key
                .clone()
                .map(|key| Msg::Ports(PortMsg::Activate(key)))
                .unwrap_or(Msg::Ports(PortMsg::CloseOverlay)),
            enter_enabled,
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-n",
            "add forward",
            Msg::Ports(PortMsg::OpenAddForm),
            can_add,
        ),
        OverlayAction::new(
            "ctrl-r",
            "retry",
            Msg::Ports(PortMsg::RetrySelected),
            can_retry,
        ),
        OverlayAction::destructive(
            ctx,
            "close",
            Msg::Ports(PortMsg::CloseSelected),
            can_close,
            armed,
        ),
    ];

    let statuses: Vec<(ForwardKey, ForwardState)> = keys
        .iter()
        .filter_map(|key| {
            ctx.state
                .ports
                .get(key)
                .map(|entry| (key.clone(), entry.state))
        })
        .collect();
    let ready_gutter = forward_gutter(ctx, ForwardState::Ready);
    let starting_gutter = forward_gutter(ctx, ForwardState::Starting);
    let reconnecting_gutter = forward_gutter(ctx, ForwardState::Reconnecting);
    let auth_gutter = forward_gutter(ctx, ForwardState::AuthRequired);
    let failed_gutter = forward_gutter(ctx, ForwardState::Failed);

    OverlayPalette::new(
        "Forwarded ports",
        crate::view::ports_overlay_key(),
        Msg::Ports(PortMsg::CloseOverlay),
        74,
    )
    .entries(entries)
    .description_wins(Length::Percent(36))
    .actions(actions)
    .armed_row(armed_row)
    .placeholder("Filter forwarded ports…")
    .initial_query(overlay.query.text().to_string())
    .selected(selected_index)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .item_gutter(Arc::new(move |item: &SearchItem<ForwardKey>, _hl| {
        let state = statuses
            .iter()
            .find(|(key, _)| key == &item.value)
            .map(|(_, state)| *state)?;
        Some(match state {
            ForwardState::Ready => ready_gutter.clone(),
            ForwardState::Starting => starting_gutter.clone(),
            ForwardState::Reconnecting => reconnecting_gutter.clone(),
            ForwardState::AuthRequired => auth_gutter.clone(),
            ForwardState::Failed => failed_gutter.clone(),
        })
    }))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::Ports(PortMsg::QueryChanged(query.to_string()))),
    )
    .on_select(ctx.link().callback(|event: SearchEvent<ForwardKey>| {
        Msg::Ports(PortMsg::Select(event.item.value.clone()))
    }))
    .on_activate(ctx.link().callback(|event: SearchEvent<ForwardKey>| {
        Msg::Ports(PortMsg::Activate(event.item.value.clone()))
    }))
    .render(ctx)
}

fn port_form_row(ctx: &Context<AppRoot>, form: &PortFormState, field: PortFormField) -> Element {
    let theme = &ctx.state.theme;
    let focused = form.focus == field;
    let label_style = if focused {
        fg_only(&theme.primary).bold()
    } else {
        fg_only(&theme.muted)
    };
    let input: Element =
        if focused {
            Input::bound(form.input(field))
                .placeholder(field.placeholder())
                .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
                .focus_style(
                    Style::new()
                        .fg(theme.border_active)
                        .bg(theme.surface.element),
                )
                .selection_style(theme.text_selection)
                .width(Length::Flex(1))
                .border(false)
                .padding((0, 0))
                .on_change(ctx.link().callback(move |event: InputEvent| {
                    Msg::Ports(PortMsg::FormChanged(field, event))
                }))
                .on_key(ctx.link().key_handler(|key| {
                    if key.is(KeyCode::Esc) {
                        Some(Msg::Ports(PortMsg::CloseAddForm))
                    } else if key.code == KeyCode::Enter && !key.mods.ctrl && !key.mods.alt {
                        Some(Msg::Ports(PortMsg::SubmitAddForm))
                    } else {
                        None
                    }
                }))
                .key(crate::view::port_form_input_key())
        } else {
            let text = form.input(field).text();
            let (text, style) = if text.is_empty() {
                (field.placeholder(), fg_only(&theme.muted).italic())
            } else {
                (text, fg_only(&theme.primary))
            };
            Text::new(text)
                .overflow(Overflow::Clip)
                .width(Length::Flex(1))
                .style(style)
                .into()
        };
    HStack::new()
        .height(Length::Auto)
        .padding((0, 1, 0, 0))
        .child(
            Text::new(if focused { "› " } else { "  " })
                .overflow(Overflow::Clip)
                .width(Length::Px(2))
                .style(fg_only(&theme.accent)),
        )
        .child(
            Text::new(field.label())
                .overflow(Overflow::Clip)
                .width(Length::Px(PORT_FORM_LABEL_WIDTH))
                .style(label_style),
        )
        .child(input)
        .into()
}

fn port_form_overlay(ctx: &Context<AppRoot>, form: &PortFormState) -> Element {
    let theme = &ctx.state.theme;
    let host = ctx.state.remote_target_label(&form.target);
    let mut body = VStack::new().height(Length::Auto).padding((1, 0, 0, 0));
    for field in PortFormField::ORDER {
        body = body.child(port_form_row(ctx, form, field));
    }
    if let Some(error) = form.error.as_deref() {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(error)
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(Style::new().fg(theme.status.warning).italic()),
                ),
        );
    }
    body = body.child(
        hint_row()
            .child(hint_button(
                ctx,
                "forward",
                "enter",
                Msg::Ports(PortMsg::SubmitAddForm),
            ))
            .child(hint_pill(theme, "next field", "tab")),
    );

    action_palette_modal(ctx, &format!("Add forward · {host}"))
        .on_close(ctx.link().callback(|_| Msg::Ports(PortMsg::CloseAddForm)))
        .child(body)
        .key("rozi-port-form-modal")
}

pub(crate) fn ports_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(overlay) = ctx.state.ports_overlay.as_ref() else {
        return Text::new("").into();
    };
    if let Some(form) = overlay.form.as_ref() {
        return port_form_overlay(ctx, form);
    }
    ports_list_overlay(ctx, overlay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::ports::PortMsg;
    use crate::session::remote::RemoteTarget;
    use crate::session::remote::forward::ForwardHost;
    use crate::state::PortForwardEntry;
    use tui_lipan::TestBackend;

    #[test]
    fn ports_overlay_renders_rows_armed_close_and_add_form() {
        let workbox = RemoteTarget::Alias("workbox".into());
        let key1 = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Loopback,
            remote_port: 5173,
        };
        let key2 = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Host("database.internal".into()),
            remote_port: 5432,
        };

        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        backend.state_mut().config.animations.enabled = false;
        backend.state_mut().current_mut().remote_target = Some(workbox);
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key1,
            local_port: 5173,
            state: ForwardState::Ready,
            generation: 1,
            handle: None,
            error: None,
            pending_opens: Vec::new(),
        });
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key2,
            local_port: 15432,
            state: ForwardState::Failed,
            generation: 2,
            handle: None,
            error: Some("ssh: connect to host workbox: Connection refused".into()),
            pending_opens: Vec::new(),
        });

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
            .unwrap();
        backend.render();
        let list_text = backend.capture_frame().plain_text();
        assert!(list_text.contains("Forwarded ports"), "{list_text}");
        assert!(
            list_text.contains("127.0.0.1:5173 → localhost:5173"),
            "{list_text}"
        );
        assert!(
            list_text.contains("127.0.0.1:15432 → database.internal:5432"),
            "{list_text}"
        );
        assert!(list_text.contains("open Enter"), "{list_text}");
        assert!(list_text.contains("add forward"), "{list_text}");

        // Optional PNG dump when reviewing visuals outside git.
        #[cfg(feature = "ui-snapshot")]
        if let Ok(path) = std::env::var("ROZI_TEST_PORTS_LIST_PNG") {
            let png = backend.capture_ui_snapshot().to_png_default().unwrap();
            std::fs::write(path, png).unwrap();
        }

        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        backend.render();
        let armed_text = backend.capture_frame().plain_text();
        assert!(armed_text.contains("again to close"), "{armed_text}");

        backend
            .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
            .unwrap();
        backend.render();
        let form_text = backend.capture_frame().plain_text();
        assert!(form_text.contains("Add forward · workbox"), "{form_text}");
        assert!(form_text.contains("Remote host"), "{form_text}");
        assert!(form_text.contains("Remote port"), "{form_text}");
        assert!(form_text.contains("Local port"), "{form_text}");

        #[cfg(feature = "ui-snapshot")]
        if let Ok(path) = std::env::var("ROZI_TEST_PORTS_FORM_PNG") {
            let png = backend.capture_ui_snapshot().to_png_default().unwrap();
            std::fs::write(path, png).unwrap();
        }
    }
}
