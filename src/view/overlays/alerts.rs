pub(crate) fn alerts_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    use AlertsAction::*;

    let pane = &ctx.state.config.pane;
    let entries = search_entries_with_groups([
        alerts_group(
            "General",
            vec![
                (
                    "Do not disturb",
                    enabled_status(ctx.state.do_not_disturb),
                    ToggleDoNotDisturb,
                ),
                (
                    "Bell urgency",
                    enabled_status(ctx.state.config.notifications.bell),
                    ToggleBellUrgency,
                ),
            ],
        ),
        alerts_group(
            "Pane border",
            vec![(
                "Effect",
                ctx.state.config.pane.alert_border.status_label(
                    ctx.state.config.animations.enabled,
                    ctx.state.config.animations.focus_chrome,
                ),
                CycleAlertBorder,
            )],
        ),
        alerts_group(
            "Workspace tab",
            vec![
                (
                    "Effect",
                    ctx.state.config.workbar.alert.mode.status_label(
                        ctx.state.config.animations.enabled,
                        ctx.state.config.animations.focus_chrome,
                    ),
                    CycleWorkbarAlert,
                ),
                (
                    "Highlight",
                    ctx.state.config.workbar.alert.paint.label().to_string(),
                    CycleWorkbarAlertPaint,
                ),
            ],
        ),
        alerts_group(
            "Status marks",
            vec![
                (
                    "Bell",
                    enabled_status(ctx.state.config.workbar.alert.bell),
                    ToggleMarkBell,
                ),
                (
                    "Blocked",
                    enabled_status(ctx.state.config.workbar.alert.blocked),
                    ToggleMarkBlocked,
                ),
                (
                    "Finished",
                    enabled_status(ctx.state.config.workbar.alert.finished),
                    ToggleMarkFinished,
                ),
                (
                    "Working",
                    enabled_status(ctx.state.config.workbar.alert.working),
                    ToggleMarkWorking,
                ),
                (
                    "Idle",
                    enabled_status(ctx.state.config.workbar.alert.idle),
                    ToggleMarkIdle,
                ),
            ],
        ),
        alerts_group(
            "Desktop notifications",
            vec![
                (
                    "Show notifications",
                    enabled_status(ctx.state.config.notifications.enabled),
                    ToggleDesktopEnabled,
                ),
                (
                    "Blocked",
                    enabled_status(ctx.state.config.notifications.pane_blocked),
                    ToggleDesktopBlocked,
                ),
                (
                    "Finished",
                    enabled_status(ctx.state.config.notifications.pane_done),
                    ToggleDesktopDone,
                ),
                (
                    "Exit",
                    enabled_status(ctx.state.config.notifications.pane_exit),
                    ToggleDesktopExit,
                ),
                (
                    "Exit with error",
                    enabled_status(ctx.state.config.notifications.pane_exit_error),
                    ToggleDesktopExitError,
                ),
            ],
        ),
        alerts_group(
            "Sounds",
            vec![
                (
                    "Play sounds",
                    enabled_status(ctx.state.config.sounds.enabled),
                    ToggleSoundEnabled,
                ),
                (
                    "Bell",
                    enabled_status(ctx.state.config.sounds.bell),
                    ToggleSoundBell,
                ),
                (
                    "Blocked",
                    enabled_status(ctx.state.config.sounds.blocked),
                    ToggleSoundBlocked,
                ),
                (
                    "Finished",
                    enabled_status(ctx.state.config.sounds.done),
                    ToggleSoundDone,
                ),
                (
                    "Exit with error",
                    enabled_status(ctx.state.config.sounds.error),
                    ToggleSoundError,
                ),
            ],
        ),
    ]);
    let selected = ctx.state.alerts_selected.and_then(|selected| {
        entries
            .iter()
            .filter_map(|entry| match entry {
                SearchEntry::Item(item) => Some(item.value.0),
                _ => None,
            })
            .position(|action| action == selected)
    });
    let item_style = fg_only(&ctx.state.theme.primary);
    let muted_style = fg_only(&ctx.state.theme.muted);
    let pane_flags = *pane;
    let notifications = ctx.state.config.notifications.enabled;
    let sounds = ctx.state.config.sounds.enabled;
    let palette = shared_search_palette::<(AlertsAction, String)>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search alerts…")
        .preserve_groups(true)
        .initial_selected_item_index(selected)
        .sync_selection(true)
        .input_key_interceptor(alerts_palette_key_interceptor(ctx))
        .render_item(Arc::new(move |item: &SearchItem<(AlertsAction, String)>, _highlight| {
            let disabled_reason = item.value.0.disabled_reason(&pane_flags, notifications, sounds);
            let style = if disabled_reason.is_some() { muted_style } else { item_style };
            ListItem::from_spans(vec![Span::new(item.label.as_ref()).style(style)])
                .description(disabled_reason.unwrap_or(&item.value.1))
                .description_style(style)
                .into()
        }))
        .on_select(ctx.link().callback(|event: SearchEvent<(AlertsAction, String)>| {
            Msg::AlertsSelect(event.item.value.0)
        }))
        .on_activate(ctx.link().callback(|event: SearchEvent<(AlertsAction, String)>| {
            Msg::AlertsActivate(event.item.value.0)
        }));
    let mut frame = Frame::new()
        .header_left("Alerts")
        .header_style(ctx.state.theme.accent.bold())
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element));
    if ctx.state.do_not_disturb {
        frame = frame.header_right("DND");
    }
    let panel: Element = frame.child(action_palette_frame(palette)).into();
    Modal::new()
        .width(Length::Px(60))
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(|_| Msg::CloseAlerts))
        .child(panel)
        .key(alerts_palette_key())
}

fn alerts_group(
    group: &'static str,
    rows: Vec<(&'static str, String, AlertsAction)>,
) -> (&'static str, Vec<SearchEntry<(AlertsAction, String)>>) {
    let entries = rows
        .into_iter()
        .map(|(label, status, action)| {
            SearchEntry::Item(
                SearchItem::new(label, (action, status)).aliases(alerts_palette_aliases(group, action)),
            )
        })
        .collect();
    (group, entries)
}

fn alerts_palette_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.mods != KeyMods::default() {
            return None;
        }
        match key.code {
            KeyCode::Left => Some(Msg::AlertsStep { reverse: true }),
            KeyCode::Right => Some(Msg::AlertsStep { reverse: false }),
            _ => None,
        }
    })
}

fn alerts_palette_aliases(group: &str, action: AlertsAction) -> Vec<Arc<str>> {
    let mut aliases = match action {
        AlertsAction::CycleAlertBorder => alias_list(&[
            "blocked pane border",
            "agent border",
            "attention border",
            "alert pulse",
        ]),
        AlertsAction::CycleWorkbarAlert => {
            alias_list(&["workspace tab alert", "workspace marker", "tab pulse"])
        }
        AlertsAction::CycleWorkbarAlertPaint => {
            alias_list(&["workspace tab alert paint", "marker fill"])
        }
        AlertsAction::ToggleDoNotDisturb => alias_list(&["dnd", "mute", "quiet"]),
        _ => Vec::new(),
    };
    aliases.push(Arc::from(group));
    aliases
}

#[cfg(test)]
mod tests {
    use super::{AlertsAction, alerts_palette_aliases};

    #[test]
    fn every_alerts_row_has_its_group_as_a_search_alias() {
        use AlertsAction::*;

        for (group, action) in [
            ("General", ToggleDoNotDisturb),
            ("General", ToggleBellUrgency),
            ("Pane border", CycleAlertBorder),
            ("Workspace tab", CycleWorkbarAlert),
            ("Workspace tab", CycleWorkbarAlertPaint),
            ("Status marks", ToggleMarkBell),
            ("Status marks", ToggleMarkBlocked),
            ("Status marks", ToggleMarkFinished),
            ("Status marks", ToggleMarkWorking),
            ("Status marks", ToggleMarkIdle),
            ("Desktop notifications", ToggleDesktopEnabled),
            ("Desktop notifications", ToggleDesktopBlocked),
            ("Desktop notifications", ToggleDesktopDone),
            ("Desktop notifications", ToggleDesktopExit),
            ("Desktop notifications", ToggleDesktopExitError),
            ("Sounds", ToggleSoundEnabled),
            ("Sounds", ToggleSoundBell),
            ("Sounds", ToggleSoundBlocked),
            ("Sounds", ToggleSoundDone),
            ("Sounds", ToggleSoundError),
        ] {
            assert!(
                alerts_palette_aliases(group, action)
                    .iter()
                    .any(|alias| alias.as_ref() == group),
                "{action:?} is missing its {group:?} alias"
            );
        }
    }
}
