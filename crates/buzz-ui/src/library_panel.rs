//! The Library panel.
//!
//! Animate's Library is where every symbol in a document lives, organised into
//! folders. This panel keeps that model: a folder tree, a search box that
//! filters across the whole library at once, a use count per symbol so you can
//! see what is safe to delete, and the same set of operations on the strip
//! along the bottom.
//!
//! # Why the folder tree is derived, not stored
//!
//! [`buzz_scene::Library`] stores a symbol's folder as a path string plus a set
//! of folder paths, exactly as XFL does. That means the tree shown here is
//! computed each frame from those two facts rather than held as a parallel
//! structure that could drift out of step with the symbols. Rebuilding it is
//! cheap — a library with thousands of symbols is a handful of string
//! comparisons per row, and only visible rows are built.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use buzz_scene::{ImageAsset, ImageId, Scene, SoundId, SymbolId, SymbolKind};
use egui::{RichText, Ui};

use crate::command::Command;
use crate::theme::Palette;

/// Edge of a symbol's picture in the list, in points.
///
/// Matches the row height a symbol already had, so adding pictures did not
/// make the library taller per entry.
const THUMBNAIL: f32 = 20.0;

/// Room kept on the right of a symbol row for its use count.
const USE_COUNT: f32 = 26.0;

/// Room kept on the right of a sound row for its length.
const DURATION: f32 = 48.0;

/// Longest side of a bitmap's thumbnail texture, in pixels. Drawn at
/// [`THUMBNAIL`] points, so this is generous even on a dense display.
const PICTURE_THUMB: usize = 64;

/// A bitmap or a sound in the document's own libraries — the two things that
/// are listed beside the symbols (Animate's Library shows them too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryMedia {
    Picture(ImageId),
    Sound(SoundId),
}

/// What the user asked of a picture or sound, left on
/// [`LibraryState::media_request`] for the shell to carry out. The panel
/// cannot do it itself: placing needs the camera and the active layer, and a
/// deletion has to check what the stage still uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRequest {
    /// Put it on the stage (double-click, the Place button, the menu).
    Place(LibraryMedia),
    /// Take it out of the library (the bin button, the menu).
    Delete(LibraryMedia),
}

/// A picture or sound being dragged out of the Library, on its way to the
/// stage. A type of its own for the same reason as [`DraggedSymbol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedMedia(pub LibraryMedia);

/// A bitmap's small picture, kept with the allocation it was made from so an
/// edited bitmap (a different allocation) gets a fresh one.
#[derive(Clone)]
struct PictureThumb {
    asset: Arc<ImageAsset>,
    texture: egui::TextureHandle,
}

// By hand: the derived `Debug` would print the bitmap's pixels.
impl std::fmt::Debug for PictureThumb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PictureThumb")
            .field("image", &self.asset.id)
            .finish()
    }
}

/// A symbol being dragged out of the Library, on its way to the stage.
///
/// A newtype rather than a bare `SymbolId` because egui keys a drag payload by
/// its *type*: anything else that ever wants to be dragged must be
/// distinguishable from this, or the stage would place a symbol when a swatch
/// was dropped on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedSymbol(pub SymbolId);

/// Which items a panel is showing: everything, only what moves, or only stills.
///
/// The same three-way choice serves the Library and the Assets panels — an
/// animated symbol and an animated asset are one idea at two scales — so it
/// lives in one place and both toggles drive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionFilter {
    /// Animated and static together, each under its own heading.
    #[default]
    All,
    /// Only what moves.
    Animated,
    /// Only stills.
    Static,
}

impl MotionFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Animated, Self::Static];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Animated => "Animated",
            Self::Static => "Static",
        }
    }

    /// Does an item with this motion pass the filter?
    pub fn accepts(self, animated: bool) -> bool {
        match self {
            Self::All => true,
            Self::Animated => animated,
            Self::Static => !animated,
        }
    }

    /// Draw the three-way toggle, updating `current`. Shared by both panels so
    /// they read and behave the same.
    pub fn toggle(ui: &mut Ui, current: &mut Self) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show").small().weak());
            for option in Self::ALL {
                if ui
                    .selectable_label(*current == option, option.label())
                    .clicked()
                {
                    *current = option;
                }
            }
        });
    }
}

/// Panel state that is not part of the document.
///
/// Which folders are open and what is typed in the search box are view state:
/// they must survive a frame but must never end up in the saved file or in the
/// undo history.
#[derive(Debug, Clone, Default)]
pub struct LibraryState {
    /// The symbol whose properties the panel is showing.
    pub selected: Option<SymbolId>,
    /// The folder currently highlighted, for "new symbol goes here".
    pub selected_folder: Option<String>,
    /// Free-text filter. Matches symbol names, case-insensitively.
    pub search: String,
    /// What kind of symbol New Symbol and Convert to Symbol produce.
    ///
    /// Animate asks in a dialog; this is the same choice, kept as a sticky
    /// setting so a run of conversions does not need one click each.
    pub new_symbol_kind: SymbolKind,
    /// Whether the list shows every symbol, only animated ones, or only stills.
    pub motion: MotionFilter,
    /// Clear All Keyframes was raised from this panel's menu, so it targets
    /// `selected` rather than a selected stage instance. Taken by the editor.
    pub clear_keys_from_library: bool,
    /// Duplicate Symbol was raised from this panel, so it copies `selected`
    /// rather than a selected stage instance's symbol. Taken by the editor.
    pub duplicate_from_library: bool,
    /// The picture or sound highlighted in the list. Exclusive with `selected`
    /// and `selected_folder`: the strip along the bottom acts on one thing.
    pub selected_media: Option<LibraryMedia>,
    /// Place or Delete asked of a picture or sound. Taken by the editor.
    pub media_request: Option<MediaRequest>,
    /// Folder paths the user has opened.
    expanded: BTreeSet<String>,
    /// Rename buffer, live only while a rename is in progress.
    renaming: Option<(SymbolId, String)>,
    /// The same for a picture or sound.
    renaming_media: Option<(LibraryMedia, String)>,
    /// Small pictures of the document's bitmaps. Made here, from the pixels,
    /// rather than by the shell's GPU thumbnails, which draw symbols.
    picture_thumbs: HashMap<ImageId, PictureThumb>,
    /// The search string, lower-cased, recomputed only when `search` changes.
    ///
    /// `matches` is called once per symbol; lower-casing the needle inside it
    /// allocated a `String` per symbol per frame, which on a large library is a
    /// per-frame cost that scales with the whole library. Cached here instead.
    search_lower: String,
}

impl LibraryState {
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    /// Open every folder, which is what a search needs in order to show hits
    /// buried several levels down.
    pub fn expand_all(&mut self, scene: &Scene) {
        self.expanded = scene.library().folders().cloned().collect();
    }

    /// Is the panel filtering?
    pub fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// Recompute the cached lower-cased needle. Called once at the top of a
    /// frame — one allocation a frame, rather than one per symbol in `matches`.
    fn sync_search(&mut self) {
        self.search_lower = self.search.trim().to_lowercase();
    }

    fn matches(&self, name: &str) -> bool {
        self.search_lower.is_empty() || name.to_lowercase().contains(&self.search_lower)
    }

    /// The small picture of a bitmap, made on first sight and again whenever
    /// the bitmap is replaced.
    fn picture_thumbnail(&mut self, ctx: &egui::Context, asset: &Arc<ImageAsset>) -> egui::TextureHandle {
        if let Some(thumb) = self.picture_thumbs.get(&asset.id)
            && Arc::ptr_eq(&thumb.asset, asset)
        {
            return thumb.texture.clone();
        }
        let texture = ctx.load_texture(
            format!("library-picture-{}", asset.id.0),
            picture_thumbnail_image(asset),
            egui::TextureOptions::LINEAR,
        );
        self.picture_thumbs.insert(
            asset.id,
            PictureThumb {
                asset: Arc::clone(asset),
                texture: texture.clone(),
            },
        );
        texture
    }
}

/// A bitmap shrunk to at most [`PICTURE_THUMB`] pixels a side, each pixel the
/// average of up to nine samples of the block it stands for. Averaged with
/// the alpha as a weight, so a transparent edge does not darken the colour.
fn picture_thumbnail_image(asset: &ImageAsset) -> egui::ColorImage {
    let (w, h) = (asset.width as usize, asset.height as usize);
    if w == 0 || h == 0 || asset.pixels.len() < w * h * 4 {
        return egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT);
    }
    let scale = (PICTURE_THUMB as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as usize).max(1);
    let th = ((h as f32 * scale).round() as usize).max(1);
    let mut rgba = Vec::with_capacity(tw * th * 4);
    for ty in 0..th {
        let y0 = ty * h / th;
        let y1 = ((ty + 1) * h / th).clamp(y0 + 1, h);
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = ((tx + 1) * w / tw).clamp(x0 + 1, w);
            let (nx, ny) = ((x1 - x0).min(3), (y1 - y0).min(3));
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..ny {
                let y = y0 + sy * (y1 - y0) / ny;
                for sx in 0..nx {
                    let x = x0 + sx * (x1 - x0) / nx;
                    let p = &asset.pixels[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    let weight = u32::from(p[3]);
                    r += u32::from(p[0]) * weight;
                    g += u32::from(p[1]) * weight;
                    b += u32::from(p[2]) * weight;
                    a += weight;
                }
            }
            let samples = (nx * ny) as u32;
            if a == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                rgba.extend_from_slice(&[(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / samples) as u8]);
            }
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba)
}

/// Looks up the picture for a symbol, and remembers that it was asked for.
///
/// A closure rather than a type, because the pictures live on the GPU and this
/// crate has no device — the shell owns them and hands in a way to ask.
pub type ThumbnailSource<'a> = &'a mut dyn FnMut(SymbolId) -> Option<egui::TextureId>;

/// Draw the Library panel.
///
impl LibraryState {
    /// **Start naming a symbol**, as though its name had been double-clicked.
    ///
    /// Called the moment one is made. Convert to Symbol used to name it
    /// "Symbol" and leave it at that: Animate asks for the name as part of the
    /// gesture, and here there was nowhere to say it — the artwork became
    /// "Symbol", then "Symbol 2", and a library of them had to be renamed
    /// afterwards one at a time, if you remembered which was which.
    ///
    /// A field with the name in it, focused, is the same offer without a modal
    /// in the way: type over it, or press Enter and keep the default.
    pub fn start_naming(&mut self, id: SymbolId, name: impl Into<String>) {
        self.renaming = Some((id, name.into()));
    }

    /// The symbol whose name is being typed, if one is. For tests and for
    /// anything that needs to know a field has the keyboard.
    pub fn naming(&self) -> Option<SymbolId> {
        self.renaming.as_ref().map(|(id, _)| *id)
    }
}

/// Takes the scene mutably because renaming a symbol and moving it between
/// folders are edits in their own right; the caller wraps the call in an undo
/// step, and a frame where nothing changed records nothing.
pub fn library_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    // Use counts per symbol. Computed off the UI thread by the shell and passed
    // in, because walking every object in a large document to count instances
    // was a per-frame cost that scaled with the whole file — see the caller.
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    thumbnail: ThumbnailSource<'_>,
) -> Option<Command> {
    let mut command = None;
    // Lower-case the search needle once, not once per symbol in `matches`.
    state.sync_search();
    // A symbol or folder chosen from outside (Place, New Folder) takes the
    // highlight from a picture or sound; pictures that are gone lose their
    // small image.
    if state.selected.is_some() || state.selected_folder.is_some() {
        state.selected_media = None;
    }
    state
        .picture_thumbs
        .retain(|id, _| scene.images().get(*id).is_some());

    let items = scene.library().len() + scene.images().len() + scene.sounds().len();
    ui.horizontal(|ui| {
        ui.heading("Library");
        ui.label(RichText::new(format!("{items} items")).small().weak());
    });

    // Search. Typing opens every folder, because a hit inside a closed folder
    // that the user cannot see is worse than no hit at all.
    ui.horizontal(|ui| {
        ui.label("🔍");
        if ui.text_edit_singleline(&mut state.search).changed() && state.is_searching() {
            state.expand_all(scene);
        }
        if !state.search.is_empty()
            && crate::widgets::icon_button(ui, "x", "Clear the search", None).clicked()
        {
            state.search.clear();
        }
    });

    // Animated or still. On "All" the two are shown apart, under their own
    // headings; the other two narrow the list to one or the other.
    MotionFilter::toggle(ui, &mut state.motion);
    ui.separator();

    // **A fixed height, not the room available.**
    //
    // This panel keeps a scroll area of its own, because a library of three
    // hundred symbols has to be scrollable without carrying the whole dock
    // column with it. But it sits *inside* the column's scroll area, and there
    // `available_height` is not the height of the window — it is however much
    // room the column is willing to promise, which is effectively all of it.
    // Asking for that meant the library grew to fill the column and pushed the
    // Assets panel below it clean off the bottom of the screen, where nobody
    // was ever going to find it.
    //
    // So: a definite number of rows. Enough to browse in, and never enough to
    // hide what comes after it.
    if items == 0 {
        egui::ScrollArea::vertical()
            .id_salt("library-items")
            .auto_shrink([false, true])
            .max_height(300.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "No symbols yet.\n\n\
                         Select artwork and press F8 to convert it to a symbol, \
                         or use File > Import to Library to bring in a picture, \
                         a sound or an Animate document.",
                    )
                    .weak()
                    .italics(),
                );
            });
    } else {
        // **Virtualized.** The tree is flattened to a list and only the rows the
        // scroll area can show are turned into widgets, so a ten-thousand-symbol
        // library costs a screenful of rows a frame, not ten thousand.
        let rows = flatten_rows(scene, state);
        let row_height = THUMBNAIL + 2.0;
        egui::ScrollArea::vertical()
            .id_salt("library-items")
            .auto_shrink([false, true])
            .max_height(300.0)
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    // Each row is built inside a fixed-height slot so the
                    // virtualized layout stays aligned with its scrollbar.
                    let (slot, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::hover(),
                    );
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(slot)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    match &rows[i] {
                        Row::Folder { path, leaf, depth } => {
                            draw_folder_row(&mut row_ui, state, path, leaf, *depth);
                        }
                        Row::Group { label, depth } => {
                            draw_group_row(&mut row_ui, label, *depth);
                        }
                        Row::Symbol {
                            id,
                            name,
                            kind,
                            depth,
                        } => {
                            let indent = *depth as f32 * 14.0;
                            draw_symbol_row(
                                &mut row_ui,
                                scene,
                                state,
                                *id,
                                name,
                                *kind,
                                indent,
                                usage,
                                thumbnail,
                                &mut command,
                            );
                        }
                        Row::Picture { id, name } => {
                            draw_picture_row(&mut row_ui, scene, state, *id, name);
                        }
                        Row::Sound { id, name } => {
                            draw_sound_row(&mut row_ui, scene, state, *id, name);
                        }
                    }
                }
            });
    }

    images_section(ui, scene);

    ui.separator();
    draw_footer(ui, scene, state, usage, &mut command);

    command
}

/// A picture dragged out of the Library's Images folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedImage(pub buzz_scene::ImageId);

/// Library names Generate Image gives its pictures (`gen_<words>_<seed>`).
fn is_generated(name: &str) -> bool {
    name.starts_with("gen_")
}

/// **The Library's Images folder**: every picture in the document — imported
/// ones, and an "AI Generated" folder for Generate Image's — each with a
/// thumbnail, to drag onto the stage as Animate's bitmaps are. A picture sent
/// to the Library used to be stored in the file but listed nowhere.
fn images_section(ui: &mut Ui, scene: &Scene) {
    let images: Vec<std::sync::Arc<buzz_scene::ImageAsset>> = scene.images().iter().cloned().collect();
    if images.is_empty() {
        return;
    }
    ui.separator();
    let (generated, imported): (Vec<_>, Vec<_>) = images.into_iter().partition(|i| is_generated(&i.name));
    egui::CollapsingHeader::new(RichText::new(format!("Images ({})", generated.len() + imported.len())).strong())
        .id_salt("library-images")
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("library-images-list")
                .auto_shrink([false, true])
                .max_height(220.0)
                .show(ui, |ui| {
                    if !generated.is_empty() {
                        egui::CollapsingHeader::new(format!("AI Generated ({})", generated.len()))
                            .id_salt("library-images-generated")
                            .default_open(true)
                            .show(ui, |ui| generated.iter().for_each(|i| image_row(ui, i)));
                    }
                    for image in &imported {
                        image_row(ui, image);
                    }
                });
        });
}

/// One picture: thumbnail and name, dragged onto the stage to place it.
fn image_row(ui: &mut Ui, image: &buzz_scene::ImageAsset) {
    let texture = image_thumbnail(ui.ctx(), image);
    let id = egui::Id::new(("library-image", image.id.0));
    ui.dnd_drag_source(id, DraggedImage(image.id), |ui| {
        ui.horizontal(|ui| {
            match &texture {
                Some(t) => {
                    let size = t.size_vec2();
                    let scale = (THUMBNAIL / size.x.max(size.y)).min(1.0);
                    ui.add(egui::Image::new(t).fit_to_exact_size(size * scale));
                }
                None => {
                    ui.add_space(THUMBNAIL);
                }
            }
            ui.label(&image.name);
        });
    })
    .response
    .on_hover_text(format!("{} \u{d7} {} \u{2014} drag onto the stage to place it", image.width, image.height));
}

/// A small texture of `image`, made once and kept in egui's memory by id.
fn image_thumbnail(ctx: &egui::Context, image: &buzz_scene::ImageAsset) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("library-image-thumb", image.id.0, image.width, image.height));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(t);
    }
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0 || h == 0 || image.pixels.len() < w * h * 4 {
        return None;
    }
    let side = 48usize;
    let scale = (side as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    let mut rgba = Vec::with_capacity(tw * th * 4);
    for y in 0..th {
        for x in 0..tw {
            let sx = (x * w / tw).min(w - 1);
            let sy = (y * h / th).min(h - 1);
            let at = (sy * w + sx) * 4;
            rgba.extend_from_slice(&image.pixels[at..at + 4]);
        }
    }
    let colour = egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba);
    let texture = ctx.load_texture(format!("library-image-{}", image.id.0), colour, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    Some(texture)
}

/// One level of the tree: the folders directly inside `parent`, then the
/// symbols that sit in `parent` itself.
///
/// Folders come first because that is where Animate puts them, and because a
/// long symbol list would otherwise push the folders off the top.
/// One line of the flattened Library tree.
///
/// The tree — folders, expanded into their contents, and the symbols at each
/// level — is flattened into this list once per frame, cheaply (no widgets),
/// and then only the rows the scroll area can actually show are built into
/// widgets. Without that, a flat library of ten thousand symbols built ten
/// thousand rows a frame, which was tens of milliseconds of pure layout and a
/// hang in its own right.
enum Row {
    Folder {
        path: String,
        leaf: String,
        depth: usize,
    },
    /// An "Animated" or "Static" heading over the symbols that follow, shown
    /// only when the filter is [`MotionFilter::All`] and that group has members.
    Group { label: &'static str, depth: usize },
    Symbol {
        id: SymbolId,
        name: String,
        kind: SymbolKind,
        depth: usize,
    },
    /// A bitmap, listed after the symbols under a "Pictures" heading.
    Picture { id: ImageId, name: String },
    /// A sound, listed after the pictures under a "Sounds" heading.
    Sound { id: SoundId, name: String },
}

/// Flatten the visible tree — expanded folders and matching symbols — into a
/// list of rows, in display order. Reads only; the widgets are built later.
fn flatten_rows(scene: &Scene, state: &LibraryState) -> Vec<Row> {
    fn walk(
        scene: &Scene,
        state: &LibraryState,
        parent: Option<&str>,
        depth: usize,
        rows: &mut Vec<Row>,
    ) {
        for folder in scene.library().child_folders(parent) {
            let leaf = folder.rsplit('/').next().unwrap_or(&folder).to_string();
            rows.push(Row::Folder {
                path: folder.clone(),
                leaf,
                depth,
            });
            if state.is_expanded(&folder) {
                walk(scene, state, Some(&folder), depth + 1, rows);
            }
        }
        // Split this level's symbols into moving and still, keeping only what
        // matches the search and the motion filter.
        let mut animated = Vec::new();
        let mut still = Vec::new();
        for symbol in scene.library().symbols_in(parent) {
            if !state.matches(&symbol.name) || !state.motion.accepts(symbol.is_animated()) {
                continue;
            }
            if symbol.is_animated() {
                &mut animated
            } else {
                &mut still
            }
            .push(symbol);
        }

        let mut emit = |label: &'static str, symbols: &[&std::sync::Arc<buzz_scene::Symbol>]| {
            if symbols.is_empty() {
                return;
            }
            // A heading only earns its row when both groups are on show; asked
            // for one alone, the list is already all of one kind.
            if state.motion == MotionFilter::All {
                rows.push(Row::Group { label, depth });
            }
            for symbol in symbols {
                rows.push(Row::Symbol {
                    id: symbol.id,
                    name: symbol.name.clone(),
                    kind: symbol.kind,
                    depth,
                });
            }
        };
        emit("Animated", &animated);
        emit("Static", &still);
    }
    let mut rows = Vec::new();
    walk(scene, state, None, 0, &mut rows);

    // Pictures and sounds are not filed in folders; they follow the tree. A
    // picture is a still, so "Animated" hides it; a sound is neither, so only
    // "All" shows it.
    let pictures: Vec<_> = scene
        .images()
        .iter()
        .filter(|i| state.motion.accepts(false) && state.matches(&i.name))
        .collect();
    if !pictures.is_empty() {
        rows.push(Row::Group {
            label: "Pictures",
            depth: 0,
        });
        rows.extend(pictures.iter().map(|i| Row::Picture {
            id: i.id,
            name: i.name.clone(),
        }));
    }
    let sounds: Vec<_> = scene
        .sounds()
        .iter()
        .filter(|s| state.motion == MotionFilter::All && state.matches(&s.name))
        .collect();
    if !sounds.is_empty() {
        rows.push(Row::Group {
            label: "Sounds",
            depth: 0,
        });
        rows.extend(sounds.iter().map(|s| Row::Sound {
            id: s.id,
            name: s.name.clone(),
        }));
    }
    rows
}

/// Draw an "Animated" / "Static" heading over the symbols beneath it.
fn draw_group_row(ui: &mut Ui, label: &str, depth: usize) {
    let indent = depth as f32 * 14.0;
    ui.horizontal(|ui| {
        ui.add_space(indent + 18.0);
        ui.label(RichText::new(label).small().weak());
    });
}

/// Draw one folder row of the flattened tree.
fn draw_folder_row(ui: &mut Ui, state: &mut LibraryState, path: &str, leaf: &str, depth: usize) {
    let indent = depth as f32 * 14.0;
    let open = state.is_expanded(path);
    let selected = state.selected_folder.as_deref() == Some(path);

    ui.horizontal(|ui| {
        ui.add_space(indent);
        // `⏷` rather than `▼`: the bundled fonts have a glyph for U+23F7 and
        // none for U+25BC, so an expanded folder used to be marked with an empty
        // box. `▶` for a closed one does render.
        let glyph = if open { "\u{23F7}" } else { "\u{25B6}" };
        if crate::widgets::icon_button(ui, glyph, "Expand or collapse the folder", None).clicked()
        {
            state.toggle(path);
        }
        ui.label(RichText::new("F").small().weak())
            .on_hover_text("Folder");
        if ui.selectable_label(selected, leaf).clicked() {
            state.selected_folder = Some(path.to_string());
            state.selected = None;
            state.selected_media = None;
        }
    });
}

#[allow(
    clippy::too_many_arguments,
    reason = "internal row painter, not an API"
)]
fn draw_symbol_row(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    id: SymbolId,
    name: &str,
    kind: SymbolKind,
    indent: f32,
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    thumbnail: ThumbnailSource<'_>,
    command: &mut Option<Command>,
) {
    let uses = usage.get(&id).copied().unwrap_or(0);

    // **The whole row is a drag source.**
    //
    // Placing a symbol was: select it, find the Place button, press it, then
    // hunt for where the artwork landed and drag it there. Dragging the row
    // onto the stage is the same intent with the three middle steps removed,
    // and it is what every library in every drawing program does. The stage
    // picks the payload up; see `App::handle_stage_input`.
    let drag_id = ui.id().with(("library-drag", id.0));
    ui.horizontal(|ui| {
        ui.add_space(indent + 18.0);

        // **The picture, where the eye goes first.**
        //
        // A symbol identified only by its name means opening symbols to find
        // out what they are. The space is always claimed, whether or not the
        // picture has been drawn yet, so a library does not jiggle as its
        // thumbnails arrive over the next few frames.
        //
        // **And the picture is the handle**, not the whole row.
        //
        // `dnd_drag_source` interacts over everything it wraps, and a drag
        // widget laid over a row swallows the clicks meant for what is in it:
        // wrapping the row made its name unselectable, so nothing could ever
        // become the selected symbol — which is why Place, Duplicate and Delete
        // sat permanently greyed out and deleting a symbol looked impossible.
        // Measured: with the row wrapped, no click anywhere in the panel
        // selected a symbol; with only the thumbnail wrapped, every click on
        // the name does.
        //
        // Grabbing the picture is also what a library is expected to offer.
        let dragged = ui.dnd_drag_source(drag_id, DraggedSymbol(id), |ui| {
            let (slot, _) =
                ui.allocate_exact_size(egui::vec2(THUMBNAIL, THUMBNAIL), egui::Sense::hover());
            match thumbnail(id) {
                Some(texture) => {
                    egui::Image::new((texture, egui::vec2(THUMBNAIL, THUMBNAIL)))
                        .paint_at(ui, slot);
                }
                // Not drawn yet: a quiet frame, so the row reads as a row
                // rather than as a gap.
                None => {
                    ui.painter().rect_stroke(
                        slot,
                        2.0,
                        egui::Stroke::new(1.0, Palette::border()),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            slot
        });
        dragged
            .response
            .on_hover_text("Drag onto the stage to place an instance");

        // A one-letter kind marker, as Animate's icon column does.
        let mark = match kind {
            SymbolKind::Graphic => "G",
            SymbolKind::MovieClip => "M",
            SymbolKind::Button => "B",
        };
        ui.label(RichText::new(mark).small().weak())
            .on_hover_text(kind.label());

        // A rename in progress replaces the label with a field.
        if let Some((renaming, buffer)) = &mut state.renaming
            && *renaming == id
        {
            let response = ui.text_edit_singleline(buffer);
            let commit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if commit {
                let new_name = buffer.trim().to_string();
                if !new_name.is_empty() && new_name != name {
                    // Uniqueness is the library's rule, not the panel's, so
                    // two symbols can never end up sharing a name.
                    let unique = scene.library().unique_name(&new_name);
                    scene.library_mut().update(id, |s| s.name = unique);
                }
                state.renaming = None;
            } else if response.lost_focus() {
                state.renaming = None;
            } else {
                response.request_focus();
            }
            return;
        }

        // Truncated rather than allowed to run on: a symbol name has no length
        // limit, and the use count on the right of this row is the one thing
        // that must not be pushed off the panel by one.
        // `add_sized`, not `min_size`: a minimum is only a floor, and
        // `truncate` still wraps against the *whole* remaining width, so the
        // button could grow into the room the use count needs. Sizing the
        // allocation is what actually bounds it.
        let room = (ui.available_width() - USE_COUNT).max(1.0);
        let label = ui.add_sized(
            egui::vec2(room, ui.spacing().interact_size.y),
            egui::Button::selectable(state.selected == Some(id), name).truncate(),
        );
        if label.clicked() {
            state.selected = Some(id);
            state.selected_folder = None;
            state.selected_media = None;
        }
        if label.double_clicked() {
            // Double-click opens the symbol, exactly as in Animate.
            state.selected = Some(id);
            *command = Some(Command::EditSymbol);
        }
        label.context_menu(|ui| {
            for c in [
                Command::EditSymbol,
                Command::PlaceInstance,
                Command::DuplicateSymbol,
                Command::SymbolToAsset,
                Command::ClearAllKeyframes,
                Command::DeleteSymbol,
            ] {
                if ui.button(c.label()).clicked() {
                    state.selected = Some(id);
                    state.clear_keys_from_library = c == Command::ClearAllKeyframes;
                    state.duplicate_from_library = c == Command::DuplicateSymbol;
                    *command = Some(c);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Rename").clicked() {
                state.renaming = Some((id, name.to_string()));
                ui.close();
            }
            // Animate's Symbol Properties ▸ Type: a movie clip holds its first
            // frame while authoring, a graphic plays on the timeline it sits
            // in — the switch that makes a nested animation play.
            ui.menu_button("Type", |ui| {
                for to in [SymbolKind::Graphic, SymbolKind::MovieClip, SymbolKind::Button] {
                    if ui.radio(kind == to, to.label()).clicked() {
                        scene.set_symbol_kind(id, to);
                        ui.close();
                    }
                }
            });
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Zero uses is worth noticing — it is the safe-to-delete signal.
            let text = RichText::new(format!("{uses}")).small();
            let text = if uses == 0 { text.weak() } else { text };
            ui.label(text).on_hover_text(if uses == 0 {
                "Not used anywhere".to_string()
            } else {
                format!("Used {uses} time(s), including inside other symbols")
            });
        });
    });
}

/// One bitmap row: its small picture (the drag handle, as a symbol's is), a
/// "P" marker, and its name.
fn draw_picture_row(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    id: ImageId,
    name: &str,
) {
    let Some(asset) = scene.images().get(id).cloned() else {
        return;
    };
    let texture = state.picture_thumbnail(ui.ctx(), &asset);
    let drag_id = ui.id().with(("library-drag-picture", id.0));
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let dragged = ui.dnd_drag_source(drag_id, DraggedMedia(LibraryMedia::Picture(id)), |ui| {
            let (slot, _) =
                ui.allocate_exact_size(egui::vec2(THUMBNAIL, THUMBNAIL), egui::Sense::hover());
            // Fitted inside the square at its own proportions.
            let [tw, th] = texture.size();
            let scale = THUMBNAIL / tw.max(th).max(1) as f32;
            let size = egui::vec2(tw as f32 * scale, th as f32 * scale);
            egui::Image::new((texture.id(), size))
                .paint_at(ui, egui::Rect::from_center_size(slot.center(), size));
            ui.painter().rect_stroke(
                slot,
                2.0,
                egui::Stroke::new(1.0, Palette::hairline()),
                egui::StrokeKind::Inside,
            );
            slot
        });
        dragged
            .response
            .on_hover_text("Drag onto the stage to place it");
        ui.label(RichText::new("P").small().weak())
            .on_hover_text(format!("Picture, {} x {}", asset.width, asset.height));
        draw_media_name(ui, scene, state, LibraryMedia::Picture(id), name, 0.0);
    });
}

/// One sound row: a small bar icon (the drag handle), an "S" marker, its name
/// and, on the right, its length.
fn draw_sound_row(ui: &mut Ui, scene: &mut Scene, state: &mut LibraryState, id: SoundId, name: &str) {
    let Some(seconds) = scene.sounds().get(id).map(|s| s.duration_seconds()) else {
        return;
    };
    let drag_id = ui.id().with(("library-drag-sound", id.0));
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let dragged = ui.dnd_drag_source(drag_id, DraggedMedia(LibraryMedia::Sound(id)), |ui| {
            let (slot, _) =
                ui.allocate_exact_size(egui::vec2(THUMBNAIL, THUMBNAIL), egui::Sense::hover());
            // Five bars of different heights, as a sound is drawn everywhere.
            let colour = ui.visuals().weak_text_color();
            for (i, height) in [0.35, 0.7, 1.0, 0.6, 0.3].into_iter().enumerate() {
                let x = slot.left() + THUMBNAIL * (0.15 + 0.175 * i as f32);
                let half = THUMBNAIL * 0.4 * height;
                ui.painter().line_segment(
                    [
                        egui::pos2(x, slot.center().y - half),
                        egui::pos2(x, slot.center().y + half),
                    ],
                    egui::Stroke::new(2.0, colour),
                );
            }
            slot
        });
        dragged
            .response
            .on_hover_text("Drag onto the stage to put it on a layer");
        ui.label(RichText::new("S").small().weak())
            .on_hover_text("Sound");
        draw_media_name(ui, scene, state, LibraryMedia::Sound(id), name, DURATION);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{seconds:.1} s")).small().weak());
        });
    });
}

/// The name of a picture or sound: a button that selects it, places it on a
/// double-click, and opens its menu; a text field while it is being renamed.
/// `reserve` is the room left on the right for what the caller draws there.
fn draw_media_name(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    media: LibraryMedia,
    name: &str,
    reserve: f32,
) {
    if let Some((renaming, buffer)) = &mut state.renaming_media
        && *renaming == media
    {
        let response = ui.text_edit_singleline(buffer);
        let commit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if commit {
            let new_name = buffer.trim().to_string();
            if !new_name.is_empty() && new_name != name {
                rename_media(scene, media, &new_name);
            }
            state.renaming_media = None;
        } else if response.lost_focus() {
            state.renaming_media = None;
        } else {
            response.request_focus();
        }
        return;
    }

    let room = (ui.available_width() - reserve).max(1.0);
    let label = ui.add_sized(
        egui::vec2(room, ui.spacing().interact_size.y),
        egui::Button::selectable(state.selected_media == Some(media), name).truncate(),
    );
    // Highlighting a picture or sound lets go of any symbol or folder.
    fn choose(state: &mut LibraryState, media: LibraryMedia) {
        state.selected_media = Some(media);
        state.selected = None;
        state.selected_folder = None;
    }
    if label.clicked() {
        choose(state, media);
    }
    if label.double_clicked() {
        choose(state, media);
        state.media_request = Some(MediaRequest::Place(media));
    }
    label.context_menu(|ui| {
        if ui.button("Place on the stage").clicked() {
            choose(state, media);
            state.media_request = Some(MediaRequest::Place(media));
            ui.close();
        }
        if ui.button("Delete").clicked() {
            choose(state, media);
            state.media_request = Some(MediaRequest::Delete(media));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename").clicked() {
            state.renaming_media = Some((media, name.to_string()));
            ui.close();
        }
    });
}

/// Rename a picture or sound, keeping its name unique among its own kind.
/// The asset is replaced under the same id, so everything that refers to it by
/// id is unaffected.
fn rename_media(scene: &mut Scene, media: LibraryMedia, wanted: &str) {
    match media {
        LibraryMedia::Picture(id) => {
            let Some(asset) = scene.images().get(id).cloned() else {
                return;
            };
            let mut renamed = (*asset).clone();
            renamed.name = scene.images().unique_name(wanted);
            scene.images_mut().insert(renamed);
        }
        LibraryMedia::Sound(id) => {
            let Some(asset) = scene.sounds().get(id).cloned() else {
                return;
            };
            let mut renamed = (*asset).clone();
            renamed.name = scene.sounds().unique_name(wanted);
            scene.sounds_mut().insert(renamed);
        }
    }
}

/// The action strip and the "move to folder" control.
fn draw_footer(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    command: &mut Option<Command>,
) {
    let selected = state.selected;
    // A highlighted picture or sound, which Place and the bin act on when no
    // symbol is selected.
    let media = state.selected_media.filter(|_| selected.is_none());

    // **Wrapped, not one long row.**
    //
    // Seven controls end to end need something over 250 points, and a dock
    // column can legitimately be narrower than that. Unwrapped, the ones at the
    // end — Place, Duplicate, Delete — were simply drawn off the edge of the
    // panel, which is the "the Library is hidden" report: not the panel, the
    // half of it that had nowhere to go. Wrapping puts them on a second line
    // instead, and costs nothing in a column wide enough for one.
    ui.horizontal_wrapped(|ui| {
        // The kind the next new symbol gets, in place of Animate's dialog.
        egui::ComboBox::from_id_salt("library_new_kind")
            .selected_text(RichText::new(state.new_symbol_kind.label()).small())
            .width(72.0)
            .show_ui(ui, |ui| {
                for kind in [
                    SymbolKind::Graphic,
                    SymbolKind::MovieClip,
                    SymbolKind::Button,
                ] {
                    ui.selectable_value(&mut state.new_symbol_kind, kind, kind.label());
                }
            });
        let new_symbol_shortcut = crate::command::shortcut_text(ui.ctx(), Command::NewSymbol);
        if crate::widgets::icon_button(
            ui,
            "\u{2795}",
            Command::NewSymbol.label(),
            (!new_symbol_shortcut.is_empty()).then_some(new_symbol_shortcut.as_str()),
        )
        .clicked()
        {
            *command = Some(Command::NewSymbol);
        }
        // Stage 6, item 3: `.small()`/`small_button` opt out of the theme's
        // 24 px minimum hit target, which the audit test now checks for.
        if ui
            .button("Fld")
            .on_hover_text(Command::NewLibraryFolder.label())
            .clicked()
        {
            *command = Some(Command::NewLibraryFolder);
        }
        ui.separator();
        if ui
            .add_enabled(
                selected.is_some() || media.is_some(),
                egui::Button::new("Place"),
            )
            .on_hover_text("Place an instance on the stage")
            .clicked()
        {
            match media {
                Some(media) => state.media_request = Some(MediaRequest::Place(media)),
                None => *command = Some(Command::PlaceInstance),
            }
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Dup"))
            .on_hover_text(Command::DuplicateSymbol.label())
            .clicked()
        {
            state.duplicate_from_library = true;
            *command = Some(Command::DuplicateSymbol);
        }
        // **Out of the document and onto the shelf.** A symbol could be kept as
        // an asset only by placing an instance of it, selecting that, keeping
        // it, and deleting the instance again. The Library is where symbols
        // are, so it is where "keep this one" is asked.
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Asset"))
            .on_hover_text("Keep this symbol in the Assets library, to reuse in any document")
            .clicked()
        {
            *command = Some(Command::SymbolToAsset);
        }
        if ui
            .add_enabled(
                selected.is_some() || media.is_some(),
                egui::Button::new("🗑"),
            )
            .on_hover_text(if media.is_some() {
                "Delete the picture or sound from the library"
            } else {
                Command::DeleteSymbol.label()
            })
            .clicked()
        {
            match media {
                Some(media) => state.media_request = Some(MediaRequest::Delete(media)),
                None => *command = Some(Command::DeleteSymbol),
            }
        }
    });

    let Some(id) = selected else { return };
    let Some(symbol) = scene.library().get(id) else {
        return;
    };
    let (current, kind) = (symbol.folder.clone(), symbol.kind);
    let uses = usage.get(&id).copied().unwrap_or(0);

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Folder").small().weak());

        // The destination list is every folder plus the root, which is what
        // "organised in folders" needs in order to be usable without drag and
        // drop.
        let shown = current.clone().unwrap_or_else(|| "(root)".to_string());
        let mut target: Option<Option<String>> = None;

        egui::ComboBox::from_id_salt("library_folder")
            .selected_text(RichText::new(shown).small())
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "(root)").clicked() {
                    target = Some(None);
                }
                for folder in scene.library().folders().cloned().collect::<Vec<_>>() {
                    let is_current = current.as_deref() == Some(folder.as_str());
                    if ui.selectable_label(is_current, &folder).clicked() {
                        target = Some(Some(folder));
                    }
                }
            });

        if let Some(destination) = target {
            scene
                .library_mut()
                .move_to_folder(id, destination.as_deref());
        }

        ui.label(
            RichText::new(format!("{} · {uses} use(s)", kind.label()))
                .small()
                .weak(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::SymbolKind;

    fn library_scene() -> Scene {
        let mut scene = Scene::empty();
        scene.add_symbol("Hero Body", SymbolKind::Graphic, Some("Characters"));
        scene.add_symbol("Hero Arm", SymbolKind::Graphic, Some("Characters/Hero"));
        scene.add_symbol("Loop", SymbolKind::MovieClip, None);
        scene.library_mut().add_folder("Empty");
        scene
    }

    #[test]
    fn search_matches_case_insensitively_anywhere_in_the_name() {
        let mut state = LibraryState {
            search: "hero".to_string(),
            ..Default::default()
        };
        // The needle is lower-cased once per frame; the panel calls this, so the
        // test does too.
        state.sync_search();
        assert!(state.matches("Hero Body"));
        assert!(state.matches("SUPERHERO"));
        assert!(!state.matches("Loop"));

        // An uppercase search still matches — the caching lower-cases both ends.
        state.search = "HERO".to_string();
        state.sync_search();
        assert!(state.matches("hero body"));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let state = LibraryState::default();
        assert!(state.matches("anything at all"));

        let state = LibraryState {
            search: "   ".to_string(),
            ..Default::default()
        };
        assert!(!state.is_searching(), "whitespace is not a search");
        assert!(state.matches("anything at all"));
    }

    #[test]
    fn expanding_a_folder_toggles_it() {
        let mut state = LibraryState::default();
        assert!(!state.is_expanded("Characters"));
        state.toggle("Characters");
        assert!(state.is_expanded("Characters"));
        state.toggle("Characters");
        assert!(!state.is_expanded("Characters"));
    }

    /// A search has to reach symbols nested several folders deep, so it opens
    /// every folder — including ones with nothing in them.
    #[test]
    fn searching_expands_every_folder_including_intermediates() {
        let scene = library_scene();
        let mut state = LibraryState::default();
        state.expand_all(&scene);

        for folder in ["Characters", "Characters/Hero", "Empty"] {
            assert!(state.is_expanded(folder), "{folder} should be open");
        }
    }

    /// On "All", symbols are split into an Animated group and a Static group,
    /// each under its own heading, so a browsing eye can tell the two apart.
    #[test]
    fn symbols_are_grouped_into_animated_and_static() {
        let scene = library_scene();
        let mut state = LibraryState::default();
        state.expand_all(&scene);

        let rows = flatten_rows(&scene, &state);

        // The movie clip sits at the root, under an "Animated" heading.
        let loop_id = scene.library().find_by_name("Loop").expect("the clip").id;
        let animated_over_loop = rows.windows(2).any(|w| {
            matches!(
                w[0],
                Row::Group {
                    label: "Animated",
                    ..
                }
            ) && matches!(&w[1], Row::Symbol { id, .. } if *id == loop_id)
        });
        assert!(
            animated_over_loop,
            "the movie clip is under an Animated heading"
        );

        // The graphics are stills, so there is a Static heading too.
        assert!(
            rows.iter().any(|r| matches!(
                r,
                Row::Group {
                    label: "Static",
                    ..
                }
            )),
            "the graphics are under a Static heading"
        );
    }

    /// Asked for one kind, the list narrows to it and drops the headings — a
    /// list already all of one kind does not need to say so.
    #[test]
    fn the_motion_filter_narrows_the_list() {
        let scene = library_scene();
        let mut state = LibraryState {
            motion: MotionFilter::Animated,
            ..Default::default()
        };
        state.expand_all(&scene);

        let names = |rows: &[Row]| {
            let mut names: Vec<String> = rows
                .iter()
                .filter_map(|r| match r {
                    Row::Symbol { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect();
            names.sort();
            names
        };

        let rows = flatten_rows(&scene, &state);
        assert!(
            !rows.iter().any(|r| matches!(r, Row::Group { .. })),
            "no headings when only one kind is shown"
        );
        assert_eq!(names(&rows), ["Loop"], "only the movie clip is animated");

        state.motion = MotionFilter::Static;
        let rows = flatten_rows(&scene, &state);
        assert_eq!(
            names(&rows),
            ["Hero Arm", "Hero Body"],
            "the rest are stills"
        );
    }

    /// The tree the panel walks must reach every symbol exactly once, or a
    /// symbol would be invisible in the panel while still being in the file.
    #[test]
    fn every_symbol_is_reachable_by_walking_the_folder_tree() {
        let scene = library_scene();

        fn walk(scene: &Scene, parent: Option<&str>, found: &mut Vec<SymbolId>) {
            for folder in scene.library().child_folders(parent) {
                walk(scene, Some(&folder), found);
            }
            found.extend(scene.library().symbols_in(parent).iter().map(|s| s.id));
        }

        let mut found = Vec::new();
        walk(&scene, None, &mut found);
        found.sort();

        let mut all: Vec<SymbolId> = scene.library().iter().map(|s| s.id).collect();
        all.sort();

        assert_eq!(found, all, "the tree walk must reach every symbol once");
    }
}
