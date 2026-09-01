use std::borrow::Cow;

pub(crate) type OverlayItemRenderer<T> =
    Arc<dyn Fn(&SearchItem<T>, &SearchHighlight) -> Option<ListItem>>;
pub(crate) type OverlayGutterRenderer<T> =
    Arc<dyn Fn(&SearchItem<T>, &SearchHighlight) -> Option<ListItemGutter>>;

fn picker_description(description: impl AsRef<str>) -> ItemDescription {
    ItemDescription::new().right(format!("  {}", description.as_ref()))
}

fn picker_row(
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
fn picker_selection_style(theme: &Theme, pending_accent: Option<Color>) -> Style {
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
fn render_pending_confirm_item(
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
            confirm: None,
        })
    }

    /// Show the action in the footer but let SearchPalette activate the visible row.
    pub(crate) fn hint_only(mut self) -> Self {
        self.intercept = false;
        self
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
    for action in actions.iter().filter(|action| action.enabled) {
        any = true;
        row = row.child(hint_pill(
            theme,
            &action.label,
            &crate::view::keys_display::format_binding(&action.key),
        ));
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
    element_key: Option<String>,
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
            placeholder: Cow::Borrowed("Search..."),
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
            element_key: None,
        }
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

    pub(crate) fn element_key(mut self, key: impl Into<String>) -> Self {
        self.element_key = Some(key.into());
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
            element_key,
        } = self;

        let confirm = armed_row.as_ref().and_then(|_| {
            actions
                .iter()
                .filter(|action| action.enabled)
                .find_map(|action| action.confirm.clone())
        });
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
        palette = apply_item_rendering(
            palette,
            &ctx.state.theme,
            armed_row,
            confirm,
            render_item,
        );

        let action_interceptor = overlay_interceptor(ctx, &actions);
        let interceptor = if let Some(fallback) = fallback_interceptor {
            KeyHandler::new(move |key| action_interceptor.handle(key) || fallback.handle(key))
        } else {
            action_interceptor
        };
        palette = palette.input_key_interceptor(interceptor);

        let palette: Element = if let Some(element_key) = element_key {
            Element::from(palette).key(element_key)
        } else {
            palette.into()
        };
        let mut body = VStack::new().height(Length::Auto).child(palette);
        if actions.iter().any(|action| action.enabled) {
            body = body.child(overlay_hints(&ctx.state.theme, &actions));
        }

        wrap_palette(ctx, title, header_right, key, close, body, width)
    }
}

fn apply_palette_options<T: Clone + PartialEq + 'static>(
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

fn apply_item_rendering<T: Clone + PartialEq + 'static>(
    mut palette: SearchPalette<T>,
    theme: &Theme,
    armed_row: Option<T>,
    confirm: Option<ConfirmCue>,
    render_item: Option<OverlayItemRenderer<T>>,
) -> SearchPalette<T> {
    if let Some(confirm) = confirm.as_ref() {
        let selection_style = picker_selection_style(theme, Some(confirm.accent));
        palette = palette
            .list_selection_style(selection_style)
            .list_unfocused_selection_style(selection_style);
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

fn wrap_palette(
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
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(action_palette_frame(body))
        .into();
    Modal::new()
        .width(Length::Px(width))
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(panel)
        .key(key)
}
