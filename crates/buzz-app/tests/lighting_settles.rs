//! **Switching a light on, and aiming one, must reach the screen.**
//!
//! The window sleeps between input events, so every frame that draws with
//! shading it knows to be provisional — a cold cache, a light still moving, a
//! batch already running — has to say so, or the last frame drawn is the stale
//! one and it stays on screen until something unrelated provokes a repaint.
//!
//! This drives the same sequence `App::draw` does, taking only the frames the
//! window would actually take, and asserts the shading converges. Run against
//! the rule that waited for a light to stop moving *without* reporting
//! staleness, it fails: adding a sun queues nothing, asks for no further frame,
//! and the crescents never appear at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Camera, Point, Rect, Shape as _, Size};
use buzz_render::document::DrawCache;
use buzz_scene::{LayerKind, LightKind, Scene, ShapeData};
use peniko::Color;

/// A few dozen curved shapes: enough that the window defers rather than
/// building everything on the frame.
fn artwork() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    for i in 0..60 {
        let x = 30.0 + ((i * 47) % 460) as f64;
        let y = 30.0 + ((i * 31) % 320) as f64;
        scene.add_shape(
            layer,
            ShapeData::filled(
                buzz_geom::Circle::new(Point::new(x, y), 26.0).to_path(0.05),
                Color::from_rgb8(0xC0, 0xC0, 0xC0),
            ),
        );
    }
    scene
}

struct Build {
    rx: crossbeam_channel::Receiver<Vec<buzz_render::lighting::Built>>,
    abandon: Arc<AtomicBool>,
    aim: u64,
}

/// `App::draw`, reduced to the lighting decisions and their ordering.
struct Window {
    editor: Editor,
    cache: DrawCache,
    vello: vello::Scene,
    build: Option<Build>,
    shade_aim: u64,
    /// What the window would do with `request_redraw`: is another frame owed?
    owed: bool,
    /// The retained stage encoding and what it was built for — the window keeps
    /// last frame's Vello scene and re-renders it when nothing that shaped it
    /// has changed. Modelled here because reusing an encoding built with
    /// provisional shading is precisely how the shading gets stuck.
    stamp: Option<(u64, u64)>,
    lights_generation: u64,
    stage_stale: bool,
}

impl Window {
    fn new(scene: Scene) -> Self {
        let mut editor = Editor::new(Document::new(scene));
        editor.camera = Camera::new(Point::new(275.0, 200.0), 1.0, Size::new(550.0, 400.0));
        Self {
            editor,
            cache: DrawCache::new(),
            vello: vello::Scene::new(),
            build: None,
            shade_aim: 0,
            owed: true,
            stamp: None,
            lights_generation: 0,
            stage_stale: false,
        }
    }

    /// One window frame. Returns whether the frame drew exact shading.
    fn frame(&mut self, gesturing: bool) -> bool {
        self.owed = false;
        let aim = self
            .editor
            .scene()
            .lights()
            .resolved_at(self.editor.current_frame)
            .aim();

        if let Some(build) = &self.build {
            if let Ok(built) = build.rx.try_recv() {
                self.cache.lights.install(built);
                self.build = None;
                self.lights_generation += 1;
                self.owed = true;
            } else if build.aim != aim {
                build.abandon.store(true, Ordering::Relaxed);
                self.build = None;
                self.owed = true;
            } else {
                // A batch in flight is polled, exactly as `wants_frame` does.
                self.owed = true;
            }
        }

        let cold = self.cache.lights.is_empty() && self.editor.scene().lights().is_active();
        let building = self.build.is_some();
        let settled = aim == self.shade_aim;
        self.shade_aim = aim;
        self.cache
            .lights
            .set_inline_budget(if cold || gesturing || building {
                std::time::Duration::ZERO
            } else {
                buzz_render::lighting::INLINE_BUDGET
            });
        self.cache.lights.set_queue(!building && settled);

        // The retained encoding, and the one rule that matters here: an
        // encoding built with provisional shading may not be reused, or nothing
        // ever re-encodes and the shading never catches up.
        let stamp = (self.editor.scene().revision(), self.lights_generation);
        let stale_encoding = self.stage_stale && self.build.is_none();
        let reuse = !cold && !stale_encoding && self.stamp == Some(stamp);
        if !reuse {
            self.vello.reset();
            buzz_app::stage::build_scene(
                &mut self.vello,
                &self.editor,
                Rect::new(0.0, 0.0, 550.0, 400.0),
                1.0,
                &mut self.cache,
            );
            self.stamp = Some(stamp);
            self.stage_stale = self.cache.lights.is_stale();
        }

        let misses = self.cache.lights.take_misses();
        self.cache.lights.set_defer(false);
        if !misses.is_empty() && self.build.is_none() {
            let (send, rx) = crossbeam_channel::bounded(1);
            let abandon = Arc::new(AtomicBool::new(false));
            let stop = Arc::clone(&abandon);
            std::thread::spawn(move || {
                use rayon::prelude::*;
                let built = misses
                    .into_par_iter()
                    .filter(|_| !stop.load(Ordering::Relaxed))
                    .map(buzz_render::lighting::Miss::build)
                    .collect::<Vec<_>>();
                let _ = send.send(built);
            });
            self.build = Some(Build { rx, abandon, aim });
            self.owed = true;
        }

        if self.stage_stale {
            self.owed = true;
        }
        !self.stage_stale
    }

    /// Draw for as long as the window would, up to a bound. Returns how many
    /// frames it took to reach exact shading, or `None` if it never did.
    fn settle(&mut self) -> Option<usize> {
        for n in 1..=40 {
            let exact = self.frame(false);
            if exact {
                return Some(n);
            }
            if !self.owed {
                // The window would now go to sleep with the wrong picture on
                // screen, and nothing would ever wake it.
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }
}

/// Switching a light on has to reach the screen without the user touching
/// anything else.
#[test]
fn adding_a_light_settles_on_its_own() {
    let mut window = Window::new(artwork());
    window.frame(false);

    window.editor.add_light(LightKind::sun());
    let frames = window.settle();
    assert!(
        frames.is_some(),
        "the window went to sleep with unlit artwork on screen; switching a \
         light on would do nothing until the pointer next crossed the window"
    );
    assert!(!window.cache.lights.is_empty(), "the crescents were never built");
}

/// Aiming a light has to reach the screen once the hand stops, without the user
/// having to jiggle the pointer to provoke it.
#[test]
fn aiming_a_light_settles_when_the_hand_stops() {
    let mut window = Window::new(artwork());
    window.editor.add_light(LightKind::sun());
    window.settle().expect("the first light should settle");

    // Drag the sun round, a frame per pointer move.
    for _ in 0..8 {
        window.editor.doc.edit("Aim Light", |scene| {
            if let Some(light) = scene.lights_mut().lights.first_mut()
                && let LightKind::Sun { azimuth, .. } = &mut light.kind
            {
                *azimuth += 0.12;
            }
        });
        window.frame(true);
    }

    // The hand comes up. Nothing else happens.
    let frames = window.settle();
    assert!(
        frames.is_some(),
        "the shading never caught up with the light after the drag ended"
    );
}

/// A lamp is the same story, and the one the report was about.
#[test]
fn moving_a_lamp_settles_when_the_hand_stops() {
    let mut window = Window::new(artwork());
    window.editor.add_light(LightKind::lamp(Point::new(80.0, 80.0)));
    window.settle().expect("the lamp should settle");

    for step in 1..=8 {
        window.editor.doc.edit("Move Light", |scene| {
            if let Some(light) = scene.lights_mut().lights.first_mut()
                && let LightKind::Lamp { position, .. } = &mut light.kind
            {
                *position = Point::new(80.0 + step as f64 * 40.0, 80.0);
            }
        });
        window.frame(true);
    }

    assert!(
        window.settle().is_some(),
        "the shading never caught up with the lamp after the drag ended"
    );
}

/// Switch the light off in the panel, leave it off, then switch it back on.
///
/// **The report.** Between the two the crescents age out of the cache
/// (`KEEP_FRAMES` is three, and a person takes seconds), so the light coming
/// back on is a *cold* cache again — and a cold frame builds nothing on the
/// thread and, because the aim has just moved, records nothing either. That
/// frame draws unlit. Everything then depends on the window asking for another
/// one, which is the thing that has to be true.
fn set_enabled(window: &mut Window, on: bool) {
    window.editor.doc.edit("Light", |scene| {
        let id = scene.lights().lights[0].id;
        if let Some(light) = scene.lights_mut().get_mut(id) {
            light.enabled = on;
        }
    });
}

#[test]
fn switching_a_light_off_and_on_again_settles() {
    let mut window = Window::new(artwork());
    window.editor.add_light(LightKind::sun());
    window.settle().expect("the first sun should settle");
    assert!(!window.cache.lights.is_empty());

    set_enabled(&mut window, false);
    // Long enough for the crescents to age out, as they would while a person
    // looks at the unlit picture and decides.
    for _ in 0..6 {
        window.frame(false);
    }

    set_enabled(&mut window, true);
    let frames = window.settle();
    assert!(
        frames.is_some(),
        "the window went to sleep with unlit artwork after the light was \
         switched back on"
    );
    assert!(
        !window.cache.lights.is_empty(),
        "the crescents were never rebuilt for the light coming back on"
    );
}

/// The same for the rig's own switch, which is the other way to turn a light
/// off and on again.
#[test]
fn switching_the_rig_off_and_on_again_settles() {
    let mut window = Window::new(artwork());
    window.editor.add_light(LightKind::lamp(Point::new(80.0, 80.0)));
    window.settle().expect("the lamp should settle");

    window.editor.doc.edit("Lighting", |scene| {
        scene.lights_mut().enabled = false;
    });
    for _ in 0..6 {
        window.frame(false);
    }

    window.editor.doc.edit("Lighting", |scene| {
        scene.lights_mut().enabled = true;
    });
    assert!(
        window.settle().is_some(),
        "the window went to sleep unlit after the rig was switched back on"
    );
    assert!(!window.cache.lights.is_empty(), "no crescents came back");
}
