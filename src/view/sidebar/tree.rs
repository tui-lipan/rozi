use tui_lipan::prelude::*;

use crate::config::{SidebarTreeConfig, SidebarTreeRoot, SidebarTreeView};
use crate::{AppRoot, Msg};

/// The file-tree tabs. `Files` browses the focused pane's directory; `Changes` shows only what git
/// reports as changed. They are one widget configured two ways, so the change projection is the
/// only real difference between them.
///
/// Neither tab is wrapped in the sidebar's scroll view — `FileTree` scrolls internally, and nesting
/// the two would leave the wheel fighting over which one moves.
pub(super) fn tree_tab(
    ctx: &Context<AppRoot>,
    panel: usize,
    view: SidebarTreeView,
    config: &SidebarTreeConfig,
) -> Element {
    let theme = &ctx.state.theme;
    let Some(root) = root_for(ctx, config.root) else {
        return super::placeholder(ctx, empty_reason(ctx, config.root));
    };
    // A remote server older than the file-tree messages will never answer a listing request, so say
    // so rather than leaving the widget on loading rows forever.
    if ctx.state.current().remote_host.is_some()
        && ctx
            .state
            .current()
            .session_client
            .as_ref()
            .is_some_and(|client| !client.supports_file_tree())
    {
        return super::placeholder(ctx, "Remote rozi is too old to browse files");
    }

    let changed_only = view == SidebarTreeView::Changes;
    let tab_id = crate::config::SidebarTabId::new(view.id());
    let toggle_tab_id = tab_id.clone();
    let config_epoch = ctx.state.sidebar.config_epoch;
    let tree_focus_key = tree_focus_key(panel, view, &root);
    let explorer_focus_key = format!("{}-explorer", tree_key(panel, view, &root));

    // The same lift the composed row lists use, so the cursor means one thing across every tab.
    let selection = super::row_highlight(theme);

    let mut tree = FileTree::new(root.clone())
        .show_hidden(config.show_hidden)
        .tree_focus_key(tree_focus_key)
        .explorer_focus_key(explorer_focus_key)
        // Keep the tree's file rows icon-free, but reserve its icon gutter for directory state.
        .show_icons(true)
        .directory_icon("")
        .opened_directory_icon("")
        .file_icon("")
        .symlink_icon("")
        .other_icon("")
        .max_entries_per_dir(config.max_entries)
        .indent_style(IndentStyle::Short)
        .indent_width(1)
        .indent_guide_style(Style::new().fg(theme.surface.element.elevate_by(0.15)))
        .change_view(if changed_only {
            FileTreeChangeView::ChangedOnly
        } else {
            FileTreeChangeView::AllFiles
        })
        .show_diff_stats(config.diff_stats)
        // Text markers (`M`, `A`, `?`) rather than the widget's default Nerd Font glyphs: rozi
        // cannot assume the user's terminal font has them, and the rest of the sidebar is text.
        .git_icon_style(GitIconStyle::Text)
        .git_refresh_token(ctx.state.sidebar.git_refresh_token)
        // Focusable so the selected row can be a real keyboard cursor, but never a tab stop: the
        // focus ring belongs to the panes, and the enclosing `FocusScope::Exclude` additionally
        // keeps clicks from focusing it. `focus-sidebar` is the only way in.
        .focusable(true)
        .tab_stop(false)
        .on_focus(ctx.link().callback(|_| Msg::SidebarTreeFocused))
        // Bare arrows and h/l expand or collapse; modified arrows remain available to the sidebar
        // for tab movement and resizing.
        .keymap(TreeKeymap::ARROWS | TreeKeymap::VIM | TreeKeymap::TOGGLE)
        .directory_label_style(super::super::fg_only(&theme.accent))
        .file_label_style(super::super::fg_only(&theme.primary))
        // Semantic change colors from the theme's git palette. The widget's own defaults are empty
        // styles — it never reads `theme.git_status` itself — so without this every marker and diff
        // stat renders in the plain row color. These cover the `M`/`A`/`D`/`R`/`?`/`!` markers and
        // the `+N -M` stats, which take the added/deleted colors.
        .git_style_modified(Style::new().fg(theme.git_status.modified))
        .git_style_added(Style::new().fg(theme.git_status.added))
        .git_style_deleted(Style::new().fg(theme.git_status.deleted))
        .git_style_renamed(Style::new().fg(theme.git_status.renamed))
        .git_style_untracked(Style::new().fg(theme.git_status.untracked))
        .git_style_conflicted(Style::new().fg(theme.git_status.conflicted))
        .scrollbar_config(super::scrollbar_config())
        // On a narrow sidebar the change markers are the point of the Changes tab, so they survive
        // truncation there; browsing instead keeps the filename readable.
        .change_suffix_priority(if changed_only {
            FileTreeSuffixPriority::Suffix
        } else {
            FileTreeSuffixPriority::Label
        })
        .empty_text(if changed_only {
            changes_empty_text(ctx)
        } else {
            "Directory is empty"
        })
        .empty_text_style(super::super::fg_only(&theme.muted))
        // The tree's rows are flush with the panel, but this is a message, not a row: it lines up
        // with every other tab's empty state instead — the same inset `placeholder` uses.
        .empty_text_padding(super::PLACEHOLDER_PADDING)
        .item_hover_style(super::row_highlight(theme))
        .selection_style(selection)
        // With the keyboard elsewhere the cursor leaves no mark: a click here is a one-shot action,
        // not a "this row is now current" state, and a highlight parked on the last-clicked file
        // claims otherwise. Hover is what gives the mouse its feedback.
        .unfocused_selection_style(Style::default())
        // A single click both expands a directory and, in the widget, "activates" it. Activation is
        // what runs the tab's action, and running it for a directory would type the directory's
        // path at the prompt just because it was opened. Only files carry the action; the widget
        // expands directories on its own, so filtering them here costs no expand behavior. The
        // widget stack has no double-click, so single-click-on-file is the activation gesture — safe
        // because the default action types the path without a newline and nothing runs on its own.
        .on_activate(
            ctx.link()
                .callback(move |event: FileTreeEvent| Msg::SidebarTreeActivate {
                    config_epoch,
                    tab_id: tab_id.clone(),
                    path: event.path.to_string(),
                    is_dir: event.kind == FileKind::Directory,
                }),
        );

    // Expansion is the app's to remember, because the widget's own cannot survive: the tab keys on
    // its root, so every pane in another directory remounts the tree from nothing. Recording each
    // toggle and seeding the next mount from it is what makes a re-rooted or reopened tab come back
    // the shape it was left in. The seed is only read on mount and on a root change, so writing to
    // it here never fights the expansion on screen.
    tree =
        tree.on_toggle(ctx.link().callback(move |event: FileTreeToggleEvent| {
            Msg::SidebarTreeToggle {
                tab_id: toggle_tab_id.clone(),
                path: event.path.to_string(),
                expanded: event.expanded,
            }
        }))
        .initial_expanded_paths(remembered_expanded(ctx, view));

    if config.icons {
        tree = tree.icon_style(FileIconStyle::NerdFont);
    }

    // Under `--remote` the files are on the server's host, so the widget cannot enumerate them
    // itself. Serve listings from state instead and let it ask for what it is missing; local
    // attaches keep the default filesystem source and never pay for any of this.
    if ctx.state.current().remote_host.is_some() {
        tree = tree
            .entry_source(FileTreeEntrySource::provided(
                ctx.state.sidebar.tree_listings.clone(),
            ))
            .on_entry_request(ctx.link().callback(|request: FileTreeEntryRequest| {
                Msg::SidebarTreeEntryRequest {
                    path: request.path.to_string(),
                }
            }))
            // Local git discovery would walk this client's filesystem using the *server's* paths,
            // so the Changes projection has to come from the server's own scan too.
            .change_source(FileTreeChangeSource::Provided(
                ctx.state.sidebar.tree_changes.clone(),
            ));
    }

    if config.explorer {
        tree = tree
            .explorer(true)
            .explorer_placeholder("Find files…")
            .explorer_match_style(super::super::fg_only(&theme.accent).bold())
            .on_explorer_focus(
                ctx.link()
                    .callback(|origin| Msg::SidebarExplorerFocus(Some(origin))),
            )
            .on_explorer_blur(ctx.link().callback(|_| Msg::SidebarExplorerFocus(None)))
            .on_explorer_escape(ctx.link().callback(|_| Msg::SidebarBlur));
    }

    tree.key(tree_key(panel, view, &root))
}

/// What the Changes tab says when it is showing nothing. A repo-less directory is worth naming —
/// "No changes" there reads as a clean tree rather than as nothing to be clean about — and a remote
/// scan still in flight has no answer yet, so it must not claim one.
fn changes_empty_text(ctx: &Context<AppRoot>) -> &'static str {
    let sidebar = &ctx.state.sidebar;
    if ctx.state.current().remote_host.is_some() {
        // Cleared on every root change, so `Some` means the server has answered for this root.
        return if sidebar.tree_changes_root.is_some() {
            "No changes"
        } else {
            "Loading changes…"
        };
    }
    if sidebar.tree_repo.is_none() {
        return "Not a git repository";
    }
    "No changes"
}

/// The directories this tab had expanded when its tree was last on screen. Absolute paths, so the
/// widget keeps the ones under whatever root it is mounting at and ignores the rest.
fn remembered_expanded(ctx: &Context<AppRoot>, view: SidebarTreeView) -> Vec<String> {
    ctx.state
        .sidebar
        .tree_expanded
        .get(&crate::config::SidebarTabId::new(view.id()))
        .map(|paths| paths.iter().cloned().collect())
        .unwrap_or_default()
}

/// Keyed on the root so switching projects remounts the tree rather than reusing another
/// directory's selection state. Expansion is not lost with it: the tab remembers what was open and
/// seeds the new tree with it. Built here rather than inline so `focus-sidebar` can aim at the same
/// key the view is about to render.
pub(super) fn tree_key(panel: usize, view: SidebarTreeView, root: &str) -> String {
    format!(
        "{}-{panel}-{}-{root}",
        crate::view::sidebar_body_key(),
        view.id()
    )
}

pub(super) fn tree_focus_key(panel: usize, view: SidebarTreeView, root: &str) -> String {
    format!("{}-tree", tree_key(panel, view, root))
}

/// The directory a tab is rooted at, exposed so the focus key can be derived without rendering.
pub(super) fn tree_root(ctx: &Context<AppRoot>, config: &SidebarTreeConfig) -> Option<String> {
    root_for(ctx, config.root)
}

/// The directory a tab is rooted at. `Repo` falls back to the working directory when it is not
/// inside a repository, so the Changes tab degrades to "nothing changed here" instead of vanishing.
fn root_for(ctx: &Context<AppRoot>, root: SidebarTreeRoot) -> Option<String> {
    let sidebar = &ctx.state.sidebar;
    match root {
        SidebarTreeRoot::Cwd => sidebar.tree_cwd.clone(),
        SidebarTreeRoot::Repo => sidebar
            .tree_repo
            .clone()
            .or_else(|| sidebar.tree_cwd.clone()),
    }
}

/// Why there is no tree to show. A focused pane whose directory is not on this client (OSC7
/// `cwd_host`, or a `--remote` session) is worth saying plainly rather than showing an empty tree.
fn empty_reason(ctx: &Context<AppRoot>, root: SidebarTreeRoot) -> &'static str {
    let remote = ctx.state.current().remote_host.is_some()
        || ctx
            .state
            .current()
            .focused_pane
            .and_then(|id| crate::pane_lifecycle::find_pane(&ctx.state, id))
            .is_some_and(|pane| pane.terminal.cwd_host.is_some());
    match (remote, root) {
        // Under `--remote` the tree is served by the session server, so the only way to have no
        // root is the pane not having reported a directory yet.
        (true, _) if ctx.state.current().remote_host.is_some() => {
            "Waiting for the remote pane's directory"
        }
        (true, _) => "Pane is on a remote host",
        (false, SidebarTreeRoot::Repo) => "No repository here",
        (false, SidebarTreeRoot::Cwd) => "No working directory",
    }
}
