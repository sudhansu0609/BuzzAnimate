//! **Test Movie**: play the whole film, full quality, in an overlay over the
//! stage — Animate's Ctrl+Enter.
//!
//! # The same picture the file will hold
//!
//! This does not read the stage's own view of the document — the stage lights
//! differently, shows guides and honours the pasteboard. Test Movie renders
//! through [`crate::preview::render_export_frame`], the same function the
//! export preview strip uses, so what plays here is what an export would
//! write: lit, masks clipping, no authoring aid anywhere in the picture.
//!
//! # One film, not one scene
//!
//! A document can hold several scenes, played one after another as one film
//! — see [`buzz_export::Reel`]. Test Movie plays all of them, in order, and
//! loops the whole film rather than the scene that happened to be open.
//!
//! # The audio clock wins while it is running
//!
//! The picture is told where the sound has reached, the same pattern
//! `Editor::advance_playback` uses, and for the same reason: a dropped video
//! frame must not nudge the dialogue. Sound plays for the scene that was
//! active when Test Movie opened only — see the plan's decision on this; a
//! multi-scene soundtrack is a follow-up.

use buzz_render::wgpu;
use buzz_scene::Scene;

/// How long the rendered picture's edge may be, in pixels.
///
/// Full quality without asking a big document to re-render at its full
/// display size every frame on the UI thread — the same trade the export
/// preview strip makes, just at a size actually worth watching.
const MAX_EDGE: u32 = 1920;

/// How many frames a stalled clock may catch up in one call, the same cap
/// `Editor::advance_playback` uses.
const MAX_CATCH_UP: u32 = 240;

/// The whole film, playing.
pub struct TestMovie {
    /// Every scene of the film, in play order, each with how many film
    /// frames it contributes — a snapshot taken when Test Movie opened, so
    /// edits made while it plays do not tear the picture. See
    /// `buzz_doc::Document::film_with_lengths`: a sequence's setups are
    /// already skipped and its shots already in play order by the time this
    /// is built.
    scenes: Vec<(Scene, u32)>,
    /// Which scene was open when Test Movie started: the one whose sound
    /// plays.
    active_index: usize,
    /// The current position, as a film frame — see [`buzz_export::Reel`].
    frame: u32,
    /// The lead scene's frame rate, read once at open.
    fps: f64,
    /// Sub-frame time carried between calls to [`Self::advance`], while the
    /// wall clock rather than the audio clock is driving.
    accumulator: f64,
    /// The pixel size the overlay last asked for.
    size_wanted: (u32, u32),
    /// The live render target, its texture id, and the size it was made at.
    target: Option<(wgpu::Texture, wgpu::TextureView, egui::TextureId, u32, u32)>,
    /// Textures to free once the renderer is reachable again.
    retired: Vec<egui::TextureId>,
    /// Which film frame `target` currently shows, if any.
    drawn: Option<u32>,
}

/// The reel this film's snapshot makes, each scene contributing exactly the
/// film frames it was given — see `buzz_doc::Document::film_with_lengths`
/// and `buzz_export::Reel::of_lengths`.
fn reel_of(scenes: &[(Scene, u32)]) -> buzz_export::Reel<'_> {
    buzz_export::Reel::of_lengths(scenes.iter().map(|(scene, length)| (scene, *length)))
}

impl TestMovie {
    /// Open the whole film, starting from frame zero. `None` for a document
    /// with nothing to play — an empty scene, or no scenes at all.
    pub fn open(scenes: Vec<(Scene, u32)>, active_index: usize) -> Option<Self> {
        let reel = reel_of(&scenes);
        if reel.is_empty() {
            return None;
        }
        let fps = reel
            .lead()
            .map(|s| s.stage().frame_rate)
            .unwrap_or(24.0)
            .max(0.01);
        Some(Self {
            scenes,
            active_index,
            frame: 0,
            fps,
            accumulator: 0.0,
            size_wanted: (0, 0),
            target: None,
            retired: Vec::new(),
            drawn: None,
        })
    }

    /// Advance the play position by `elapsed` seconds of wall clock.
    ///
    /// `audio_frame` is `Some` with the active scene's own timeline frame
    /// while its sound is playing — [`crate::sound::SoundBank::playing_frame`]
    /// — and while it is, the picture follows it rather than the wall clock,
    /// exactly as `Editor::advance_playback` follows sound within one scene.
    /// The whole film always loops, whatever the active scene's own loop
    /// region says: Test Movie plays the film end to end, on repeat.
    pub fn advance(&mut self, elapsed: f64, audio_frame: Option<u32>) {
        let (total, active_start) = {
            let reel = reel_of(&self.scenes);
            (reel.frames(), reel.start_of(self.active_index))
        };
        if total == 0 {
            return;
        }

        if let (Some(local), Some(start)) = (audio_frame, active_start) {
            self.frame = (start + local).min(total - 1);
            self.accumulator = 0.0;
            return;
        }

        self.accumulator += elapsed.max(0.0);
        let per_frame = 1.0 / self.fps;
        let whole = (self.accumulator / per_frame).floor();
        let advanced = whole.clamp(0.0, MAX_CATCH_UP as f64) as u32;
        if whole >= MAX_CATCH_UP as f64 {
            // Gave up catching up after a stall; drop the backlog.
            self.accumulator = 0.0;
        } else {
            self.accumulator -= advanced as f64 * per_frame;
        }
        if advanced == 0 {
            return;
        }
        self.frame = (self.frame + advanced) % total;
    }

    /// The current film frame.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// How long the whole film is, in frames.
    pub fn frames(&self) -> u32 {
        reel_of(&self.scenes).frames()
    }

    /// Is there a frame waiting to be rendered? True the first time, and
    /// whenever the play position has moved since the last render.
    pub fn needs_render(&self) -> bool {
        self.target.is_none() || self.drawn != Some(self.frame)
    }

    /// Fit the render target to the stage's own aspect within the space the
    /// overlay has, the long edge capped at [`MAX_EDGE`].
    pub fn set_view_size(&mut self, px_w: u32, px_h: u32) {
        let px_w = px_w.max(1);
        let px_h = px_h.max(1);
        let aspect = {
            let reel = reel_of(&self.scenes);
            reel.lead().map(|s| {
                let size = s.stage().size;
                size.width.max(1.0) / size.height.max(1.0)
            })
        }
        .unwrap_or(1.0);

        let (mut w, mut h) = if (px_w as f64 / px_h as f64) >= aspect {
            // Room is wider than the stage: the height is the limit.
            let h = px_h;
            (((h as f64) * aspect).round() as u32, h)
        } else {
            let w = px_w;
            (w, ((w as f64) / aspect).round() as u32)
        };
        if w.max(h) > MAX_EDGE {
            if w >= h {
                h = ((MAX_EDGE as f64) * (h as f64) / (w.max(1) as f64)).round() as u32;
                w = MAX_EDGE;
            } else {
                w = ((MAX_EDGE as f64) * (w as f64) / (h.max(1) as f64)).round() as u32;
                h = MAX_EDGE;
            }
        }
        self.size_wanted = (w.max(1), h.max(1));
    }

    /// The texture to draw, its id, and its pixel size — once a frame has
    /// landed.
    pub fn texture(&self) -> Option<(egui::TextureId, u32, u32)> {
        self.target.as_ref().map(|(_, _, id, w, h)| (*id, *w, *h))
    }

    /// Render the current frame, through the same pipeline an export uses.
    ///
    /// Recreates the target texture only when the wanted size has changed —
    /// most frames just redraw into the one that is already there.
    pub fn fulfil(
        &mut self,
        gpu: &mut buzz_render::GpuContext,
        egui_renderer: &mut egui_wgpu::Renderer,
        vello: &mut vello::Scene,
        cache: &mut buzz_render::document::DrawCache,
    ) {
        if !self.needs_render() {
            return;
        }
        let (width, height) = self.size_wanted;
        if width == 0 || height == 0 {
            return;
        }

        let needs_new_texture = match &self.target {
            Some((_, _, _, w, h)) => *w != width || *h != height,
            None => true,
        };
        if needs_new_texture {
            if let Some((_, _, id, _, _)) = self.target.take() {
                self.retired.push(id);
            }
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("test movie"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: buzz_render::RENDER_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = egui_renderer.register_native_texture(
                &gpu.device,
                &view,
                wgpu::FilterMode::Linear,
            );
            self.target = Some((texture, view, id, width, height));
        }

        let frame_to_draw = self.frame;
        let reel = reel_of(&self.scenes);
        let Some((scene, local)) = reel.at_clamped(frame_to_draw) else {
            return;
        };
        let Some((_, view, _, _, _)) = &self.target else {
            return;
        };
        if crate::preview::render_export_frame(gpu, vello, scene, local, width, height, cache, view)
            .is_some()
        {
            self.drawn = Some(frame_to_draw);
        }
    }

    /// Close the movie, handing back every texture it holds so the caller can
    /// free them once the renderer is reachable.
    pub fn close(mut self) -> Vec<egui::TextureId> {
        if let Some((_, _, id, _, _)) = self.target.take() {
            self.retired.push(id);
        }
        self.retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LayerKind;

    fn scene_with_frames(frames: u32) -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(frames.saturating_sub(1));
        });
        scene
    }

    #[test]
    fn opening_an_empty_document_gives_nothing_to_play() {
        // A default scene has one frame — not empty — so build one with no
        // frames at all to exercise the refusal.
        let empty = Scene::default();
        let reel = buzz_export::Reel::of([&empty]);
        assert!(!reel.is_empty(), "a default scene always has a frame");

        assert!(
            TestMovie::open(Vec::new(), 0).is_none(),
            "no scenes at all is nothing to play"
        );
    }

    #[test]
    fn test_movie_advances_and_loops() {
        let scenes = vec![(scene_with_frames(4), 4)];
        let mut movie = TestMovie::open(scenes, 0).expect("something to play");
        assert_eq!(movie.frame(), 0);
        assert_eq!(movie.frames(), 4);

        // A whole second at the document's frame rate crosses the four-frame
        // film more than once, and lands back inside it rather than past the
        // end.
        movie.advance(1.0, None);
        assert!(movie.frame() < movie.frames(), "the film always loops");
    }

    #[test]
    fn test_movie_follows_the_audio_clock() {
        let scenes = vec![(scene_with_frames(10), 10)];
        let mut movie = TestMovie::open(scenes, 0).expect("something to play");

        // The audio clock says frame 6 of the active scene; the picture
        // follows it exactly, ignoring the wall clock entirely.
        movie.advance(10.0, Some(6));
        assert_eq!(movie.frame(), 6);
    }
}
