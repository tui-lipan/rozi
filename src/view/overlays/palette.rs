use super::*;

use std::borrow::Cow;

pub(crate) type OverlayItemRenderer<T> =
    Arc<dyn Fn(&SearchItem<T>, &SearchHighlight) -> Option<ListItem>>;
pub(crate) type OverlayGutterRenderer<T> =
    Arc<dyn Fn(&SearchItem<T>, &SearchHighlight) -> Option<ListItemGutter>>;

pub(super) fn picker_description(description: impl AsRef<str>) -> ItemDescription {
    ItemDescription::new().right(format!("  {}", description.as_ref()))
}

pub(super) fn picker_row(
    label: impl IntoIterator<Item = Span>,
    description: impl Into<Arc<str>>,
    description_style: Style,
) -> ListItem {
    let item = ListItem::from_spans(label);
    let description = description.into();
    if description.is_empty() {
        item
    } else {
        item.description(format!("  {description}"))
            .description_style(description_style)
            .primary_truncate_description_first(true)
    }
}

/// Selection highlight shared by pickers while an action awaits a confirming second press.
pub(super) fn picker_selection_style(theme: &Theme, pending_accent: Option<Color>) -> Style {
    if let Some(accent) = pending_accent {
        Style::new()
            .bg(accent)
            .fg(readable_text_color(None, accent))
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    } else {
        Style::new()
            .fg(theme.surface.backdrop)
            .bg(theme.border_active)
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    }
}

/// Armed second-press row shared by destructive and cautionary picker actions.
pub(super) fn render_pending_confirm_item(
    label: &str,
    accent: Color,
    cue: &str,
    strike: bool,
) -> ListItem {
    let fg = readable_text_color(None, accent);
    let label_style = if strike {
        Style::new().fg(fg).strikethrough()
    } else {
        Style::new().fg(fg).bold()
    };
    picker_row(
        [Span::new(label).style(label_style)],
        cue,
        Style::new().fg(fg).italic(),
    )
    .style(Style::new().bg(accent).fg(fg))
}

#[derive(Clone)]
pub(crate) struct ConfirmCue {
    pub cue: String,
    pub accent: Color,
    pub strike: bool,
}

#[derive(Clone)]
pub(crate) struct OverlayAction {
    pub key: KeyBinding,
    pub label: String,
    pub msg: Msg,
    pub enabled: bool,
    pub intercept: bool,
    pub hint: bool,
    pub confirm: Option<ConfirmCue>,
}

impl OverlayAction {
    pub(crate) fn new(key: &str, label: impl Into<String>, msg: Msg, enabled: bool) -> Self {
        Self {
            key: KeyBinding::from_str(key).expect("built-in overlay key parses"),
            label: label.into(),
            msg,
            enabled,
            intercept: true,
            hint: true,
            confirm: None,
        }
    }

    pub(crate) fn try_new(
        key: &str,
        label: impl Into<String>,
        msg: Msg,
        enabled: bool,
    ) -> Option<Self> {
        Some(Self {
            key: KeyBinding::from_str(key).ok()?,
            label: label.into(),
            msg,
            enabled,
            intercept: true,
            hint: true,
            confirm: None,
        })
    }

    /// Show the action in the footer but let SearchPalette activate the visible row.
    pub(crate) fn hint_only(mut self) -> Self {
        self.intercept = false;
        self
    }

    /// Keep the key interceptor without advertising it in the footer.
    pub(crate) fn hide_hint(mut self) -> Self {
        self.hint = false;
        self
    }

    pub(super) fn shows_hint(&self) -> bool {
        self.enabled && self.hint
    }

    pub(crate) fn confirm(mut self, cue: impl Into<String>, accent: Color, strike: bool) -> Self {
        self.confirm = Some(ConfirmCue {
            cue: cue.into(),
            accent,
            strike,
        });
        self
    }

    pub(crate) fn confirm_if(
        self,
        armed: bool,
        cue: impl Into<String>,
        accent: Color,
        strike: bool,
    ) -> Self {
        if armed {
            self.confirm(cue, accent, strike)
        } else {
            self
        }
    }
}

pub(crate) fn overlay_hints(theme: &Theme, actions: &[OverlayAction]) -> Element {
    let mut row = hint_row();
    let mut any = false;
    for action in actions.iter().filter(|action| action.shows_hint()) {
        any = true;
        row = row.child(hint_pill(theme, &action.label, &action.key.label()));
    }
    if any {
        row.into()
    } else {
        Text::new("").into()
    }
}

pub(crate) fn overlay_interceptor(ctx: &Context<AppRoot>, actions: &[OverlayAction]) -> KeyHandler {
    let actions = actions
        .iter()
        .filter(|action| action.enabled && action.intercept)
        .map(|action| (action.key.clone(), action.msg.clone()))
        .collect::<Vec<_>>();
    ctx.link().key_handler(move |key| {
        actions
            .iter()
            .find(|(binding, _)| binding.matches_sequence(&[key]))
            .map(|(_, msg)| msg.clone())
    })
}

/// Pages of an [`OverlayPalette`], drawn as the shared picker tab strip above the query.
///
/// The query sits inside the tab rather than above it because each page keeps its own filter:
/// the palette remounts per page, seeded with that page's query and highlight.
pub(crate) struct OverlayTabs {
    labels: Vec<String>,
    active: usize,
    select: fn(usize) -> Msg,
}

impl OverlayTabs {
    pub(crate) fn new(labels: Vec<String>, active: usize, select: fn(usize) -> Msg) -> Self {
        Self {
            labels,
            active,
            select,
        }
    }

    /// Tab and Shift+Tab, or Left and Right, step through the pages, wrapping, as they do on every
    /// tabbed picker.
    fn interceptor(&self, ctx: &Context<AppRoot>) -> KeyHandler {
        let count = self.labels.len();
        let active = self.active;
        let select = self.select;
        ctx.link().key_handler(move |key| {
            let plain = !key.mods.ctrl && !key.mods.alt && !key.mods.super_key;
            if count < 2 || !plain {
                return None;
            }
            match key.code {
                KeyCode::Tab if !key.mods.shift => Some(select((active + 1) % count)),
                KeyCode::BackTab | KeyCode::Tab => Some(select((active + count - 1) % count)),
                KeyCode::Right if !key.mods.shift => Some(select((active + 1) % count)),
                KeyCode::Left if !key.mods.shift => Some(select((active + count - 1) % count)),
                _ => None,
            }
        })
    }
}

pub(crate) struct OverlayPalette<'a, T> {
    title: Cow<'a, str>,
    header_right: Option<Cow<'a, str>>,
    key: &'static str,
    close: Msg,
    width: u16,
    placeholder: Cow<'a, str>,
    entries: Vec<SearchEntry<T>>,
    actions: Vec<OverlayAction>,
    armed_row: Option<T>,
    selected: Option<usize>,
    initial_query: Cow<'a, str>,
    empty_text: Option<Cow<'a, str>>,
    preserve_groups: Option<bool>,
    on_query_change: Option<Callback<Arc<str>>>,
    on_select: Option<Callback<SearchEvent<T>>>,
    on_activate: Option<Callback<SearchEvent<T>>>,
    render_item: Option<OverlayItemRenderer<T>>,
    item_gutter: Option<OverlayGutterRenderer<T>>,
    fallback_interceptor: Option<KeyHandler>,
    tabs: Option<OverlayTabs>,
}

impl<'a, T: Clone + PartialEq + 'static> OverlayPalette<'a, T> {
    pub(crate) fn new(
        title: impl Into<Cow<'a, str>>,
        key: &'static str,
        close: Msg,
        width: u16,
    ) -> Self {
        Self {
            title: title.into(),
            header_right: None,
            key,
            close,
            width,
            placeholder: Cow::Borrowed("Search…"),
            entries: Vec::new(),
            actions: Vec::new(),
            armed_row: None,
            selected: None,
            initial_query: Cow::Borrowed(""),
            empty_text: None,
            preserve_groups: None,
            on_query_change: None,
            on_select: None,
            on_activate: None,
            render_item: None,
            item_gutter: None,
            fallback_interceptor: None,
            tabs: None,
        }
    }

    /// Show `tabs` above the query. `entries`, `selected`, and `initial_query` then describe the
    /// active page only.
    pub(crate) fn tabs(mut self, tabs: OverlayTabs) -> Self {
        self.tabs = Some(tabs);
        self
    }

    pub(crate) fn placeholder(mut self, placeholder: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub(crate) fn header_right(mut self, header: impl Into<Cow<'a, str>>) -> Self {
        self.header_right = Some(header.into());
        self
    }

    pub(crate) fn entries(mut self, entries: Vec<SearchEntry<T>>) -> Self {
        self.entries = entries;
        self
    }

    pub(crate) fn actions(mut self, actions: Vec<OverlayAction>) -> Self {
        self.actions = actions;
        self
    }

    pub(crate) fn armed_row(mut self, armed_row: Option<T>) -> Self {
        self.armed_row = armed_row;
        self
    }

    pub(crate) fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    pub(crate) fn initial_query(mut self, query: impl Into<Cow<'a, str>>) -> Self {
        self.initial_query = query.into();
        self
    }

    pub(crate) fn empty_text(mut self, text: impl Into<Cow<'a, str>>) -> Self {
        self.empty_text = Some(text.into());
        self
    }

    pub(crate) fn preserve_groups(mut self, preserve: bool) -> Self {
        self.preserve_groups = Some(preserve);
        self
    }

    pub(crate) fn on_query_change(mut self, callback: Callback<Arc<str>>) -> Self {
        self.on_query_change = Some(callback);
        self
    }

    pub(crate) fn on_select(mut self, callback: Callback<SearchEvent<T>>) -> Self {
        self.on_select = Some(callback);
        self
    }

    pub(crate) fn on_activate(mut self, callback: Callback<SearchEvent<T>>) -> Self {
        self.on_activate = Some(callback);
        self
    }

    pub(crate) fn render_item(mut self, renderer: OverlayItemRenderer<T>) -> Self {
        self.render_item = Some(renderer);
        self
    }

    pub(crate) fn item_gutter(mut self, renderer: OverlayGutterRenderer<T>) -> Self {
        self.item_gutter = Some(renderer);
        self
    }

    pub(crate) fn fallback_interceptor(mut self, interceptor: KeyHandler) -> Self {
        self.fallback_interceptor = Some(interceptor);
        self
    }

    pub(crate) fn render(self, ctx: &Context<AppRoot>) -> Element {
        let Self {
            title,
            header_right,
            key,
            close,
            width,
            placeholder,
            entries,
            actions,
            armed_row,
            selected,
            initial_query,
            empty_text,
            preserve_groups,
            on_query_change,
            on_select,
            on_activate,
            render_item,
            item_gutter,
            fallback_interceptor,
            tabs,
        } = self;

        let confirm = armed_row.as_ref().and_then(|_| {
            actions
                .iter()
                .filter(|action| action.enabled)
                .find_map(|action| action.confirm.clone())
        });
        let has_gutter = item_gutter.is_some();
        let (cap_left, _) = crate::view::picker_selection_cap_glyphs(&ctx.state.config);
        let mut palette = shared_search_palette::<T>(ctx, Length::Auto, false)
            .entries(entries)
            .placeholder(placeholder.into_owned())
            .initial_query(initial_query.into_owned())
            .initial_selected_item_index(selected)
            .sync_selection(true);
        palette = apply_palette_options(
            palette,
            empty_text,
            preserve_groups,
            on_query_change,
            on_select,
            on_activate,
            item_gutter,
        );
        if has_gutter {
            palette = palette
                .list_item_horizontal_padding((0, 1, 0, 0))
                .empty_text_padding((0, 0, 0, 1))
                .list_unselected_symbol(" ");
            if cap_left.is_empty() {
                let pad_symbol_style = picker_selection_style(&ctx.state.theme, None);
                palette = palette
                    .list_selection_symbol(" ")
                    .list_selection_symbol_style(pad_symbol_style)
                    .list_unfocused_selection_symbol_style(pad_symbol_style);
            }
        }
        palette = apply_item_rendering(
            palette,
            &ctx.state.theme,
            armed_row,
            confirm,
            render_item,
            cap_left.is_empty(),
        );

        // Caller actions come first, so a producer may claim Tab for itself.
        let mut interceptors = vec![overlay_interceptor(ctx, &actions)];
        interceptors.extend(tabs.as_ref().map(|tabs| tabs.interceptor(ctx)));
        interceptors.extend(fallback_interceptor);
        let interceptor = if interceptors.len() == 1 {
            interceptors.remove(0)
        } else {
            KeyHandler::new(move |key| interceptors.iter().any(|handler| handler.handle(key)))
        };
        palette = palette.input_key_interceptor(interceptor);

        let mut body = VStack::new().height(Length::Auto);
        if let Some(tabs) = tabs {
            let select = tabs.select;
            let labels = tabs.labels.iter().map(String::as_str).collect::<Vec<_>>();
            body = body.child(picker_tabs(
                ctx,
                &labels,
                tabs.active,
                ctx.link()
                    .callback(move |event: TabsEvent| select(event.index)),
            ));
            // Keyed per page: the query field and highlight are seeded only on mount, and each
            // page brings its own.
            let palette: Element = palette.into();
            body = body.child(palette.key(format!("{key}-page-{}", tabs.active)));
        } else {
            body = body.child(palette);
        }
        if actions.iter().any(OverlayAction::shows_hint) {
            body = body.child(overlay_hints(&ctx.state.theme, &actions));
        }

        wrap_palette(ctx, title, header_right, key, close, body, width)
    }
}

pub(super) fn apply_palette_options<T: Clone + PartialEq + 'static>(
    mut palette: SearchPalette<T>,
    empty_text: Option<Cow<'_, str>>,
    preserve_groups: Option<bool>,
    on_query_change: Option<Callback<Arc<str>>>,
    on_select: Option<Callback<SearchEvent<T>>>,
    on_activate: Option<Callback<SearchEvent<T>>>,
    item_gutter: Option<OverlayGutterRenderer<T>>,
) -> SearchPalette<T> {
    if let Some(text) = empty_text {
        palette = palette.empty_text(text.into_owned());
    }
    if let Some(preserve) = preserve_groups {
        palette = palette.preserve_groups(preserve);
    }
    if let Some(callback) = on_query_change {
        palette = palette.on_query_change(callback);
    }
    if let Some(callback) = on_select {
        palette = palette.on_select(callback);
    }
    if let Some(callback) = on_activate {
        palette = palette.on_activate(callback);
    }
    if let Some(gutter) = item_gutter {
        palette = palette.item_gutter(gutter);
    }
    palette
}

pub(super) fn apply_item_rendering<T: Clone + PartialEq + 'static>(
    mut palette: SearchPalette<T>,
    theme: &Theme,
    armed_row: Option<T>,
    confirm: Option<ConfirmCue>,
    render_item: Option<OverlayItemRenderer<T>>,
    padded_selection: bool,
) -> SearchPalette<T> {
    if let Some(confirm) = confirm.as_ref() {
        let selection_style = picker_selection_style(theme, Some(confirm.accent));
        palette = palette
            .list_selection_style(selection_style)
            .list_unfocused_selection_style(selection_style);
        palette = if padded_selection {
            palette
                .list_selection_symbol_style(selection_style)
                .list_unfocused_selection_symbol_style(selection_style)
        } else {
            palette.list_selection_symbol_style(crate::view::picker_selection_cap_style(
                theme,
                confirm.accent,
            ))
        };
    }
    if armed_row.is_none() && render_item.is_none() {
        return palette;
    }
    palette.render_item(Arc::new(move |item, highlight| {
        if armed_row.as_ref() == Some(&item.value)
            && let Some(confirm) = confirm.as_ref()
        {
            return Some(render_pending_confirm_item(
                item.label.as_ref(),
                confirm.accent,
                &confirm.cue,
                confirm.strike,
            ));
        }
        render_item
            .as_ref()
            .and_then(|renderer| renderer(item, highlight))
    }))
}

pub(super) fn wrap_palette(
    ctx: &Context<AppRoot>,
    title: Cow<'_, str>,
    header_right: Option<Cow<'_, str>>,
    key: &'static str,
    close: Msg,
    body: VStack,
    width: u16,
) -> Element {
    let Some(header_right) = header_right else {
        return action_palette(ctx, &title, key, close, body, width);
    };
    let panel: Element = Frame::new()
        .header_left(title.into_owned())
        .header_right(header_right.into_owned())
        .header_style(ctx.state.theme.accent.bold())
        .border_style(overlay_border_style(ctx))
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(body)
        .into();
    Modal::new()
        .width(Length::Px(width))
        .height(Length::Auto)
        .max_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .reserve_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(panel)
        .key(key)
}

/// Shared category navigation. Query inputs retain keyboard focus while tabs accept clicks.
pub(super) fn picker_tabs(
    ctx: &Context<AppRoot>,
    labels: &[&str],
    active: usize,
    on_change: Callback<TabsEvent>,
) -> Element {
    let theme = &ctx.state.theme;
    let host = theme.surface.element;
    let strip = if ctx.state.config.pane.picker_tab_background {
        crate::view::strip_background(theme, false, host)
    } else {
        host
    };
    let caps = ctx
        .state
        .config
        .effective_cap_style(ctx.state.config.pane.picker_tab_style)
        .glyphs()
        .and_then(|(left, right)| Some((left.chars().next()?, right.chars().next()?)));
    DraggableTabBar::new()
        .tabs(labels.iter().map(|label| DraggableTab::new(*label)))
        .active(active)
        .draggable(false)
        .focusable(false)
        .tab_stop(false)
        .show_close_buttons(false)
        .height(Length::Px(1))
        .divider(' ')
        .caps(caps)
        .overflow_left_label(|_| Arc::from("❮ "))
        .overflow_right_label(|_| Arc::from(" ❯"))
        .overflow_style(Style::new().fg(theme.border_active))
        .overflow_hover_style(
            Style::new()
                .fg(theme.border_active)
                .bg(strip.elevate_by(0.08)),
        )
        .style(
            Style::new()
                .fg(crate::ops::theme::chrome_label_fg(theme, strip))
                .bg(strip),
        )
        .active_style({
            let active_bg = theme.border_active;
            Style::new()
                .fg(crate::ops::theme::chrome_label_fg(theme, active_bg))
                .bg(active_bg)
                .bold()
        })
        .tab_hover_style(Style::new().transform_bg(crate::view::hover_lift()))
        .on_change(on_change)
        .into()
}

pub(super) fn picker_divider(theme: &Theme) -> Element {
    Divider::horizontal()
        .join_frame(false)
        .style(fg_only(&theme.border))
        .into()
}

/// Frame shared by the tabbed Keybindings and Settings pickers.
pub(super) fn tabbed_picker_panel(
    ctx: &Context<AppRoot>,
    title: &str,
    height: Length,
    body: Element,
) -> Element {
    Frame::new()
        .header_left(title)
        .header_style(ctx.state.theme.accent.bold())
        .border(true)
        .border_style(overlay_border_style(ctx))
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .padding(0)
        .height(height)
        .child(body)
        .into()
}

/// Next selectable row after moving `delta` steps. Up/Down wrap like SearchPalette; Page/Home/End
/// still stop at the ends so a long grouped list does not jump a whole viewport on PageUp.
pub(super) fn stepped_selectable_row<T>(
    targets: &[Option<T>],
    current: Option<usize>,
    delta: isize,
    wrap: bool,
) -> Option<usize> {
    let matches: Vec<bool> = targets.iter().map(|target| target.is_some()).collect();
    List::step_matching(&matches, current.unwrap_or(0), delta, wrap)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selectable_rows_wrap_on_arrows_and_clamp_on_paging() {
        let targets = [None, Some("a"), None, Some("b"), Some("c")];
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(1), -1, true),
            Some(4)
        );
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(4), 1, true),
            Some(1)
        );
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(1), -1, false),
            Some(1)
        );
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(4), 1, false),
            Some(4)
        );
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(3), isize::MIN, false),
            Some(1)
        );
        assert_eq!(
            super::stepped_selectable_row(&targets, Some(1), isize::MAX, false),
            Some(4)
        );
    }
}
