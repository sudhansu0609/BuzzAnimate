//! Scripts that ship with the editor.
//!
//! An empty code box teaches nobody the API. These are the first thing the
//! Actions panel offers, and they are chosen to show the four things a script
//! is actually for: drawing more shapes than a hand would, arranging what is
//! already there, computing positions, and reporting on a document.
//!
//! They live here rather than in the panel so that a test can **run** them.
//! Sample code that no longer works against the API is worse than none: the
//! user takes it as the reference and blames their own typing.

/// A named script, ready to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Menu label.
    pub name: &'static str,
    /// One line saying what it does, for the tooltip.
    pub summary: &'static str,
    pub source: &'static str,
}

/// Every built-in sample, in the order the panel lists them.
pub fn samples() -> &'static [Sample] {
    SAMPLES
}

const SAMPLES: &[Sample] = &[
    Sample {
        name: "Describe this document",
        summary: "Reads the stage, timeline and library, and changes nothing",
        source: r#"// Reading only — this leaves the document exactly as it was.
var d = fl.getDocumentDOM();
var t = d.getTimeline();

fl.trace('Stage:  ' + d.width + ' x ' + d.height + ' at ' + d.frameRate + ' fps');
fl.trace('Layers: ' + t.layerCount + '   Frames: ' + t.frameCount);
fl.trace('Library items: ' + d.library.itemCount);

for (var i = 0; i < t.layerCount; i++) {
    var layer = t.layers[i];
    fl.trace('  ' + i + '. ' + layer.name +
             (layer.visible ? '' : '  (hidden)') +
             (layer.locked ? '  (locked)' : '') +
             (layer.depth ? '  depth ' + layer.depth : ''));
}
"#,
    },
    Sample {
        name: "Grid of squares",
        summary: "Draws a 8 x 5 grid — one undo step for all forty",
        source: r#"var d = fl.getDocumentDOM();
d.setFillColor('#4A90D9');

var size = 50, gap = 10, columns = 8, rows = 5;
for (var y = 0; y < rows; y++) {
    for (var x = 0; x < columns; x++) {
        var left = 20 + x * (size + gap);
        var top  = 20 + y * (size + gap);
        d.addNewRectangle({left: left, top: top, right: left + size, bottom: top + size});
    }
}
fl.trace('drew ' + (columns * rows) + ' squares');
"#,
    },
    Sample {
        name: "Ring of dots",
        summary: "Places ovals around a circle — the kind of maths a hand cannot do",
        source: r#"var d = fl.getDocumentDOM();
d.setFillColor('#E8734A');

var count = 24;
var cx = d.width / 2, cy = d.height / 2;
var radius = Math.min(cx, cy) * 0.7;

for (var i = 0; i < count; i++) {
    var angle = (i / count) * Math.PI * 2;
    var x = cx + Math.cos(angle) * radius;
    var y = cy + Math.sin(angle) * radius;
    d.addNewOval({left: x - 9, top: y - 9, right: x + 9, bottom: y + 9});
}
fl.trace('placed ' + count + ' dots around the stage centre');
"#,
    },
    Sample {
        name: "Parallax layer stack",
        summary: "Adds named layers and spreads them in depth for a camera pan",
        source: r#"// Layer depth is BuzzAnimate's own: 0 is the focal plane, larger is
// further from the camera and therefore drawn smaller and panning less.
var t = fl.getDocumentDOM().getTimeline();

var names = ['Foreground', 'Stage', 'Trees', 'Hills', 'Sky'];
for (var i = 0; i < names.length; i++) {
    t.addNewLayer(names[i]);
}

// The front of the stack sits nearest the camera.
for (var i = 0; i < t.layerCount; i++) {
    t.layers[i].depth = i * 400;
}
fl.trace('arranged ' + t.layerCount + ' layers from 0 to ' +
         ((t.layerCount - 1) * 400) + ' units deep');
"#,
    },
    Sample {
        name: "Make a symbol from the selection",
        summary: "Wraps whatever is selected in a Graphic symbol",
        source: r#"var d = fl.getDocumentDOM();

if (d.selection.length === 0) {
    fl.trace('Select something on the stage first, then run this again.');
} else {
    d.convertToSymbol('graphic', 'Scripted Symbol');
    fl.trace('the library now holds ' + d.library.itemCount + ' item(s)');
}
"#,
    },
    Sample {
        name: "Direct a scene from a brief",
        summary: "A few lines of prose become a staged, animated shot",
        source: r#"// The director reads plain sentences and does what an animator would do
// with them: set the scene, stand the cast in it, and write the walks,
// talks and waits onto the timeline as ordinary pose keyframes.
//
// Everything it leaves behind is editable — one Ctrl+Z takes all of it back.
var d = fl.getDocumentDOM();

var frames = d.direct(
    'Night. Ana walks in from the left.\n' +
    'Ana talks to Ben. Ben listens.\n' +
    'Ben walks off right.'
);

fl.trace('The shot came out ' + frames + ' frames long.');
fl.trace('Layers now: ' + d.getTimeline().layerCount);
"#,
    },
    Sample {
        name: "Shoot it: a camera move and a focus pull",
        summary: "Keys the camera across the shot and pulls focus as it goes",
        source: r#"// A push in, with the focus travelling from the background to the
// foreground as it arrives — the move that carries the eye to a face.
var d = fl.getDocumentDOM();
var last = Math.max(24, d.getTimeline().frameCount - 1);

d.camera.setKey(0,    { x: d.width / 2, y: d.height / 2, zoom: 1.0 });
d.camera.setKey(last, { x: d.width / 2, y: d.height / 2, zoom: 1.6 });

// Focus starts on the layer at depth 600 and ends on the stage plane.
d.camera.setFocusKey(0,    600, 0.03);
d.camera.setFocusKey(last, 0,   0.03);

// And a shutter, so fast motion smears rather than strobing.
d.camera.setShutter(0.5, 8);

fl.trace('Shot keyed over ' + last + ' frames.');
"#,
    },
    Sample {
        name: "Give everything a little life",
        summary: "Puts a wiggle on the whole selection so nothing sits dead still",
        source: r#"// The cheapest thing that stops a held drawing looking like a mistake.
// Select what should breathe, then run this.
var d = fl.getDocumentDOM();

if (d.selection.length === 0) {
    d.selectAll();
}

if (d.selection.length === 0) {
    fl.trace('Nothing on the stage to wiggle yet — draw something first.');
} else {
    fl.trace('Wiggling ' + d.selection.length + ' object(s).');
    // Small and slow: this is life, not a shiver.
    d.addWiggle(3, 1.5);
}
"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Limits, ScriptContext, run};
    use buzz_scene::{LayerKind, Scene};

    fn document() -> Scene {
        let mut scene = Scene::default();
        scene.add_layer("Second", LayerKind::Normal);
        scene
    }

    /// The point of keeping the samples in Rust: they are executed, so a
    /// change to the API that breaks one fails the build rather than waiting
    /// to embarrass the user who trusted it.
    #[test]
    fn every_sample_runs_without_error() {
        for sample in samples() {
            let mut scene = document();
            let out = run(
                &mut scene,
                ScriptContext::default(),
                sample.source,
                &Limits::default(),
            );
            assert!(
                out.succeeded(),
                "sample {:?} failed: {:?}",
                sample.name,
                out.error
            );
            assert!(
                !out.trace.is_empty(),
                "sample {:?} should say what it did",
                sample.name
            );
        }
    }

    /// The first sample is offered as a safe thing to try. If it edited the
    /// document, that promise would be false.
    #[test]
    fn the_describing_sample_changes_nothing() {
        let mut scene = document();
        let before = scene.clone();
        let out = run(
            &mut scene,
            ScriptContext::default(),
            samples()[0].source,
            &Limits::default(),
        );

        assert!(out.succeeded(), "{:?}", out.error);
        assert!(!out.changed);
        assert_eq!(scene, before);
    }

    #[test]
    fn the_drawing_samples_actually_draw() {
        for name in ["Grid of squares", "Ring of dots"] {
            let sample = samples().iter().find(|s| s.name == name).expect(name);
            let mut scene = document();
            let out = run(
                &mut scene,
                ScriptContext::default(),
                sample.source,
                &Limits::default(),
            );

            assert!(out.succeeded(), "{name}: {:?}", out.error);
            assert!(out.changed, "{name} should change the document");
            assert!(scene.shape_count_at(0) > 10, "{name} drew too little");
        }
    }

    #[test]
    fn samples_are_named_distinctly_and_described() {
        for (i, sample) in samples().iter().enumerate() {
            assert!(!sample.name.is_empty());
            assert!(!sample.summary.is_empty(), "{} has no summary", sample.name);
            for other in &samples()[i + 1..] {
                assert_ne!(sample.name, other.name, "two samples share a name");
            }
        }
    }
}
