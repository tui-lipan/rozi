# Widget Selection

Check for project-local wrapper widgets, re-exports, and theme helpers before reaching for raw tui-lipan widgets. App repos often wrap common panels, lists, inputs, and dialogs.

## Pick By Use Case

- Layout: `VStack`, `HStack`, `Grid`, `ScrollView`, `Splitter`, `ZStack`, `Center`
- Chrome: `Frame` when you need border, title, status, tabs, or clipping
- Text and display: `Text`, `DocumentView`, `Chart`, `Sparkline`, `Heatmap`, `BigText`, `Image`
- Input: `Button`, `Input`, `TextArea`, `Checkbox`, `Radio`, `Select`, `ComboBox`, `MultiSelect`, `Slider`, `DatePicker`, `HexArea`
- Data: `List`, `Table`, `Tree`, `FileTree`, `LogView`
- Feedback: `ProgressBar`, `Spinner`, `StatusBar`, `PaginationBar`, `Breadcrumb`, `Badge`
- Overlays: `Modal`, `Popover`, `Tooltip`, `Accordion`, `SearchPalette`, `ContextMenu`
- Tabs: `Tabs`, `DraggableTabBar`
- Terminal: `ManagedTerminal` first, then lower-level terminal widgets only when necessary

## Default Selection Heuristics

- Use `List` for most selectable collections.
- Use `Table` for structured records and inspector-like views.
- Use `ComboBox` for searchable single-select.
- Use `MultiSelect` for multi-pick flows.
- Use `SearchPalette` for command-palette style search; wrap it in `Modal` if you want overlay behavior.
- Use `Splitter` for IDE-style shells.
- Use `ScrollView` only when a widget does not already manage its own scrolling.

Prefer the smallest abstraction that matches the repo's existing style:

- raw widget when the project uses widgets directly in `ui!`
- helper function when only styling or chrome is shared
- wrapper or composite widget when configuration is repeated across screens

## Event Payload Gotchas

- `List` callbacks receive `ListEvent`; use `.index`.
- `ComboBoxCommitEvent.index` can be `None` for custom values.
- `FileTree` events include a `PathBuf`; key app logic off paths rather than visible row indices.
- `LogViewEvent` distinguishes visible and source indices.
- `Button::on_click` also covers focused keyboard activation.
- `Hyperlink::on_activate` does not open the URL for you.
- `Input` and `TextArea` cursor positions are byte offsets.

## Defaults Worth Remembering

- Container width and height default to `Length::Flex(1)`.
- Leaf widgets generally default to `Length::Auto`.
- `Align::Start` and `Justify::Start` are defaults.
- Interactive widgets are usually focusable by default.
- `TextArea.wrap(true)` is the default.
- `Modal` renders as a root-level portal by default.
- Toasts are driven through `ctx.toast()` rather than normal tree rendering.
- `ManagedTerminal` is the recommended terminal entry point.

## Feature Flags

- `clipboard` is enabled by default for system clipboard integration
- `devtools` for the in-app DevTools overlay and `Context` controls
- `clipboard-images` for image clipboard read/write without the `Image` widget
- `big-text` for `BigText`
- `diff-view` for `DiffView`
- `image` for `Image` and clipboard image support
- `markdown` for markdown formatting in `DocumentView`
- `profiling-tracing` for tracing spans/events around render and document hot paths
- `syntax-syntect` for syntax highlighting in `TextArea` and `DiffView`
- `terminal` for terminal widgets
- `theme-reload` for runtime theme file reload during development
- `web` for the browser/WASM backend

Check `docs/quick-start.md` before using feature-gated widgets.

If you are outside the framework repo, verify feature flags in `Cargo.toml`, lockfile versions, and local docs before assuming a widget is available.
