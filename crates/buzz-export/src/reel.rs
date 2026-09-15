//! The scenes a film is made of, in the order they play.
//!
//! # Why this exists
//!
//! A document holds several named scenes — the shots of the film — and until
//! now an export rendered exactly one of them: whichever was open. So a
//! three-scene conversation exported as the middle of the conversation, and
//! the only way to get a whole film out was to build it as one enormous
//! timeline, pasting each shot onto the end of the last. That is a real thing
//! people were doing, and this is the piece that makes it unnecessary.
//!
//! # It is the loop playlist, generalised
//!
//! Export already resolved its frame numbers through a *playlist*: a document
//! with a looping section is longer than its timeline, and
//! [`buzz_scene::Scene::playlist`] maps each frame of the finished film to the
//! timeline frame it should be drawn from. A reel is that same idea across
//! scenes — each film frame knows **which scene** as well as which frame — so
//! looping keeps working untouched inside each scene, and every export path
//! keeps the shape it already had.
//!
//! ```text
//! Kitchen  0 1 2 3        Hallway 0 1 2      <- three timelines
//! film     0 1 2 3        4 5 6              <- one film
//! ```
//!
//! # A film has one stage
//!
//! Scenes can in principle disagree about stage size, background and frame
//! rate. A film cannot: it is one file, at one size, at one rate. The **first**
//! scene decides — see [`Reel::lead`] — because it is the one whose settings
//! the export dialog was showing when the numbers were chosen.

use buzz_scene::Scene;

/// An ordered run of scenes, and where each frame of the film comes from.
///
/// Borrowed rather than owned: the caller already holds the snapshot it wants
/// rendered, and a film of long scenes is not worth copying twice.
#[derive(Debug, Clone)]
pub struct Reel<'a> {
    scenes: Vec<&'a Scene>,
    /// Film frame → the scene it comes from, and which of that scene's
    /// timeline frames to draw.
    ///
    /// Materialised for the same reason [`buzz_scene::Scene::playlist`] is:
    /// every lookup afterwards is an index, and the length of the film is a
    /// number rather than a sum that has to be recomputed.
    playlist: Vec<(usize, u32)>,
    /// The first film frame of each scene, for anything that has to place a
    /// scene's own events — a soundtrack, mostly.
    starts: Vec<u32>,
}

impl<'a> Reel<'a> {
    /// A film of one scene, which is what every export was before scenes could
    /// be strung together.
    pub fn single(scene: &'a Scene) -> Self {
        Self::of([scene])
    }

    /// A film of scenes, in the order they play.
    pub fn of(scenes: impl IntoIterator<Item = &'a Scene>) -> Self {
        let scenes: Vec<&Scene> = scenes.into_iter().collect();
        let mut playlist = Vec::new();
        let mut starts = Vec::with_capacity(scenes.len());

        for (index, scene) in scenes.iter().enumerate() {
            starts.push(playlist.len() as u32);
            // Each scene's own playlist, so a looping section inside a shot
            // repeats within that shot and nowhere else.
            for frame in scene.playlist() {
                playlist.push((index, frame));
            }
        }

        Self {
            scenes,
            playlist,
            starts,
        }
    }

    /// How long the finished film is, in frames.
    pub fn frames(&self) -> u32 {
        self.playlist.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.playlist.is_empty()
    }

    /// What to draw for one frame of the film: a scene, and a frame of it.
    ///
    /// `None` past the end, which is a range asking for more film than there
    /// is rather than something to paper over.
    pub fn at(&self, film_frame: u32) -> Option<(&'a Scene, u32)> {
        let (scene, frame) = *self.playlist.get(film_frame as usize)?;
        Some((self.scenes[scene], frame))
    }

    /// The frame to draw, clamped to the film rather than refused.
    ///
    /// For the callers that would otherwise have to invent a fallback — the
    /// export loops, which already had one. An empty reel has nothing to
    /// clamp to and still returns `None`.
    pub fn at_clamped(&self, film_frame: u32) -> Option<(&'a Scene, u32)> {
        if self.playlist.is_empty() {
            return None;
        }
        let last = self.playlist.len() as u32 - 1;
        self.at(film_frame.min(last))
    }

    /// The scene whose stage the film takes: size, background and frame rate.
    /// See the module header for why it is the first one.
    pub fn lead(&self) -> Option<&'a Scene> {
        self.scenes.first().copied()
    }

    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    /// Every scene, with the film frame it starts at.
    pub fn scenes(&self) -> impl Iterator<Item = (&'a Scene, u32)> + '_ {
        self.scenes.iter().copied().zip(self.starts.iter().copied())
    }

    /// Where a scene's own timeline frame first lands in the film.
    ///
    /// "First" because a looping section puts one timeline frame at several
    /// places in the film; a sound cued on it plays when the section is first
    /// reached, which is the reading that matches what is heard while
    /// animating.
    pub fn film_frame_of(&self, scene: usize, timeline_frame: u32) -> Option<u32> {
        let start = *self.starts.get(scene)? as usize;
        let end = self
            .starts
            .get(scene + 1)
            .map(|s| *s as usize)
            .unwrap_or(self.playlist.len());
        self.playlist[start..end]
            .iter()
            .position(|(_, frame)| *frame == timeline_frame)
            .map(|offset| (start + offset) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::{LayerKind, LoopRegion};

    /// A scene `frames` long, with a layer stretched to fill it.
    fn scene(frames: u32) -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(frames.saturating_sub(1));
        });
        scene
    }

    /// The whole point: several scenes play one after another, and the film is
    /// as long as all of them together.
    #[test]
    fn scenes_play_one_after_another() {
        let (a, b, c) = (scene(4), scene(2), scene(3));
        let reel = Reel::of([&a, &b, &c]);

        assert_eq!(reel.frames(), 9, "four, then two, then three");
        assert_eq!(reel.scene_count(), 3);

        // Film frame 0..4 is the first scene's own frames 0..4.
        for f in 0..4 {
            let (which, frame) = reel.at(f).expect("in the film");
            assert!(std::ptr::eq(which, &a), "frame {f} came from the wrong scene");
            assert_eq!(frame, f);
        }
        // Then the second scene, from *its* frame zero.
        let (which, frame) = reel.at(4).unwrap();
        assert!(std::ptr::eq(which, &b));
        assert_eq!(frame, 0, "a new scene starts at its own beginning");
        // And the third.
        let (which, frame) = reel.at(8).unwrap();
        assert!(std::ptr::eq(which, &c));
        assert_eq!(frame, 2);

        assert!(reel.at(9).is_none(), "past the end is past the end");
    }

    /// A film of one scene is exactly what an export was before reels existed.
    #[test]
    fn one_scene_is_the_film_it_always_was() {
        let only = scene(5);
        let reel = Reel::single(&only);
        assert_eq!(reel.frames(), 5);
        for f in 0..5 {
            assert_eq!(reel.at(f).unwrap().1, f);
        }
    }

    /// **Looping still works, and works per scene.** A section that repeats
    /// inside one shot repeats inside that shot, and the shots after it start
    /// where the repeats leave off.
    #[test]
    fn a_looping_scene_repeats_within_its_own_stretch_of_film() {
        let mut looped = scene(4);
        *looped.looping_mut() = LoopRegion {
            enabled: true,
            start: 1,
            end: 2,
            repeats: 3,
        };
        let after = scene(2);
        let reel = Reel::of([&looped, &after]);

        // 0, then 1-2 three times, then 3 — eight frames — then the next scene.
        assert_eq!(reel.frames(), 8 + 2);
        let frames: Vec<u32> = (0..8).map(|f| reel.at(f).unwrap().1).collect();
        assert_eq!(frames, vec![0, 1, 2, 1, 2, 1, 2, 3]);

        let (which, frame) = reel.at(8).unwrap();
        assert!(std::ptr::eq(which, &after), "the next shot follows the repeats");
        assert_eq!(frame, 0);
    }

    /// A sound cued in the third scene has to play in the third scene, not at
    /// the same number of seconds into the film.
    #[test]
    fn a_scenes_own_frame_maps_to_where_it_lands_in_the_film() {
        let (a, b, c) = (scene(4), scene(2), scene(3));
        let reel = Reel::of([&a, &b, &c]);

        assert_eq!(reel.film_frame_of(0, 0), Some(0));
        assert_eq!(reel.film_frame_of(1, 1), Some(5));
        assert_eq!(reel.film_frame_of(2, 2), Some(8));
        assert_eq!(reel.film_frame_of(2, 9), None, "no such frame in that scene");
        assert_eq!(reel.film_frame_of(7, 0), None, "no such scene");
    }

    /// The first scene decides the stage, because a film is one file at one
    /// size.
    #[test]
    fn the_first_scene_leads() {
        let (a, b) = (scene(2), scene(2));
        let reel = Reel::of([&a, &b]);
        assert!(std::ptr::eq(reel.lead().unwrap(), &a));

        let starts: Vec<u32> = reel.scenes().map(|(_, start)| start).collect();
        assert_eq!(starts, vec![0, 2]);
    }

    /// A reel of nothing does not panic; it is simply an empty film.
    #[test]
    fn an_empty_reel_is_empty_rather_than_broken() {
        let reel = Reel::of([]);
        assert!(reel.is_empty());
        assert_eq!(reel.frames(), 0);
        assert!(reel.at(0).is_none());
        assert!(reel.at_clamped(0).is_none());
        assert!(reel.lead().is_none());
    }

    /// Past the end, the clamped lookup gives the last frame of the film —
    /// what the export loops used to do with their own fallback.
    #[test]
    fn the_clamped_lookup_holds_on_the_last_frame() {
        let (a, b) = (scene(3), scene(2));
        let reel = Reel::of([&a, &b]);
        let (which, frame) = reel.at_clamped(99).unwrap();
        assert!(std::ptr::eq(which, &b));
        assert_eq!(frame, 1, "the last frame of the last scene");
    }
}
