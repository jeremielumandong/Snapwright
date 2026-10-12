//! The library window: every capture as a thumbnail grid with search, tags and trash, and a
//! detail pane for the selected capture.
//!
//! Queries run on a background thread against the shared [`Library`]; results land here
//! tagged with a generation number so a slow, stale query never overwrites a newer one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, FontWeight,
    Global, InteractiveElement as _, IntoElement, KeyDownEvent, ObjectFit, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Subscription, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    div, img, px, size, uniform_list,
};
use snapwright_library::{Capture, Library, Query, Source};

use crate::session::{self, LibraryService};

const SIDEBAR_W: f32 = 200.;
const DETAIL_W: f32 = 320.;
const CELL_W: f32 = 196.;
const CELL_H: f32 = 188.;
const THUMB_H: f32 = 120.;
const GAP: f32 = 12.;

/// Bumped whenever the library changes outside this window (a new capture), so an open
/// library window reloads.
#[derive(Default)]
pub struct LibraryChanged(pub u64);
impl Global for LibraryChanged {}

/// Records that the library changed.
pub fn notify_changed(cx: &mut App) {
    let next = cx.default_global::<LibraryChanged>().0 + 1;
    cx.set_global(LibraryChanged(next));
}

/// The open library window, if any.
#[derive(Default)]
struct LibraryWindow(Option<AnyWindowHandle>);
impl Global for LibraryWindow {}

/// A capture the library window should select once it has loaded.
#[derive(Default)]
struct PendingSelect(Option<i64>);
impl Global for PendingSelect {}

/// Opens the library window, or brings it to the front.
pub fn open(cx: &mut App) {
    open_at(None, cx);
}

/// Opens the library window with capture `id` selected.
pub fn open_at(id: Option<i64>, cx: &mut App) {
    if id.is_some() {
        cx.set_global(PendingSelect(id));
        // An open window reloads on this and picks the selection up.
        notify_changed(cx);
    }
    if let Some(h) = cx.default_global::<LibraryWindow>().0
        && h.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let Some(library) = cx.global::<LibraryService>().0.clone() else {
        tracing::error!("the capture library is unavailable");
        return;
    };
    let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(860.), px(480.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Snapwright Library".into()),
            ..Default::default()
        }),
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| LibraryView::new(library, window, cx))
    }) {
        Ok((handle, _)) => cx.set_global(LibraryWindow(Some(handle))),
        Err(e) => tracing::error!(error = %e, "could not open the library window"),
    }
}

/// What the grid shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    All,
    Tag(String),
    Trash,
}

/// A capture with its file paths, resolved off the UI thread.
#[derive(Clone)]
struct Item {
    capture: Capture,
    image: PathBuf,
    thumb: PathBuf,
}

struct Loaded {
    items: Vec<Item>,
    tags: Vec<(String, usize)>,
    total: usize,
    trashed: usize,
}

pub struct LibraryView {
    library: Arc<Mutex<Library>>,
    search: Entity<InputState>,
    new_tag: Entity<InputState>,
    note: Entity<InputState>,
    filter: Filter,
    items: Vec<Item>,
    tags: Vec<(String, usize)>,
    total: usize,
    trashed: usize,
    selected: Option<i64>,
    /// The note field shows another capture's note; refreshed on the next render.
    note_stale: bool,
    generation: u64,
    grid_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Item to scroll into view on the next render.
    scroll_to: Option<usize>,
    /// Clear the search box on the next render (which then reloads).
    search_reset: bool,
    _subs: Vec<Subscription>,
}

impl LibraryView {
    fn new(library: Arc<Mutex<Library>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search apps, titles, tags, notes…"));
        let new_tag = cx.new(|cx| InputState::new(window, cx).placeholder("Add tag"));
        let note = cx.new(|cx| InputState::new(window, cx).placeholder("Note (Enter saves)"));
        let subs = vec![
            cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.reload(cx);
                }
            }),
            cx.subscribe_in(
                &new_tag,
                window,
                |this, input, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        let name = input.read(cx).value().to_string();
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.edit(cx, move |lib, id| lib.tag(id, &name));
                    }
                },
            ),
            cx.subscribe_in(&note, window, |this, input, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    this.edit(cx, move |lib, id| lib.set_note(id, &text));
                }
            }),
            cx.observe_global_in::<LibraryChanged>(window, |this, _, cx| this.reload(cx)),
        ];
        let grid_focus = cx.focus_handle();
        window.focus(&grid_focus, cx);
        let mut view = Self {
            library,
            search,
            new_tag,
            note,
            filter: Filter::All,
            items: Vec::new(),
            tags: Vec::new(),
            total: 0,
            trashed: 0,
            selected: None,
            note_stale: false,
            generation: 0,
            grid_focus,
            scroll: UniformListScrollHandle::new(),
            scroll_to: None,
            search_reset: false,
            _subs: subs,
        };
        view.reload(cx);
        view
    }

    fn query(&self, cx: &App) -> Query {
        let text = self.search.read(cx).value().trim().to_owned();
        Query {
            text: (!text.is_empty()).then_some(text),
            tag: match &self.filter {
                Filter::Tag(t) => Some(t.clone()),
                _ => None,
            },
            trash: self.filter == Filter::Trash,
            ..Query::default()
        }
    }

    /// Re-runs the current query off the UI thread.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let query = self.query(cx);
        let library = self.library.clone();
        let load = cx.background_executor().spawn(async move {
            let lib = library
                .lock()
                .map_err(|_| anyhow::anyhow!("library poisoned by an earlier panic"))?;
            let items = lib
                .list(&query)?
                .into_iter()
                .map(|c| Item {
                    image: lib.image_path_of(&c),
                    thumb: lib.thumb_path_of(&c),
                    capture: c,
                })
                .collect();
            let total = lib.count(&Query::default())?;
            let trashed = lib.count(&Query {
                trash: true,
                ..Query::default()
            })?;
            anyhow::Ok(Loaded {
                items,
                tags: lib.tags()?,
                total,
                trashed,
            })
        });
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match loaded {
                    Ok(l) => {
                        if let Some(id) = cx.default_global::<PendingSelect>().0.take() {
                            match l.items.iter().position(|i| i.capture.id == id) {
                                Some(ix) => {
                                    this.selected = Some(id);
                                    this.note_stale = true;
                                    this.scroll_to = Some(ix);
                                }
                                // Not under this filter or search: show everything and look
                                // again.
                                None if this.filter != Filter::All
                                    || !this.search.read(cx).value().is_empty() =>
                                {
                                    this.filter = Filter::All;
                                    this.search_reset = true;
                                    cx.set_global(PendingSelect(Some(id)));
                                    cx.notify();
                                    return;
                                }
                                None => {}
                            }
                        }
                        this.items = l.items;
                        this.tags = l.tags;
                        this.total = l.total;
                        this.trashed = l.trashed;
                        if this
                            .selected
                            .is_some_and(|id| !this.items.iter().any(|i| i.capture.id == id))
                        {
                            this.selected = None;
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "loading the library failed"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Applies a change to the selected capture off the UI thread, then reloads.
    fn edit(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Library, i64) -> snapwright_library::Result<()> + Send + 'static,
    ) {
        let Some(id) = self.selected else {
            return;
        };
        let library = self.library.clone();
        let task = cx.background_executor().spawn(async move {
            let mut lib = library
                .lock()
                .map_err(|_| anyhow::anyhow!("library poisoned by an earlier panic"))?;
            change(&mut lib, id)?;
            anyhow::Ok(())
        });
        cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                tracing::error!(error = %e, "changing the capture failed");
            }
            let _ = this.update(cx, |this, cx| this.reload(cx));
        })
        .detach();
    }

    fn empty_trash(&mut self, cx: &mut Context<Self>) {
        let library = self.library.clone();
        let task = cx.background_executor().spawn(async move {
            let mut lib = library
                .lock()
                .map_err(|_| anyhow::anyhow!("library poisoned by an earlier panic"))?;
            anyhow::Ok(lib.purge_trash()?)
        });
        cx.spawn(async move |this, cx| {
            match task.await {
                Ok(n) => tracing::info!(removed = n, "emptied the trash"),
                Err(e) => tracing::error!(error = %e, "emptying the trash failed"),
            }
            let _ = this.update(cx, |this, cx| this.reload(cx));
        })
        .detach();
    }

    fn selected_item(&self) -> Option<&Item> {
        let id = self.selected?;
        self.items.iter().find(|i| i.capture.id == id)
    }

    fn select(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id);
        let note = self
            .selected_item()
            .map(|i| i.capture.note.clone())
            .unwrap_or_default();
        self.note.update(cx, |s, cx| s.set_value(note, window, cx));
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = None;
        self.reload(cx);
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            session::copy_file_to_clipboard(item.image.clone(), cx);
        }
    }

    fn trash_or_restore(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item() else {
            return;
        };
        if item.capture.trashed {
            self.edit(cx, |lib, id| lib.restore(id));
        } else {
            self.edit(cx, |lib, id| lib.trash(id));
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mods = e.keystroke.modifiers;
        match e.keystroke.key.as_str() {
            "delete" | "backspace" => self.trash_or_restore(cx),
            "c" if mods.secondary() => self.copy_selected(cx),
            "enter" => {
                if let Some(item) = self.selected_item() {
                    cx.open_with_system(&item.image);
                }
            }
            key @ ("left" | "right" | "up" | "down") => {
                let columns = self.columns(window);
                let step: isize = match key {
                    "left" => -1,
                    "right" => 1,
                    "up" => -(columns as isize),
                    _ => columns as isize,
                };
                let at = self
                    .selected
                    .and_then(|id| self.items.iter().position(|i| i.capture.id == id));
                let next = match at {
                    None => 0,
                    Some(i) => (i as isize + step).clamp(0, self.items.len() as isize - 1) as usize,
                };
                if let Some(id) = self.items.get(next).map(|i| i.capture.id) {
                    self.select(id, window, cx);
                    self.scroll
                        .scroll_to_item(next / columns, gpui_kit::ScrollStrategy::Center);
                }
            }
            _ => {}
        }
    }

    /// Thumbnails per grid row at the current window width.
    fn columns(&self, window: &Window) -> usize {
        let width = f32::from(window.viewport_size().width) - SIDEBAR_W - DETAIL_W - GAP * 2.;
        ((width + GAP) / (CELL_W + GAP)).floor().max(1.) as usize
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let row = |id: SharedString, label: String, active: bool| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme.sidebar_accent)
                        .text_color(theme.sidebar_accent_foreground)
                })
                .hover(|d| d.bg(theme.sidebar_accent))
                .child(label)
        };
        let mut side = div()
            .w(px(SIDEBAR_W))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                row(
                    "all".into(),
                    format!("All captures  {}", self.total),
                    self.filter == Filter::All,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_filter(Filter::All, cx))),
            )
            .child(
                row(
                    "trash".into(),
                    format!("Trash  {}", self.trashed),
                    self.filter == Filter::Trash,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_filter(Filter::Trash, cx))),
            );
        if !self.tags.is_empty() {
            side = side.child(
                div()
                    .mt(px(14.))
                    .mb(px(4.))
                    .px(px(10.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("TAGS"),
            );
        }
        for (i, (tag, n)) in self.tags.iter().enumerate() {
            let active = self.filter == Filter::Tag(tag.clone());
            let name = tag.clone();
            side = side.child(
                row(format!("tag-{i}").into(), format!("#{tag}  {n}"), active).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.set_filter(Filter::Tag(name.clone()), cx)
                    }),
                ),
            );
        }
        side
    }

    fn render_cell(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = &item.capture;
        let selected = self.selected == Some(c.id);
        let id = c.id;
        let caption = caption(c);
        div()
            .id(("capture", c.id as u64))
            .w(px(CELL_W))
            .h(px(CELL_H - GAP))
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .rounded(px(8.))
            .border_2()
            .border_color(if selected {
                theme.primary
            } else {
                gpui_kit::transparent_black()
            })
            .when(selected, |d| d.bg(theme.list_active))
            .hover(|d| d.bg(theme.list_hover))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, e: &gpui_kit::ClickEvent, window, cx| {
                    window.focus(&this.grid_focus, cx);
                    this.select(id, window, cx);
                    if e.click_count() >= 2
                        && let Some(item) = this.selected_item()
                    {
                        cx.open_with_system(&item.image);
                    }
                }),
            )
            .child(
                div()
                    .h(px(THUMB_H))
                    .w_full()
                    .rounded(px(4.))
                    .bg(theme.muted)
                    .overflow_hidden()
                    .child(
                        img(item.thumb.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.foreground)
                    .truncate()
                    .child(caption),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(format!(
                        "{}  ·  {}×{}",
                        when(c.created_ms),
                        c.width,
                        c.height
                    )),
            )
    }

    fn render_grid(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        if self.items.is_empty() {
            let message = match (&self.filter, self.search.read(cx).value().is_empty()) {
                (Filter::Trash, _) => "The trash is empty.".to_owned(),
                (_, false) => "No captures match.".to_owned(),
                _ => format!(
                    "No captures yet. Press {} to take one.",
                    if cfg!(target_os = "macos") {
                        "Cmd+Shift+2"
                    } else {
                        "Ctrl+Shift+2"
                    }
                ),
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(message)
                .into_any_element();
        }
        let columns = self.columns(window);
        let rows = self.items.len().div_ceil(columns);
        uniform_list(
            "grid",
            rows,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(this.items.len());
                        let cells: Vec<Item> = this.items[start..end].to_vec();
                        let rendered: Vec<_> = cells
                            .iter()
                            .map(|item| this.render_cell(item, cx).into_any_element())
                            .collect();
                        div()
                            .h(px(CELL_H))
                            .flex()
                            .gap(px(GAP))
                            .px(px(GAP))
                            .children(rendered)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .pt(px(GAP))
        .into_any_element()
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let pane = div()
            .w(px(DETAIL_W))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background);
        let Some(item) = self.selected_item() else {
            return pane
                .text_color(theme.muted_foreground)
                .text_size(px(13.))
                .child("Select a capture to see it here.")
                .into_any_element();
        };
        let c = &item.capture;
        let line = |label: &str, value: String| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(64.))
                        .text_color(theme.muted_foreground)
                        .child(label.to_owned()),
                )
                .child(div().flex_1().truncate().child(value))
        };
        let source = match c.meta.source {
            Source::Region => "Region",
            Source::Window => "Window",
            Source::Display => "Whole display",
            Source::Import => "Imported",
        };
        let mut tags = div().flex().flex_wrap().gap(px(6.));
        for (i, tag) in c.tags.iter().enumerate() {
            let name = tag.clone();
            tags = tags.child(
                div()
                    .id(("tag-chip", i))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .bg(theme.secondary)
                    .text_size(px(12.))
                    .child(format!("#{tag}"))
                    .child(
                        div()
                            .id(("untag", i))
                            .cursor_pointer()
                            .text_color(theme.muted_foreground)
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let name = name.clone();
                                this.edit(cx, move |lib, id| lib.untag(id, &name));
                            })),
                    ),
            );
        }
        pane.child(
            div()
                .h(px(200.))
                .w_full()
                .rounded(px(6.))
                .bg(theme.muted)
                .overflow_hidden()
                .child(
                    img(item.image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                ),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(caption(c)),
        )
        .child(line("Taken", when(c.created_ms)))
        .child(line("Size", format!("{} × {} px", c.width, c.height)))
        .child(line("File", human_bytes(c.bytes)))
        .child(line("Source", source.to_owned()))
        .when(!c.meta.url.is_empty(), |d| {
            d.child(line("URL", c.meta.url.clone()))
        })
        .child(tags)
        .child(Input::new(&self.new_tag).small())
        .child(Input::new(&self.note).small())
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .child(
                    Button::new("copy")
                        .primary()
                        .small()
                        .label("Copy")
                        .on_click(cx.listener(|this, _, _, cx| this.copy_selected(cx))),
                )
                .child(
                    Button::new("open")
                        .small()
                        .label("Open")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(item) = this.selected_item() {
                                cx.open_with_system(&item.image);
                            }
                        })),
                )
                .child(
                    Button::new("reveal")
                        .small()
                        .label("Show file")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(item) = this.selected_item() {
                                cx.reveal_path(&item.image);
                            }
                        })),
                )
                .child(
                    Button::new("trash-restore")
                        .small()
                        .when(!c.trashed, |b| b.danger())
                        .label(if c.trashed {
                            "Restore"
                        } else {
                            "Move to Trash"
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.trash_or_restore(cx))),
                ),
        )
        .into_any_element()
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.note_stale) {
            let note = self
                .selected_item()
                .map(|i| i.capture.note.clone())
                .unwrap_or_default();
            self.note.update(cx, |s, cx| s.set_value(note, window, cx));
        }
        if std::mem::take(&mut self.search_reset) {
            // Clearing emits a change, which reloads; reload anyway in case it was empty.
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.reload(cx);
        }
        if let Some(ix) = self.scroll_to.take() {
            let columns = self.columns(window);
            self.scroll
                .scroll_to_item(ix / columns, gpui_kit::ScrollStrategy::Center);
        }
        let theme = cx.theme();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(GAP))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.search).cleanable(true)),
            )
            .when(
                self.filter == Filter::Trash && !self.items.is_empty(),
                |d| {
                    d.child(
                        Button::new("empty-trash")
                            .danger()
                            .small()
                            .label("Empty Trash")
                            .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                    )
                },
            );
        div()
            .size_full()
            .flex()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .id("grid-area")
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .track_focus(&self.grid_focus)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .child(header)
                    .child(self.render_grid(window, cx)),
            )
            .child(self.render_detail(cx))
    }
}

/// The app and window title, or what kind of capture it was.
fn caption(c: &Capture) -> String {
    match (c.meta.app.is_empty(), c.meta.window_title.is_empty()) {
        (false, false) => format!("{} — {}", c.meta.app, c.meta.window_title),
        (false, true) => c.meta.app.clone(),
        (true, false) => c.meta.window_title.clone(),
        (true, true) => match c.meta.source {
            Source::Display => "Whole display".to_owned(),
            Source::Window => "Window".to_owned(),
            Source::Import => "Imported image".to_owned(),
            Source::Region => "Region".to_owned(),
        },
    }
}

/// Local date and time of a capture, like `12 Oct 2026, 14:05`.
fn when(ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1 << 20)),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapwright_library::Metadata;

    fn capture(app: &str, title: &str, source: Source) -> Capture {
        Capture {
            id: 1,
            hash: String::new(),
            created_ms: 0,
            width: 1,
            height: 1,
            bytes: 0,
            meta: Metadata {
                source,
                app: app.into(),
                window_title: title.into(),
                ..Metadata::default()
            },
            note: String::new(),
            tags: Vec::new(),
            text: String::new(),
            trashed: false,
        }
    }

    #[test]
    fn captions_prefer_app_and_title() {
        assert_eq!(
            caption(&capture("Figma", "Login", Source::Window)),
            "Figma — Login"
        );
        assert_eq!(caption(&capture("Figma", "", Source::Region)), "Figma");
        assert_eq!(caption(&capture("", "", Source::Display)), "Whole display");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_bytes(512), "512 bytes");
        assert_eq!(human_bytes(20 * 1024), "20 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 + 300 * 1024), "3.3 MB");
    }
}
