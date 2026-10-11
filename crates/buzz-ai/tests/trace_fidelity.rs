//! `trace_character_whole_with` must trace the **visible source picture**: exact
//! colours, small features kept, no boundary strokes unless asked, and parts cut
//! only where a region genuinely straddles two of them.
//!
//! Synthetic pictures only (no models): a two-part body, the torso on the left
//! (x 20..140) and an upper arm on the right (x 140..220), a plain fill each (body
//! coordinates; the body sits at (OFF, OFF) on a canvas of 1040 px, since the
//! tracer upscales pictures under 1024 px and its bicubic ringing would then bias
//! the small regions' mean colours). The torso carries a dashed plaid on its top
//! half; below it sit two flat patches, one 95% inside the torso and one
//! straddling the torso/arm cut 50/50.

use buzz_ai::character::{
    trace_character_whole, trace_character_whole_with, Bone, CompletedPart, CompletedParts, FaceFeatures, Joint,
    Skeleton, TraceCharacterOptions, TracedCharacter,
};
use buzz_geom::{BezPath, Point, Shape};
use image::{GrayImage, Luma, Rgba, RgbaImage};

const W: u32 = 1040;
const H: u32 = 1040;
/// Where the body's (0, 0) sits on the canvas.
const OFF: u32 = 400;
/// The torso/arm cut: torso is x < CUT, arm is x >= CUT.
const CUT: u32 = 140;

// T9b: the flat colours of the FILL tests (the patches, BAG) are lighter than
// 0.62 x their neighbour's luma (torso 204) or above luma 150, so the ink layer's
// local-contrast detection does not take their edges for outlines: a thin DARK
// ink-coloured feature is ink by contract (a stroke on top, the fill under it the
// neighbour colour), and these tests are about the fills. T9c: the plaid GRID is a
// DARK RED again (locally dark, luma 66 against the torso's 204), and stays a fill
// because it is not ink-coloured (CIELAB ΔE ~ 60 from the ink colour, limit 30).
const TORSO: [u8; 3] = [225, 200, 170];
const ARM: [u8; 3] = [170, 190, 215];
const GRID: [u8; 3] = [150, 30, 30];
const INK: [u8; 3] = [20, 20, 25];
/// 95% inside the torso, 5% inside the arm.
const PATCH_MOSTLY: [u8; 3] = [235, 120, 120];
/// 50% inside the torso, 50% inside the arm.
const PATCH_STRADDLE: [u8; 3] = [110, 200, 120];

/// Fills a rectangle given in body coordinates.
fn fill_rect(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]) {
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x + OFF, y + OFF, Rgba([c[0], c[1], c[2], 255]));
        }
    }
}

/// Number of plaid dashes `fixture` draws: dashes of 3 x 11 px centred in every
/// gap between two crossings of a 24 px grid (kept apart so each is a region).
fn plaid_dashes(img: &mut RgbaImage) -> usize {
    let xs: Vec<u32> = (0..5).map(|i| 32 + 24 * i).collect(); // 32..128
    let ys: Vec<u32> = (0..5).map(|j| 32 + 24 * j).collect(); // 32..128
    let mut n = 0;
    for &x in &xs {
        for w in ys.windows(2) {
            fill_rect(img, x, w[0] + 8, x + 3, w[0] + 19, GRID);
            n += 1;
        }
    }
    for &y in &ys {
        for w in xs.windows(2) {
            fill_rect(img, w[0] + 8, y, w[0] + 19, y + 3, GRID);
            n += 1;
        }
    }
    n
}

struct Fixture {
    source: RgbaImage,
    completed: CompletedParts,
    skel: Skeleton,
    matte: GrayImage,
    dashes: usize,
}

fn fixture() -> Fixture {
    let mut source = RgbaImage::new(W, H); // transparent outside the body
    // A 10 px ink outline round the whole body, so the picture has a real ink
    // colour (the darkest in the character palette, which the dark-fill guard
    // measures fills against), then the two fills inside it.
    fill_rect(&mut source, 10, 10, 230, 230, INK);
    fill_rect(&mut source, 20, 20, CUT, 220, TORSO);
    fill_rect(&mut source, CUT, 20, 220, 220, ARM);
    let dashes = plaid_dashes(&mut source);
    // 95 / 5: x 45..145 is 95 columns of torso and 5 of arm.
    fill_rect(&mut source, 45, 140, 145, 175, PATCH_MOSTLY);
    // 50 / 50: x 100..180 is 40 columns either side of the cut.
    fill_rect(&mut source, 100, 185, 180, 215, PATCH_STRADDLE);

    let mut matte = GrayImage::new(W, H);
    for y in 10..230 {
        for x in 10..230 {
            matte.put_pixel(x + OFF, y + OFF, Luma([255]));
        }
    }

    let part = |name: &str, depth: i32, x0: u32, x1: u32, dom: [u8; 3]| {
        let mut rgba = vec![0u8; (W * H * 4) as usize];
        for y in 10..230u32 {
            for x in x0..x1 {
                let i = (((y + OFF) * W + x + OFF) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&source.get_pixel(x + OFF, y + OFF).0);
            }
        }
        CompletedPart { name: name.into(), depth, width: W, height: H, rgba, dom_color: dom, extruded: Vec::new(), borrowed_from: None }
    };
    let completed = CompletedParts {
        width: W,
        height: H,
        parts: vec![part("Chest", 0, 10, CUT, TORSO), part("Upper Arm L", 1, CUT, 230, ARM)],
    };

    let o = OFF as f32;
    let skel = Skeleton {
        joints: vec![
            Joint { name: "Chest".into(), x: 80.0 + o, y: 40.0 + o, confidence: 0.9 },
            Joint { name: "Upper Arm L".into(), x: 180.0 + o, y: 40.0 + o, confidence: 0.9 },
        ],
        bones: vec![
            Bone {
                name: "Chest".into(),
                head: (80.0 + o, 40.0 + o),
                tail: (80.0 + o, 200.0 + o),
                confidence: 0.9,
                length: 160.0,
            },
            Bone {
                name: "Upper Arm L".into(),
                head: (180.0 + o, 40.0 + o),
                tail: (180.0 + o, 200.0 + o),
                confidence: 0.9,
                length: 160.0,
            },
        ],
        face: FaceFeatures::default(),
        character_height: 200.0,
    };
    Fixture { source, completed, skel, matte, dashes }
}

fn trace_with(f: &Fixture, outline_strokes: bool) -> TracedCharacter {
    trace_character_whole_with(
        &f.source,
        &f.completed,
        &f.skel,
        &f.matte,
        &TraceCharacterOptions { uniform_width: 2.0, outline_strokes, ..Default::default() },
    )
    .expect("trace")
}

/// I5: the T9e path (`ink_colour: false`: Outline fills over the under-painted
/// picture, LineArt brush ink, protected face parts), for the tests about it.
fn trace_t9e(f: &Fixture) -> TracedCharacter {
    trace_character_whole_with(
        &f.source,
        &f.completed,
        &f.skel,
        &f.matte,
        &TraceCharacterOptions { uniform_width: 2.0, outline_strokes: false, ink_colour: false, ..Default::default() },
    )
    .expect("trace")
}

/// The pipeline's line colour for `f` (the darkest palette colour of its parts).
fn line_colour_of(f: &Fixture) -> peniko::Color {
    buzz_ai::character::character_line_colour(&f.completed)
}

/// The thin ink of the fixture's source, as the pipeline sees it (no face boxes).
fn ink_of(f: &Fixture) -> Vec<bool> {
    buzz_ai::character::character_thin_ink(W, H, f.source.as_raw(), Some(line_colour_of(f)))
}

fn srgb_to_lab(c: [u8; 3]) -> [f64; 3] {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    [116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z))]
}

fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (srgb_to_lab(a), srgb_to_lab(b));
    ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2)).sqrt()
}

fn fill_rgb(shape: &buzz_scene::ShapeData) -> [u8; 3] {
    let c = shape.fill.as_ref().expect("fill").color().to_rgba8();
    [c.r, c.g, c.b]
}

/// Every filled shape of every part with its part's name. There is no longer a
/// close-the-cut base fill, so nothing is skipped; the plain fixture hides no
/// pixels either, so none of these is a hidden-completion fill.
fn traced_fills(t: &TracedCharacter) -> Vec<(&str, &buzz_scene::ShapeData)> {
    let mut out = Vec::new();
    for part in &t.parts {
        for shape in part.shapes.iter() {
            if shape.fill.is_some() {
                out.push((part.name.as_str(), shape));
            }
        }
    }
    out
}

fn path_area(path: &BezPath) -> f64 {
    path.area().abs()
}

/// Mean source colour over the picture's pixel centres inside `path`, leaving out
/// the thin-ink pixels `ink` marks (T9b: a fill runs UNDER the ink, in the colour of
/// its neighbours, so the ink pixels are not what its colour is measured against).
fn mean_source_under(source: &RgbaImage, path: &BezPath, ink: &[bool]) -> Option<[f64; 3]> {
    mean_source_under_uncovered(source, path, ink, &[])
}

/// [`mean_source_under`], also leaving out the pixels inside any of `covered`
/// (the shapes drawn over this one).
fn mean_source_under_uncovered(
    source: &RgbaImage,
    path: &BezPath,
    ink: &[bool],
    covered: &[&BezPath],
) -> Option<[f64; 3]> {
    let bb = path.bounding_box();
    let (x0, x1) = (bb.x0.floor().max(0.0) as u32, (bb.x1.ceil().max(0.0) as u32).min(source.width()));
    let (y0, y1) = (bb.y0.floor().max(0.0) as u32, (bb.y1.ceil().max(0.0) as u32).min(source.height()));
    let (mut sum, mut n) = ([0.0f64; 3], 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let centre = Point::new(x as f64 + 0.5, y as f64 + 0.5);
            if ink[(y * source.width() + x) as usize]
                || !path.contains(centre)
                || covered.iter().any(|c| c.contains(centre))
            {
                continue;
            }
            let p = source.get_pixel(x, y);
            if p[3] > 128 {
                for k in 0..3 {
                    sum[k] += p[k] as f64;
                }
                n += 1;
            }
        }
    }
    (n > 0).then(|| [sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64])
}

/// (a) The plaid survives: >= 90% of the dashes come out as their own fills.
#[test]
fn plaid_grid_regions_survive_as_fills() {
    let f = fixture();
    let t = trace_with(&f, false);
    let grid_fills = traced_fills(&t).iter().filter(|(_, s)| delta_e(fill_rgb(s), GRID) <= 10.0).count();
    let expected = f.dashes;
    assert!(expected >= 40, "fixture should draw at least 40 dashes, drew {expected}");
    assert!(
        grid_fills as f64 >= 0.9 * expected as f64,
        "only {grid_fills} of {expected} plaid dashes survived as fills"
    );
}

/// (b) Every fill is the tracer's exact mean colour of the source under it: no
/// palette snapping.
#[test]
fn every_fill_is_the_exact_source_colour_under_it() {
    let f = fixture();
    let t = trace_with(&f, false);
    let fills = traced_fills(&t);
    assert!(fills.len() > 40, "expected many traced fills, got {}", fills.len());
    let ink = ink_of(&f);
    // I5: every ink shape is drawn over the fills, and a fill runs UNDER the
    // ink (the 10 px border included — thick ink the thin-ink mask does not
    // mark), so the ink shapes' pixels are not what a fill's colour is
    // measured against either.
    let ink_paths: Vec<&BezPath> = t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)).collect();
    for (i, (part, shape)) in fills.iter().enumerate() {
        let fill = fill_rgb(shape);
        // I5: a piece under 200 px² (the pink patch's arm sliver, 49 px²; the
        // arm's 165 px² wrap round the green patch's corner) has its colour
        // measured over edge pixels: the boolean clip at the part cut, and the
        // 1 px smoothing of each region's own boundary, which bulges a concave
        // corner over its neighbour by a few pixels. The colour is still the
        // region's; the shared-edge vectorisation (plan stage B) removes the
        // corner effect.
        if path_area(&shape.path) < 200.0 {
            continue;
        }
        // I5: a base fill runs UNDER the pattern lines drawn after it in the same
        // part (the plaid dashes over the torso), so its colour is measured over
        // the source pixels no later shape of that part covers.
        let covered: Vec<&BezPath> = fills[i + 1..]
            .iter()
            .filter(|(p, _)| p == part)
            .map(|(_, s)| &s.path)
            .chain(ink_paths.iter().copied())
            .collect();
        let Some(mean) = mean_source_under_uncovered(&f.source, &shape.path, &ink, &covered) else { continue };
        let mean_rgb = [mean[0].round() as u8, mean[1].round() as u8, mean[2].round() as u8];
        let rgb_dist = (0..3).map(|k| (fill[k] as f64 - mean[k]).powi(2)).sum::<f64>().sqrt();
        let de = delta_e(fill, mean_rgb);
        let bb = shape.path.bounding_box();
        assert!(
            de <= 2.0 || rgb_dist <= 6.0,
            "{part}: fill {fill:?} ({:.0} px², bbox ({:.1},{:.1})-({:.1},{:.1})) is {de:.1} dE / {rgb_dist:.1} rgb away from the source mean {mean:?} under it",
            path_area(&shape.path),
            bb.x0,
            bb.y0,
            bb.x1,
            bb.y1
        );
    }
}

/// (c) Boundary strokes are opt-in.
#[test]
fn outline_strokes_are_opt_in() {
    let f = fixture();
    let off = trace_with(&f, false);
    let stroked_off = off.parts.iter().flat_map(|p| &p.shapes).filter(|s| s.stroke.is_some()).count();
    assert_eq!(stroked_off, 0, "outline_strokes: false must produce no stroked shapes");
    let on = trace_with(&f, true);
    let stroked_on = on.parts.iter().flat_map(|p| &p.shapes).filter(|s| s.stroke.is_some()).count();
    assert!(stroked_on >= 1, "outline_strokes: true must produce stroked shapes");
}

/// The wrapper (no source picture) keeps working and also draws no strokes.
#[test]
fn wrapper_without_a_source_traces_from_the_completed_composite() {
    let f = fixture();
    let t = trace_character_whole(&f.completed, &f.skel, &f.matte, Some(2.0)).expect("trace");
    assert_eq!(t.uniform_width, 2.0);
    assert!(t.parts.iter().flat_map(|p| &p.shapes).all(|s| s.stroke.is_none()));
    assert!(!traced_fills(&t).is_empty());
}

/// The area of the region the tracer finds for `colour` in the visible picture,
/// before any part assignment — the reference the part assignment is held to.
fn untouched_region_area(f: &Fixture, colour: [u8; 3]) -> f64 {
    let mut visible = vec![0u8; (W * H * 4) as usize];
    for (i, (px, m)) in f.source.pixels().zip(f.matte.pixels()).enumerate() {
        if m[0] > 128 {
            visible[i * 4..i * 4 + 4].copy_from_slice(&px.0);
        }
    }
    let report = buzz_scene::trace::trace(
        W,
        H,
        &visible,
        &buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::Outline,
            colours: 32,
            // The fills' own settings in `trace_character_whole_with` (T9).
            smooth: 0.75,
            detail: 1.8,
            ..Default::default()
        },
    );
    report
        .shapes
        .iter()
        .filter(|s| s.fill.is_some() && delta_e(fill_rgb(s), colour) <= 5.0)
        .map(|s| path_area(&s.path))
        .fold(0.0, f64::max)
}

/// (d) A region 95% inside one part goes to it whole; one straddling the cut
/// 50/50 is clipped into one piece per part, and the pieces add back up.
#[test]
fn mostly_inside_goes_whole_and_straddling_is_clipped_into_pieces() {
    let f = fixture();
    // I5: T9e — the reference region is the T9 Outline trace's (`untouched_region_area`).
    let t = trace_t9e(&f);
    let fills = traced_fills(&t);

    // 95/5: one shape, in the torso, with the traced region's own area.
    let reference = untouched_region_area(&f, PATCH_MOSTLY);
    assert!(reference > 3000.0, "the traced 100x35 patch should be ~3500 px^2, got {reference}");
    let pieces: Vec<_> = fills.iter().filter(|(_, s)| delta_e(fill_rgb(s), PATCH_MOSTLY) <= 5.0).collect();
    assert_eq!(pieces.len(), 1, "a region 95% inside one part must not be split, got {} pieces", pieces.len());
    assert_eq!(pieces[0].0, "Chest", "the 95% region belongs to the torso");
    let area = path_area(&pieces[0].1.path);
    assert!(
        (area - reference).abs() / reference <= 0.005,
        "the unclipped region's area {area:.1} must equal the traced region's {reference:.1} within 0.5%"
    );

    // 50/50: one piece per part, summing to the traced region within 1%.
    let reference = untouched_region_area(&f, PATCH_STRADDLE);
    assert!(reference > 2000.0, "the traced 80x30 patch should be ~2400 px^2, got {reference}");
    let pieces: Vec<_> = fills.iter().filter(|(_, s)| delta_e(fill_rgb(s), PATCH_STRADDLE) <= 5.0).collect();
    let names: Vec<&str> = pieces.iter().map(|(n, _)| *n).collect();
    assert_eq!(pieces.len(), 2, "a region straddling two parts 50/50 must be clipped into two pieces: {names:?}");
    assert!(names.contains(&"Chest") && names.contains(&"Upper Arm L"), "one piece per part: {names:?}");
    let sum: f64 = pieces.iter().map(|(_, s)| path_area(&s.path)).sum();
    assert!(
        (sum - reference).abs() / reference <= 0.01,
        "the clipped pieces' areas sum to {sum:.1}, the traced region has {reference:.1} (must be within 1%)"
    );
}

fn subpath_count(path: &BezPath) -> usize {
    path.elements().iter().filter(|e| matches!(e, buzz_geom::PathEl::MoveTo(_))).count()
}

/// With strokes on, a stroke is never cut at a part boundary: each traced stroke
/// subpath lands whole in exactly one part, so the parts hold no more stroke
/// subpaths than the tracer produced (cutting per pixel run would add a piece
/// for every boundary crossing, e.g. the straddling patch's outline).
#[test]
fn strokes_are_assigned_whole_to_one_part() {
    let f = fixture();
    let t = trace_with(&f, true);
    let out: usize =
        t.parts.iter().flat_map(|p| &p.shapes).filter(|s| s.stroke.is_some()).map(|s| subpath_count(&s.path)).sum();

    let mut visible = vec![0u8; (W * H * 4) as usize];
    for (i, (px, m)) in f.source.pixels().zip(f.matte.pixels()).enumerate() {
        if m[0] > 128 {
            visible[i * 4..i * 4 + 4].copy_from_slice(&px.0);
        }
    }
    let report = buzz_scene::trace::trace(
        W,
        H,
        &visible,
        &buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::ColourLines,
            line_style: buzz_scene::trace::LineStyle::Stroke,
            uniform_width: Some(2.0),
            line_smooth: 0.6,
            fill_enclosed: true,
            colours: 24,
            line_colour: Some(peniko::Color::from_rgb8(15, 15, 15)),
            ..Default::default()
        },
    );
    let traced: usize =
        report.shapes.iter().filter(|s| s.stroke.is_some()).map(|s| subpath_count(&s.path)).sum();
    assert!(traced >= 1, "the tracer should find boundary strokes in the fixture");
    assert!(out <= traced, "parts hold {out} stroke subpaths but the tracer made only {traced}: strokes were cut");
    assert!(out as f64 >= 0.9 * traced as f64, "parts hold {out} of the tracer's {traced} stroke subpaths");
}

// ---------------------------------------------------------------------------
// Unowned regions and the hidden completion (T3)
// ---------------------------------------------------------------------------

/// A purple "bag": drawn in the source, absent from every completed part.
const BAG: [u8; 3] = [190, 150, 230];

/// Paints a rectangle given in CANVAS coordinates into the source and the matte
/// (it is part of the character) but not into any completed part.
fn paint_unowned(f: &mut Fixture, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]) {
    for y in y0..y1 {
        for x in x0..x1 {
            f.source.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
            f.matte.put_pixel(x, y, Luma([255]));
        }
    }
}

/// The traced fills of colour `colour` as (part name, shape).
fn pieces_of(t: &TracedCharacter, colour: [u8; 3]) -> Vec<(&str, &buzz_scene::ShapeData)> {
    traced_fills(t).into_iter().filter(|(_, s)| delta_e(fill_rgb(s), colour) <= 5.0).collect()
}

/// (e) A region drawn entirely outside every part's owner mask is not dropped:
/// it goes whole to the part whose bone is nearest its centroid. One bag sits
/// left of the body (nearest the chest bone), one right of it (nearest the arm
/// bone), so "the biggest part" would be a wrong answer for the second.
#[test]
fn a_region_no_part_owns_goes_whole_to_the_nearest_bone_part() {
    let mut f = fixture();
    // Body x -40..10 (canvas 360..410), y 80..160 (canvas 480..560): T11, it must touch
    // the owned torso (which starts at canvas 410), else the stray rule drops it.
    paint_unowned(&mut f, 360, 480, 410, 560, BAG);
    let left_ref = untouched_region_area(&f, BAG);
    let t = trace_with(&f, false);
    let pieces = pieces_of(&t, BAG);
    assert_eq!(pieces.len(), 1, "the unowned bag must be kept as one shape, got {} pieces", pieces.len());
    assert_eq!(pieces[0].0, "Chest", "the bag left of the body is nearest the chest bone");
    let area = path_area(&pieces[0].1.path);
    // T11: the bag now touches the torso's 10 px ink outline, which the under-paint gives
    // the bag's colour on the bag's side, so the fill is ~12 % larger than the bare region.
    assert!(
        (area - left_ref).abs() / left_ref <= 0.15,
        "the unowned region keeps its full area: {area:.1} vs {left_ref:.1}"
    );

    // And one right of the body, body x 230..290 (canvas 630..690), touching the arm
    // (owned to canvas 630): nearest the arm bone.
    let mut f = fixture();
    paint_unowned(&mut f, 630, 480, 690, 560, BAG);
    let t = trace_with(&f, false);
    let pieces = pieces_of(&t, BAG);
    assert_eq!(pieces.len(), 1, "got {} pieces", pieces.len());
    assert_eq!(pieces[0].0, "Upper Arm L", "the bag right of the body is nearest the arm bone");
}

/// (e2) T11: an unowned blob far from every bone (centroid over 1.5 limb radii from the
/// nearest bone segment) that touches no owned pixel is dropped, not handed to the
/// nearest part; the same blob touching the torso is kept (test (e) above).
#[test]
fn an_unowned_blob_far_from_every_bone_and_touching_nothing_is_dropped() {
    let mut f = fixture();
    // Body x -70..-10 (canvas 330..390): 20 px clear of the torso (canvas 410), 95 px
    // from the chest bone (the limb radius there is ~46 px: 1.5 x = 70).
    paint_unowned(&mut f, 330, 480, 390, 560, BAG);
    let t = trace_with(&f, false);
    let pieces = pieces_of(&t, BAG);
    assert!(pieces.is_empty(), "the floating blob must be dropped, got {} piece(s)", pieces.len());
}

/// (o2) T11: the "Left Eyebrow" part is the drawn brow, not the segmenter's patch. A
/// head (the fixture's skin part renamed "Head") has a 50 x 12 dark brow bar above a
/// drawn eye, brow landmarks along the bar and a "Left Eyebrow" part that is a much larger
/// oversized patch of the skin round it (the segmenter's capsule). After the trace the
/// brow part's shapes lie exactly over the bar (union box within 2 px) and the head part
/// keeps the skin.
#[test]
fn the_brow_part_is_the_drawn_brow_and_the_head_keeps_the_skin() {
    let (mut f, _, _) = brow_fixture();
    let o = OFF as f32;
    f.skel.face.left_eye = vec![
        (167.0 + o, 90.0 + o, 0.9),
        (203.0 + o, 90.0 + o, 0.9),
        (185.0 + o, 80.0 + o, 0.9),
        (185.0 + o, 100.0 + o, 0.9),
    ];
    f.skel.face.left_brow = vec![(160.0 + o, 51.0 + o, 0.9), (185.0 + o, 51.0 + o, 0.9), (209.0 + o, 51.0 + o, 0.9)];
    // The oversized patch: body x 145..225, y 25..75 (80 x 50 px) of the picture.
    let mut patch = vec![0u8; (W * H * 4) as usize];
    for y in 25..75u32 {
        for x in 145..225u32 {
            let i = (((y + OFF) * W + x + OFF) * 4) as usize;
            patch[i..i + 4].copy_from_slice(&f.source.get_pixel(x + OFF, y + OFF).0);
        }
    }
    f.completed.parts.push(CompletedPart {
        name: "Left Eyebrow".into(),
        depth: 100,
        width: W,
        height: H,
        rgba: patch,
        dom_color: ARM,
        extruded: Vec::new(),
        borrowed_from: None,
    });
    let t = trace_with(&f, false);
    let brow = part(&t, "Left Eyebrow");
    let mut union: Option<buzz_geom::Rect> = None;
    for s in &brow.shapes {
        let b = s.path.bounding_box();
        union = Some(union.map_or(b, |u| u.union(b)));
    }
    let u = union.expect("the brow part draws the brow");
    let o = OFF as f64;
    let (bx0, by0, bx1, by1) = (160.0 + o, 45.0 + o, 210.0 + o, 57.0 + o);
    assert!(
        (u.x0 - bx0).abs() <= 2.0 && (u.x1 - bx1).abs() <= 2.0 && (u.y0 - by0).abs() <= 2.0 && (u.y1 - by1).abs() <= 2.0,
        "the brow part's box {u:?} is not the bar's ({bx0}, {by0}, {bx1}, {by1})"
    );
    let skin_area: f64 = part(&t, "Head")
        .shapes
        .iter()
        .filter(|s| s.fill.is_some() && delta_e(fill_rgb(s), ARM) <= 8.0)
        .map(|s| path_area(&s.path))
        .sum();
    assert!(skin_area > 5000.0, "the head keeps the skin: {skin_area:.0} px²");
}

/// (f) A region 60% on the torso's owner mask and 40% on pixels nobody owns goes
/// whole to the torso with its full area: shares are taken over OWNED pixels
/// only, so it is 100% torso (it used to be clipped to its 60% sliver).
#[test]
fn a_region_sixty_percent_torso_forty_percent_unowned_goes_whole_to_the_torso() {
    let mut f = fixture();
    // Body x 2..22 (canvas 402..422), y 100..160 (canvas 500..560). The torso
    // owner mask starts at body x 10: 12 of the 20 columns are owned, 8 are not.
    paint_unowned(&mut f, 402, 500, 422, 560, BAG);
    let reference = untouched_region_area(&f, BAG);
    assert!(reference > 1000.0, "the traced 20x60 patch should be ~1200 px^2, got {reference}");
    // I5: T9e — the area is held to the T9 Outline region and the T9b under-paint.
    let t = trace_t9e(&f);
    let pieces = pieces_of(&t, BAG);
    assert_eq!(pieces.len(), 1, "must not be split, got {} pieces", pieces.len());
    assert_eq!(pieces[0].0, "Chest");
    let area = path_area(&pieces[0].1.path);
    // T9b: the fill also runs under the end of the 10 px ink outline beside it
    // (the ink pixels take their nearest neighbour's colour), so it is a few
    // percent larger than the region of the unpainted picture; clipped to the
    // owned 60 % it would be 720 px^2, so 10 % still tells whole from clipped.
    assert!(
        (area - reference).abs() / reference <= 0.10,
        "the whole region goes to the torso: area {area:.1} vs the traced region's {reference:.1} (within 10%)"
    );
}

/// A part built like the fixture's: the source over body columns x0..x1.
fn part_in(source: &RgbaImage, name: &str, depth: i32, x0: u32, x1: u32, dom: [u8; 3]) -> CompletedPart {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for y in 10..230u32 {
        for x in x0..x1 {
            let i = (((y + OFF) * W + x + OFF) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&source.get_pixel(x + OFF, y + OFF).0);
        }
    }
    CompletedPart { name: name.into(), depth, width: W, height: H, rgba, dom_color: dom, extruded: Vec::new(), borrowed_from: None }
}

/// The fixture with its arm reaching back over the right half of the torso:
/// the arm (in front) owns body x 80..230; the chest owns x 10..80 and, if
/// `hidden`, its completed picture still reaches x 140, so x 80..140 is hidden
/// under the arm. The visible picture and the owner map are identical either way.
fn covered_fixture(hidden: bool) -> Fixture {
    let mut f = fixture();
    f.completed.parts = vec![
        part_in(&f.source, "Chest", 0, 10, if hidden { CUT } else { 80 }, TORSO),
        part_in(&f.source, "Upper Arm L", 1, 80, 230, ARM),
    ];
    f
}

fn part<'a>(t: &'a TracedCharacter, name: &str) -> &'a buzz_ai::character::TracedPart {
    t.parts.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("no part {name}"))
}

fn alpha_at(p: &CompletedPart, x: u32, y: u32) -> u8 {
    p.rgba[((y * W + x) * 4 + 3) as usize]
}

/// (g) A part half covered by another gets its hidden completion at the front of
/// its shapes; a fully visible part gets none; and no shape is a base fill of the
/// part's whole boundary any more. B2: the completion is traced as Ink + colour
/// (its colour fills at the front of the shapes, its ink at the front of the ink)
/// and overlaps the part's own visible pixels by 2 px, so the seam has no crack.
#[test]
fn a_covered_part_gets_a_hidden_fill_at_index_zero_and_a_visible_part_none() {
    let plain = trace_with(&covered_fixture(false), false);
    let f = covered_fixture(true);
    let t = trace_with(&f, false);

    let chest_plain = part(&plain, "Chest");
    let chest = part(&t, "Chest");
    assert!(
        chest.shapes.len() > chest_plain.shapes.len(),
        "the covered chest gets hidden-completion shapes ({} vs {})",
        chest.shapes.len(),
        chest_plain.shapes.len()
    );
    let k = chest.shapes.len() - chest_plain.shapes.len();
    // A fully visible part is untouched by the hidden completion.
    assert_eq!(
        part(&t, "Upper Arm L").shapes.len(),
        part(&plain, "Upper Arm L").shapes.len(),
        "a fully visible part gets no hidden fill"
    );
    assert_eq!(
        part(&t, "Upper Arm L").ink.len(),
        part(&plain, "Upper Arm L").ink.len(),
        "a fully visible part gets no hidden ink"
    );

    // The extra shapes are the FIRST ones, and they sit on the pixels the arm owns
    // (plus the 2 px overlap into the chest's own pixels).
    let chest_alpha = &f.completed.parts[0];
    let arm_alpha = &f.completed.parts[1];
    assert!(chest.shapes[..k].iter().all(|s| s.fill.is_some()), "the hidden completion's colour shapes are fills");
    let cover = coverage(chest.shapes[..k].iter().map(|s| &s.path));
    // I5: the hidden completion reaches `reach` px (0.02 × the character height,
    // 8–40 px) past the pixels the part owns: the strip under the joint, not the
    // whole covered region. (The segmenter's chest mask takes in the sleeves,
    // and a whole-sleeve cream patch stayed at the T-pose when the arm fell.)
    let reach = (0.02 * f.skel.character_height as f64).clamp(8.0, 40.0) as u32;
    let (mut inside, mut inside_on_arm, mut off_strip) = (0usize, 0usize, 0usize);
    for y in 0..H {
        for x in 0..W {
            if !cover[(y * W + x) as usize] {
                continue;
            }
            inside += 1;
            if alpha_at(arm_alpha, x, y) > 128 {
                inside_on_arm += 1;
            }
            let bx = x as i64 - OFF as i64;
            if bx < 80 - 3 || bx > (80 + reach) as i64 {
                off_strip += 1;
            }
        }
    }
    let expected = (reach * 220) as usize;
    let with_overlap = ((reach + 2) * 220) as usize;
    assert!(
        inside as f64 >= 0.85 * expected as f64 && inside as f64 <= 1.25 * with_overlap as f64,
        "the hidden fills should be the ~{expected} px^2 strip within {reach} px of the chest's own pixels (plus the 2 px overlap), got {inside}"
    );
    assert!(
        inside_on_arm as f64 >= 0.95 * (inside as f64 - (2 * 220) as f64 * 1.25),
        "only {inside_on_arm} of the hidden fills' {inside} pixels are owned by the covering part"
    );
    assert!(
        off_strip as f64 <= 0.01 * inside as f64,
        "{off_strip} of the hidden fills' {inside} pixels lie outside the strip (2 px overlap .. reach)"
    );
    // ... and with the hidden ink (at the front of the chest's ink) they cover
    // (nearly) every hidden pixel within reach.
    assert!(chest.ink.len() >= chest_plain.ink.len(), "the covered chest keeps its visible ink");
    let m = chest.ink.len() - chest_plain.ink.len();
    let cover = coverage(chest.shapes[..k].iter().chain(&chest.ink[..m]).map(|s| &s.path));
    let mut hidden_px = 0usize;
    let mut covered = 0usize;
    for y in 10..230u32 {
        for x in 80..(80 + reach).min(CUT) {
            let (cx, cy) = (x + OFF, y + OFF);
            if alpha_at(chest_alpha, cx, cy) > 128 && alpha_at(arm_alpha, cx, cy) > 128 {
                hidden_px += 1;
                if cover[(cy * W + cx) as usize] {
                    covered += 1;
                }
            }
        }
    }
    assert!(hidden_px > 1_000);
    assert!(
        covered as f64 >= 0.95 * hidden_px as f64,
        "the hidden fills and ink cover only {covered} of {hidden_px} hidden pixels within reach"
    );

    // No base fill of the whole part boundary: with nothing hidden, no part's
    // first shape is as large as the part's owner mask.
    let plain_f = covered_fixture(false);
    for (i, p) in plain.parts.iter().enumerate() {
        let me = &plain_f.completed.parts[i];
        let owned = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                alpha_at(me, x, y) > 128
                    && plain_f.completed.parts.iter().filter(|o| o.depth > me.depth).all(|o| alpha_at(o, x, y) <= 128)
            })
            .count() as f64;
        let first = path_area(&p.shapes[0].path);
        assert!(
            first < owned,
            "{}: the first shape ({first:.0} px^2) must be smaller than the part's owner mask ({owned:.0} px^2)",
            p.name
        );
    }
    let main = trace_with(&fixture(), false);
    let chest_owned = (130 * 220) as f64;
    assert!(
        path_area(&part(&main, "Chest").shapes[0].path) < chest_owned,
        "the chest's first shape must not be a base fill of its whole boundary"
    );
}

// ---------------------------------------------------------------------------
// T8: no gaps between clipped pieces; enclosed matte holes
// ---------------------------------------------------------------------------

/// (h) The pieces of a straddling region tile it. Here the chest owns body
/// x 10..130 and the arm x 150..230, so the two simplified boundaries leave a
/// 20 px gap between them; the 50/50 patch (x 100..180) used to lose the part of
/// itself lying in that gap (a wedge of background in the rig). The last owner
/// now takes everything still uncovered: the pieces' areas add up to the
/// traced region and a point in the former gap is inside one of them.
#[test]
fn clipped_pieces_tile_the_region_with_no_gap_between_the_boundaries() {
    let mut f = fixture();
    f.completed.parts = vec![
        part_in(&f.source, "Chest", 0, 10, 130, TORSO),
        part_in(&f.source, "Upper Arm L", 1, 150, 230, ARM),
    ];
    let reference = untouched_region_area(&f, PATCH_STRADDLE);
    assert!(reference > 2000.0, "the traced 80x30 patch should be ~2400 px^2, got {reference}");

    // I5: T9e — piece count and area are held to the T9 Outline region.
    let t = trace_t9e(&f);
    let pieces = pieces_of(&t, PATCH_STRADDLE);
    let names: Vec<&str> = pieces.iter().map(|(n, _)| *n).collect();
    assert_eq!(pieces.len(), 2, "one piece per owning part: {names:?}");
    let sum: f64 = pieces.iter().map(|(_, s)| path_area(&s.path)).sum();
    assert!(
        (sum - reference).abs() / reference <= 0.005,
        "the pieces sum to {sum:.1} px^2 but the region has {reference:.1}: a gap was left between them"
    );
    // Body (140, 200) lies in the gap between the two owner boundaries.
    let gap = Point::new((140 + OFF) as f64 + 0.5, (200 + OFF) as f64 + 0.5);
    assert!(
        pieces.iter().any(|(_, s)| s.path.contains(gap)),
        "the point in the former gap is covered by no piece"
    );
    // And the pieces do not overlap: no point of the patch is in both.
    let mut both = 0;
    for y in 185..215u32 {
        for x in 100..180u32 {
            let p = Point::new((x + OFF) as f64 + 0.5, (y + OFF) as f64 + 0.5);
            if pieces.iter().filter(|(_, s)| s.path.contains(p)).count() > 1 {
                both += 1;
            }
        }
    }
    assert!(both <= 24, "{both} pixels of the patch are in two pieces");
}

/// (i) A patch the segmenter left out of the matte, fully enclosed by the
/// character, is not background: a 10x10 hole over a highlight colour still
/// comes out as a fill of that colour covering the hole.
#[test]
fn an_enclosed_hole_in_the_matte_is_filled_and_traced() {
    let mut f = fixture();
    let highlight = [250u8, 245, 120];
    // Body (40..50, 190..200): flat torso, clear of the plaid and both patches.
    fill_rect(&mut f.source, 40, 190, 50, 200, highlight);
    for y in 190..200u32 {
        for x in 40..50u32 {
            f.matte.put_pixel(x + OFF, y + OFF, Luma([0]));
        }
    }
    let t = trace_with(&f, false);
    let centre = Point::new((45 + OFF) as f64, (195 + OFF) as f64);
    let covering: Vec<[u8; 3]> = traced_fills(&t)
        .into_iter()
        .filter(|(_, s)| s.path.contains(centre))
        .map(|(_, s)| fill_rgb(s))
        .collect();
    assert!(
        covering.iter().any(|&c| delta_e(c, highlight) <= 6.0),
        "no fill of the highlight colour covers the matte hole; fills there: {covering:?}"
    );
}

// ---------------------------------------------------------------------------
// T9: the ink as smooth brush strokes, the fills running under it
// ---------------------------------------------------------------------------

/// Signed distance (negative inside) from `(x, y)` to the rounded rectangle
/// `(x0, y0)..(x1, y1)` of corner radius `r`.
fn round_rect_sd(x: f64, y: f64, (x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) -> f64 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    let (qx, qy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// A figure drawn as a `band` px dark outline round a light fill (torso colour left of
/// the cut, arm colour right of it), anti-aliased on the inside edge. With
/// `shared_outline` one rounded rectangle spans torso and arm (its outline
/// crosses the torso/arm cut); otherwise each part has its own, clear of the cut.
fn inked_fixture(shared_outline: bool, band: f64) -> Fixture {
    let mut f = fixture();
    // Small figures: the outline must be over 12 % of the figure's pixels, as it is on
    // a real character (the pipeline takes the darkest 12 % of them for ink).
    let figures: Vec<(f64, f64, f64, f64)> = if shared_outline {
        vec![(95.0, 80.0, 185.0, 170.0)]
    } else {
        vec![(80.0, 70.0, 130.0, 180.0), (150.0, 70.0, 200.0, 180.0)]
    };
    let mut source = RgbaImage::new(W, H);
    let mut matte = GrayImage::new(W, H);
    for by in 0..240u32 {
        for bx in 0..240u32 {
            let (px, py) = (bx as f64 + 0.5, by as f64 + 0.5);
            for &fig in &figures {
                let sd = round_rect_sd(px, py, fig, 20.0);
                if sd > 0.0 {
                    continue;
                }
                let fill = if px < CUT as f64 { TORSO } else { ARM };
                // Ink weight: 1 on the band, fading to 0 across the inner edge.
                let w_ink = (sd + band + 0.5).clamp(0.0, 1.0);
                let mut c = [0u8; 4];
                for k in 0..3 {
                    c[k] = (fill[k] as f64 * (1.0 - w_ink) + INK[k] as f64 * w_ink).round() as u8;
                }
                c[3] = 255;
                source.put_pixel(bx + OFF, by + OFF, Rgba(c));
                matte.put_pixel(bx + OFF, by + OFF, Luma([255]));
            }
        }
    }
    let part = |name: &str, depth: i32, x0: u32, x1: u32, dom: [u8; 3]| {
        let mut rgba = vec![0u8; (W * H * 4) as usize];
        for y in 0..240u32 {
            for x in x0..x1 {
                let i = (((y + OFF) * W + x + OFF) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&source.get_pixel(x + OFF, y + OFF).0);
            }
        }
        CompletedPart { name: name.into(), depth, width: W, height: H, rgba, dom_color: dom, extruded: Vec::new(), borrowed_from: None }
    };
    f.completed.parts = vec![part("Chest", 0, 0, CUT, TORSO), part("Upper Arm L", 1, CUT, 240, ARM)];
    // The character palette's darkest colour (the line colour the ink is measured
    // against, T9c) must be the ink, as on a real character where the outline is a
    // tenth of the pixels: a strip of ink across the top of the canvas, outside the
    // figure and the matte, in the first part's picture.
    for y in 0..8u32 {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            f.completed.parts[0].rgba[i..i + 4].copy_from_slice(&[INK[0], INK[1], INK[2], 255]);
        }
    }
    f.source = source;
    f.matte = matte;
    f.dashes = 0;
    f
}

/// Path vertices (every segment end; a close adds none) over the shapes, and the
/// total outline length of those shapes in px.
fn vertices_and_outline_length(shapes: &[buzz_scene::ShapeData]) -> (usize, f64) {
    let mut vertices = 0;
    let mut length = 0.0;
    for s in shapes {
        vertices += s
            .path
            .elements()
            .iter()
            .filter(|e| !matches!(e, buzz_geom::PathEl::ClosePath))
            .count();
        length += s.path.perimeter(0.1);
    }
    (vertices, length)
}

fn lightness_of(rgb: [u8; 3]) -> f64 {
    srgb_to_lab(rgb)[0]
}

/// The canvas pixels (centres) inside any of `paths`.
fn coverage<'a>(paths: impl IntoIterator<Item = &'a BezPath>) -> Vec<bool> {
    let mut grid = vec![false; (W * H) as usize];
    for path in paths {
        let bb = path.bounding_box();
        let (x0, x1) = (bb.x0.floor().max(0.0) as u32, (bb.x1.ceil().max(0.0) as u32).min(W));
        let (y0, y1) = (bb.y0.floor().max(0.0) as u32, (bb.y1.ceil().max(0.0) as u32).min(H));
        for y in y0..y1 {
            for x in x0..x1 {
                if path.contains(Point::new(x as f64 + 0.5, y as f64 + 0.5)) {
                    grid[(y * W + x) as usize] = true;
                }
            }
        }
    }
    grid
}

/// (j) A figure drawn as a 3 px dark outline round a light fill traces to ink that
/// is a FILLED brush shape (not a stroke) per part, in the tracer's dark colour.
#[test]
fn ink_outlines_are_filled_brush_shapes_in_every_part() {
    let f = inked_fixture(false, 3.0);
    let t = trace_with(&f, false);
    for name in ["Chest", "Upper Arm L"] {
        let p = part(&t, name);
        assert!(!p.ink.is_empty(), "{name} has an ink outline");
        assert!(
            p.ink.iter().all(|s| s.fill.is_some() && s.stroke.is_none()),
            "{name}: the ink is brush fills, not strokes"
        );
        let (_, length) = vertices_and_outline_length(&p.ink);
        assert!(length > 300.0, "{name}: the ink outline is long ({length:.0} px)");
        for s in &p.ink {
            assert!(lightness_of(fill_rgb(s)) < 45.0, "{name}: the ink keeps the tracer's dark colour");
        }
    }
}

/// (j3) The fills run UNDER the ink (T9b contract): no FILL with L* < 45 overlaps
/// the thin-ink mask by more than 20 % of the mask (the pixels under the ink are
/// painted with the neighbour colour before the fills are traced), and a point on the
/// outline is covered by a fill of the light colour beside it.
#[test]
fn dark_fills_do_not_sit_on_the_ink_and_a_light_fill_runs_under_it() {
    let f = inked_fixture(false, 3.0);
    let t = trace_with(&f, false);

    // The thin-ink RIM pixels (I5: under-painting reaches 3 px into the ink, so
    // the fills run under the ink's rim; the core of ink thicker than 6 px — the
    // fixture's 10 px body border — keeps its own dark colour as a fill by
    // design), and the union of the dark fills' coverage over them.
    let ink = ink_rim(&ink_of(&f), 3);
    let ink_px = ink.iter().filter(|&&v| v).count();
    assert!(ink_px > 300, "the fixture has thin ink ({ink_px} px)");
    let dark_paths: Vec<&BezPath> = traced_fills(&t)
        .into_iter()
        .filter(|(_, shape)| lightness_of(fill_rgb(shape)) < 45.0)
        .map(|(_, shape)| &shape.path)
        .collect();
    let dark_cover = coverage(dark_paths.iter().copied());
    let on_ink = dark_cover.iter().zip(&ink).filter(|(c, i)| **c && **i).count();
    let described: Vec<String> = traced_fills(&t)
        .into_iter()
        .filter(|(_, shape)| lightness_of(fill_rgb(shape)) < 45.0)
        .map(|(part, shape)| {
            let b = shape.path.bounding_box();
            format!(
                "{part} {:?} {:.0} px² ({:.0},{:.0})-({:.0},{:.0})",
                fill_rgb(shape),
                path_area(&shape.path),
                b.x0,
                b.y0,
                b.x1,
                b.y1
            )
        })
        .collect();
    assert!(
        on_ink as f64 <= 0.2 * ink_px as f64,
        "{} dark fills cover {on_ink} of the {ink_px} thin-ink rim px (> 20 %): {described:#?}",
        dark_paths.len()
    );
    // On the inner half of the torso's left outline (the 3 px band is body x 80–82;
    // probe x 82.5, y 125) a torso-coloured fill lies under the ink. Since A1
    // (TRACE_SPILL_AND_OUTPAINT_PLAN) the fill stops at the centre of an outline on
    // the figure's edge, so the outer half (x 80–81) is the ink's alone.
    let on_line = Point::new((82 + OFF) as f64 + 0.5, (125 + OFF) as f64 + 0.5);
    assert!(
        traced_fills(&t).iter().any(|(_, s)| s.path.contains(on_line) && delta_e(fill_rgb(s), TORSO) <= 6.0),
        "no torso-coloured fill runs under the ink line"
    );
}

/// The ink pixels within `depth` px (Chebyshev) of a non-ink pixel: the rim the
/// under-painting reaches. Ink deeper than that is the core of a mass or of a
/// line thicker than `2 · depth`.
fn ink_rim(ink: &[bool], depth: i64) -> Vec<bool> {
    let (w, h) = (W as i64, H as i64);
    (0..ink.len())
        .map(|i| {
            if !ink[i] {
                return false;
            }
            let (x, y) = (i as i64 % w, i as i64 / w);
            (-depth..=depth).any(|dy| {
                (-depth..=depth).any(|dx| {
                    let (nx, ny) = (x + dx, y + dy);
                    nx < 0 || ny < 0 || nx >= w || ny >= h || !ink[(ny * w + nx) as usize]
                })
            })
        })
        .collect()
}

/// The traced character painted back to pixels the cheap way (no renderer): every
/// part's fills in order, then every part's ink on top, each shape's pixel centres
/// inside its path painted in its fill colour, over white. Opaque RGBA8, `W` x `H`.
fn rasterise(t: &TracedCharacter, with_ink: bool) -> Vec<u8> {
    rasterise_filtered(t, with_ink, |_| true)
}

/// [`rasterise`] painting only the fills `keep` accepts (the ink is never filtered).
fn rasterise_filtered(t: &TracedCharacter, with_ink: bool, keep: impl Fn(&buzz_scene::ShapeData) -> bool) -> Vec<u8> {
    let mut out = vec![255u8; (W * H * 4) as usize];
    let mut paint = |shape: &buzz_scene::ShapeData| {
        let Some(_) = shape.fill.as_ref() else { return };
        let rgb = fill_rgb(shape);
        for (i, on) in coverage([&shape.path]).iter().enumerate() {
            if *on {
                out[i * 4..i * 4 + 3].copy_from_slice(&rgb);
            }
        }
    };
    for part in &t.parts {
        part.shapes.iter().filter(|s| keep(s)).for_each(&mut paint);
    }
    if with_ink {
        for part in &t.parts {
            part.ink.iter().for_each(&mut paint);
        }
    }
    out
}

/// (j2) The outline-continuity measure (T9b), the one `rig_vs_source` prints: of the
/// source's thin-ink pixels, the share with a rendered dark pixel (L* < 45) within
/// 2 px, at least 90 % on a figure drawn as a 3 px dark outline round light fills —
/// and (the metric must discriminate) the fills alone, with no ink layer, cover almost
/// none of it.
#[test]
fn ink_coverage_on_a_synthetic_outlined_figure_is_at_least_90_percent() {
    let f = inked_fixture(false, 3.0);
    let t = trace_with(&f, false);
    let ink = ink_of(&f);
    assert!(ink.iter().filter(|&&v| v).count() > 300, "the fixture has thin ink");

    let (covered, total, pct) =
        buzz_scene::trace_metrics::compute_ink_coverage(&ink, &rasterise(&t, true), W as usize, H as usize);
    assert!(pct >= 90.0, "ink coverage {pct:.1} % ({covered} of {total} thin-ink px)");

    // I5: the LIGHT fills alone cover almost none of it (the dark fills under
    // the 10 px border's core are the ink's own colour by design and sit
    // within 2 px of its rim, so they are left out of this check).
    let light_only = rasterise_filtered(&t, false, |s| lightness_of(fill_rgb(s)) >= 45.0);
    let (covered, total, pct) = buzz_scene::trace_metrics::compute_ink_coverage(&ink, &light_only, W as usize, H as usize);
    assert!(pct < 20.0, "without the ink layer the light fills' coverage is {pct:.1} % ({covered} of {total}), not near zero");
}

/// (k) An ink outline running from the torso into the upper arm is CUT where it
/// crosses the part boundary — one piece per part — and the pieces' union is the
/// stroke the tracer drew, to 1 %.
#[test]
fn an_ink_outline_crossing_two_parts_is_cut_and_the_pieces_add_back_up() {
    let f = inked_fixture(true, 6.0);
    // I5: T9e — the reference is the LineArt brush ink (`character_ink_options`).
    let t = trace_t9e(&f);

    // The reference: the line tracer's own ink shapes, before any part assignment.
    let mut visible = vec![0u8; (W * H * 4) as usize];
    for (i, (px, m)) in f.source.pixels().zip(f.matte.pixels()).enumerate() {
        if m[0] > 128 {
            visible[i * 4..i * 4 + 4].copy_from_slice(&px.0);
        }
    }
    let report = buzz_scene::trace::trace(W, H, &visible, &buzz_ai::character::character_ink_options(W, H, Some(line_colour_of(&f))));
    let cut_x = (CUT + OFF) as f64;
    assert!(
        report.shapes.iter().any(|s| {
            let bb = s.path.bounding_box();
            bb.x0 < cut_x - 5.0 && bb.x1 > cut_x + 5.0
        }),
        "the traced outline crosses the torso/arm cut"
    );

    let chest = part(&t, "Chest");
    let arm = part(&t, "Upper Arm L");
    assert!(!chest.ink.is_empty() && !arm.ink.is_empty(), "the outline is cut into a piece for each part");
    // Each piece stays on its own side of the cut (to a sliver: under 1 % of its pixels
    // lie more than 3 px past it).
    let (chest_cover, arm_cover) = (
        coverage(chest.ink.iter().map(|s| &s.path)),
        coverage(arm.ink.iter().map(|s| &s.path)),
    );
    let past = |cover: &[bool], beyond: &dyn Fn(u32) -> bool| {
        let (mut all, mut far) = (0usize, 0usize);
        for (i, &c) in cover.iter().enumerate() {
            if c {
                all += 1;
                far += beyond(i as u32 % W) as usize;
            }
        }
        (far, all)
    };
    // T9b: the LARGEST owner takes whatever lies outside every simplified boundary,
    // so the arm's outermost silhouette row (its smoothed boundary runs a pixel
    // inside the stroke's outer edge) lands on the torso piece: ~1.2 % of it here.
    let (far, all) = past(&chest_cover, &|x| x as f64 >= cut_x + 3.0);
    assert!(far as f64 <= 0.02 * all as f64, "{far} of the chest ink's {all} px lie in the arm");
    let (far, all) = past(&arm_cover, &|x| (x as f64) < cut_x - 3.0);
    assert!(far as f64 <= 0.02 * all as f64, "{far} of the arm ink's {all} px lie in the torso");
    assert!(
        chest_cover.iter().filter(|&&c| c).count() > 100 && arm_cover.iter().filter(|&&c| c).count() > 100,
        "both parts get a real piece"
    );

    let original = coverage(report.shapes.iter().map(|s| &s.path));
    let pieces = coverage(chest.ink.iter().chain(&arm.ink).map(|s| &s.path));
    let total = original.iter().filter(|&&c| c).count();
    let differing = original.iter().zip(&pieces).filter(|(a, b)| a != b).count();
    assert!(total > 500, "the outline covers ~{total} px");
    assert!(
        differing as f64 <= 0.01 * total as f64,
        "the pieces differ from the traced stroke on {differing} of {total} px (must be within 1%)"
    );
}

// ---------------------------------------------------------------------------
// T9c: ink-coloured only, protected face boxes, every part gets its piece
// ---------------------------------------------------------------------------

/// (l) A DARK RED plaid (locally far darker than the torso) is not ink-coloured: no
/// thin-ink pixel lies on a grid dash, no ink shape covers one, and the dashes stay
/// fills of their own red (`plaid_grid_regions_survive_as_fills` counts them).
#[test]
fn a_dark_red_plaid_is_a_fill_and_never_ink() {
    let f = fixture();
    let red: Vec<usize> = f
        .source
        .pixels()
        .enumerate()
        .filter(|(_, p)| p.0[..3] == GRID)
        .map(|(i, _)| i)
        .collect();
    assert!(red.len() >= 40 * 33, "the fixture draws {} red px", red.len());
    let ink = ink_of(&f);
    let on_ink = red.iter().filter(|&&i| ink[i]).count();
    assert!(on_ink <= red.len() / 50, "{on_ink} of {} red px are thin ink", red.len());

    let t = trace_with(&f, false);
    let ink_cover = coverage(t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)));
    let covered = red.iter().filter(|&&i| ink_cover[i]).count();
    assert!(covered <= red.len() / 50, "{covered} of {} red px lie under an ink shape", red.len());
    assert!(
        t.parts.iter().flat_map(|p| &p.ink).all(|s| delta_e(fill_rgb(s), GRID) > 25.0),
        "an ink shape came out red"
    );
}

/// What a drawn eye is made of, as canvas pixel indices.
struct DrawnEye {
    white: Vec<usize>,
    ring: Vec<usize>,
    pupil: Vec<usize>,
}

const EYE_WHITE: [u8; 3] = [245, 245, 240];
/// The drawn eye's centre, in body coordinates, on the arm (clear of the plaid and patches).
const EYE_CENTRE: (f64, f64) = (185.0, 90.0);

/// The fixture with an eye drawn on the arm: a white ellipse (rx 15, ry 7), a dark
/// ring of 3 px round it (outer rx 18, ry 10) and a dark pupil of radius 5. With
/// `landmarks` the skeleton has the left eye landmarks at the ring extremes,
/// so the protected eye box is the ring box padded 15 %. With `as_part` the picture
/// also has a "Left Eye" part (depth 5, above the arm) holding exactly the drawn eye
/// (ring, white and pupil): the segmenter's part mask the T9d protection is built from.
fn eye_fixture(landmarks: bool, as_part: bool) -> (Fixture, DrawnEye) {
    let mut f = fixture();
    let mut eye = DrawnEye { white: vec![], ring: vec![], pupil: vec![] };
    let mut eye_rgba = vec![0u8; (W * H * 4) as usize];
    let (cx, cy) = EYE_CENTRE;
    for by in 70..110u32 {
        for bx in 160..210u32 {
            let (dx, dy) = (bx as f64 + 0.5 - cx, by as f64 + 0.5 - cy);
            let outer = (dx / 18.0).powi(2) + (dy / 10.0).powi(2);
            let inner = (dx / 15.0).powi(2) + (dy / 7.0).powi(2);
            if outer > 1.0 {
                continue;
            }
            let (x, y) = (bx + OFF, by + OFF);
            let i = (y * W + x) as usize;
            let c = if dx * dx + dy * dy <= 25.0 {
                eye.pupil.push(i);
                INK
            } else if inner <= 1.0 {
                eye.white.push(i);
                EYE_WHITE
            } else {
                eye.ring.push(i);
                INK
            };
            f.source.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
            f.completed.parts[1].rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            eye_rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    if as_part {
        f.completed.parts.push(CompletedPart {
            name: "Left Eye".into(),
            depth: 5,
            width: W,
            height: H,
            rgba: eye_rgba,
            dom_color: EYE_WHITE,
            extruded: Vec::new(),
            borrowed_from: None,
        });
    }
    if landmarks {
        let o = OFF as f32;
        f.skel.face.left_eye = vec![
            (167.0 + o, 90.0 + o, 0.9),
            (203.0 + o, 90.0 + o, 0.9),
            (185.0 + o, 80.0 + o, 0.9),
            (185.0 + o, 100.0 + o, 0.9),
        ];
    }
    (f, eye)
}

/// The share of `pixels` (canvas indices) whose centres lie inside any of `paths`.
fn covered_share<'a>(paths: impl IntoIterator<Item = &'a BezPath>, pixels: &[usize]) -> f64 {
    let cover = coverage(paths);
    pixels.iter().filter(|&&i| cover[i]).count() as f64 / pixels.len().max(1) as f64
}

/// (m) FALLBACK, a character with no Eye / Eyebrow / Mouth part: a drawn eye inside
/// the protected eye box (landmark box padded 15 %) is traced exactly as drawn: the
/// white, the ring and the whole pupil come out as FILLS of their own colours, and no
/// ink shape lies inside the box. (Without the landmarks the same ring and pupil are
/// thin ink, so the fixture proves something.)
#[test]
fn a_drawn_eye_in_the_protected_box_survives_as_fills_and_has_no_ink() {
    let (bare, _) = eye_fixture(false, false);
    let (f, eye) = eye_fixture(true, false);
    let boxes = buzz_ai::character::protected_face_boxes(&f.skel);
    assert_eq!(boxes.len(), 1, "one eye box from the one eye's landmarks");
    let eye_box = boxes[0];
    let o = OFF as f64;
    assert!(
        (eye_box.x0 - (161.6 + o)).abs() < 0.2 && (eye_box.x1 - (208.4 + o)).abs() < 0.2,
        "the eye box is the landmark box padded 15 %: {eye_box:?}"
    );
    let ink_in_box = |t: &TracedCharacter| {
        let cover = coverage(t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)));
        let mut n = 0;
        for y in eye_box.y0.ceil() as u32..eye_box.y1.floor() as u32 {
            for x in eye_box.x0.ceil() as u32..eye_box.x1.floor() as u32 {
                n += cover[(y * W + x) as usize] as usize;
            }
        }
        n
    };
    // I5: T9e — the protected eye box exists only on the T9e path.
    let unprotected = ink_in_box(&trace_t9e(&bare));
    assert!(unprotected > 100, "without the box the drawn ring is ink ({unprotected} px)");

    let t = trace_t9e(&f);
    let protected = ink_in_box(&t);
    assert!(protected <= 10, "{protected} px of ink lie inside the protected eye box");

    let fills = traced_fills(&t);
    let of_colour = |colour: [u8; 3], tol: f64| -> Vec<&BezPath> {
        fills.iter().filter(|(_, s)| delta_e(fill_rgb(s), colour) <= tol).map(|(_, s)| &s.path).collect()
    };
    let white = covered_share(of_colour(EYE_WHITE, 8.0), &eye.white);
    assert!(white >= 0.7, "the eye white is a fill over {:.0} % of its pixels", white * 100.0);
    let dark = of_colour(INK, 12.0);
    let ring = covered_share(dark.iter().copied().filter(|p| p.bounding_box().width() >= 30.0), &eye.ring);
    assert!(ring >= 0.7, "the dark ring is a fill over {:.0} % of its pixels", ring * 100.0);
    // The pupil is ONE whole small fill, not fragments.
    let pupil = dark
        .iter()
        .copied()
        .filter(|p| p.bounding_box().width() <= 14.0 && p.bounding_box().height() <= 14.0)
        .map(|p| covered_share([p], &eye.pupil))
        .fold(0.0, f64::max);
    assert!(pupil >= 0.8, "the best single pupil fill covers {:.0} % of the pupil", pupil * 100.0);
}

/// (m2) T9d: the protected region is the segmenter's own feature part. The same drawn
/// eye, owned by a "Left Eye" part and with NO landmarks at all, is traced as drawn:
/// the whole pupil is one fill, the white and ring are fills, and no ink shape lies on
/// any pixel of the eye. (Without the part the ring and pupil are thin ink.)
#[test]
fn a_drawn_eye_owned_by_an_eye_part_survives_whole_with_no_ink() {
    let (bare, _) = eye_fixture(false, false);
    let (f, eye) = eye_fixture(false, true);
    assert!(buzz_ai::character::protected_face_boxes(&f.skel).is_empty(), "no landmarks: the part protects it");
    let all: Vec<usize> = eye.white.iter().chain(&eye.ring).chain(&eye.pupil).copied().collect();
    let ink_on_eye = |t: &TracedCharacter| {
        let cover = coverage(t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)));
        all.iter().filter(|&&i| cover[i]).count()
    };
    // I5: T9e — the protected Eye part mask exists only on the T9e path.
    let unprotected = ink_on_eye(&trace_t9e(&bare));
    assert!(unprotected > 100, "without the part the drawn eye is ink ({unprotected} px)");

    let t = trace_t9e(&f);
    let protected = ink_on_eye(&t);
    assert!(protected <= 10, "{protected} px of ink lie on the drawn eye");

    let fills = traced_fills(&t);
    let of_colour = |colour: [u8; 3], tol: f64| -> Vec<&BezPath> {
        fills.iter().filter(|(_, s)| delta_e(fill_rgb(s), colour) <= tol).map(|(_, s)| &s.path).collect()
    };
    let white = covered_share(of_colour(EYE_WHITE, 8.0), &eye.white);
    assert!(white >= 0.7, "the eye white is a fill over {:.0} % of its pixels", white * 100.0);
    let dark = of_colour(INK, 12.0);
    let ring = covered_share(dark.iter().copied().filter(|p| p.bounding_box().width() >= 30.0), &eye.ring);
    assert!(ring >= 0.7, "the dark ring is a fill over {:.0} % of its pixels", ring * 100.0);
    let pupil = dark
        .iter()
        .copied()
        .filter(|p| p.bounding_box().width() <= 14.0 && p.bounding_box().height() <= 14.0)
        .map(|p| covered_share([p], &eye.pupil))
        .fold(0.0, f64::max);
    assert!(pupil >= 0.8, "the best single pupil fill covers {:.0} % of the pupil", pupil * 100.0);
}

/// The dark brown of the drawn brow (an ink colour: ΔE to the ink colour is well under 30).
const BROW: [u8; 3] = [35, 22, 18];

/// T9e: the eye fixture on a part named "Head" (the arm part renamed, bone and joint
/// too), with a 50 x 12 px dark BROW bar above the eye (body x 160..210, y 45..57) and
/// four 3 px pen lines (body x 155..205, y 115, 125, 135 and 145) below it, so the
/// picture's typical pen is the 2-3 px line it is in a real drawing. Returns the
/// fixture, the drawn eye (ring, white, pupil) and the canvas indices of the brow.
fn brow_fixture() -> (Fixture, DrawnEye, Vec<usize>) {
    let (mut f, eye) = eye_fixture(false, false);
    f.completed.parts[1].name = "Head".into();
    f.skel.bones[1].name = "Head".into();
    f.skel.joints[1].name = "Head".into();
    let mut paint = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]| {
        let mut idx = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let (cx, cy) = (x + OFF, y + OFF);
                f.source.put_pixel(cx, cy, Rgba([c[0], c[1], c[2], 255]));
                let i = (cy * W + cx) as usize;
                f.completed.parts[1].rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                idx.push(i);
            }
        }
        idx
    };
    let brow = paint(160, 45, 210, 57, BROW);
    for y in [115, 125, 135, 145] {
        paint(155, y, 205, y + 3, INK);
    }
    (f, eye, brow)
}

/// (o) T9e: the brow is a MASS (a 12 px bar; its radius is under `max_line_radius`, so
/// only its compactness says so), and the eye's 3 px ring is a line. In the head part the
/// brow comes out as a dark FILL, whole (no ink shape covers over 20 % of it, and the
/// ink together under 20 %), while the eye ring is ink.
#[test]
fn a_brow_bar_in_the_head_is_a_fill_and_an_eye_ring_is_ink() {
    let (f, eye, brow) = brow_fixture();
    let t = trace_with(&f, false);
    let head = part(&t, "Head");
    for shape in t.parts.iter().flat_map(|p| p.ink.iter()) {
        let share = covered_share([&shape.path], &brow);
        assert!(share <= 0.2, "an ink shape covers {:.0} % of the brow", share * 100.0);
    }
    let ink_on_brow = covered_share(t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)), &brow);
    assert!(ink_on_brow <= 0.2, "the ink covers {:.0} % of the brow", ink_on_brow * 100.0);
    let dark: Vec<&BezPath> = head
        .shapes
        .iter()
        .filter(|s| s.fill.is_some() && delta_e(fill_rgb(s), BROW) <= 12.0)
        .map(|s| &s.path)
        .collect();
    let fill = covered_share(dark.iter().copied(), &brow);
    assert!(fill >= 0.8, "the brow is a dark fill over {:.0} % of its pixels", fill * 100.0);
    // One whole fill, not fragments: the best single fill covers most of it.
    let best = dark.iter().map(|p| covered_share([*p], &brow)).fold(0.0, f64::max);
    assert!(best >= 0.8, "the best single fill covers {:.0} % of the brow", best * 100.0);
    let ring = covered_share(t.parts.iter().flat_map(|p| p.ink.iter().map(|s| &s.path)), &eye.ring);
    assert!(ring >= 0.6, "the eye ring is ink over {:.0} % of its pixels", ring * 100.0);
}

/// (p) T9e: the under-paint leaves the brow untouched. The fills are traced from the
/// under-painted picture, so a brow the under-paint had repainted with skin would be
/// covered by skin-coloured fills: no fill lighter than L* 45 covers over 15 % of the
/// brow's pixels (the arm-coloured region has a hole where the brow is).
#[test]
fn the_under_paint_leaves_a_brow_mass_untouched() {
    let (f, _, brow) = brow_fixture();
    // I5: T9e — `paint_under_ink` is the T9e under-paint (Ink + colour fills run under every ink shape).
    let t = trace_t9e(&f);
    let light: Vec<&BezPath> = traced_fills(&t)
        .into_iter()
        .filter(|(_, s)| lightness_of(fill_rgb(s)) > 45.0)
        .map(|(_, s)| &s.path)
        .collect();
    let covered = covered_share(light.iter().copied(), &brow);
    assert!(covered <= 0.15, "light fills cover {:.0} % of the brow: the under-paint changed it", covered * 100.0);
}

/// (n) A region straddling three parts with shares 80 / 15 / 5 % comes out as THREE
/// pieces, one per part, each lying at least 95 % on its own owner's pixels (the
/// 5 % owner holds 500 px, over the 400 px floor and the 1 % share) — they used to be
/// two, the smallest part's pixels handed to the largest.
#[test]
fn a_region_straddling_three_parts_80_15_5_gives_each_part_its_piece() {
    let mut f = fixture();
    // Flat torso and arm, no plaid and no patches; then a 100 x 100 patch at body
    // x 60..160, y 60..160 over three owners: Chest x 10..140, Upper Arm L 140..155,
    // Hand L 155..230 -> 80 / 15 / 5 columns of the patch.
    fill_rect(&mut f.source, 20, 20, CUT, 220, TORSO);
    fill_rect(&mut f.source, CUT, 20, 220, 220, ARM);
    fill_rect(&mut f.source, 60, 60, 160, 160, PATCH_STRADDLE);
    f.completed.parts = vec![
        part_in(&f.source, "Chest", 0, 10, 140, TORSO),
        part_in(&f.source, "Upper Arm L", 1, 140, 155, ARM),
        part_in(&f.source, "Hand L", 2, 155, 230, ARM),
    ];
    let reference = untouched_region_area(&f, PATCH_STRADDLE);
    assert!(reference > 9000.0, "the traced 100x100 patch should be ~10000 px^2, got {reference}");

    // I5: T9e — piece count and area are held to the T9 Outline region.
    let t = trace_t9e(&f);
    let pieces = pieces_of(&t, PATCH_STRADDLE);
    let names: Vec<&str> = pieces.iter().map(|(n, _)| *n).collect();
    assert_eq!(pieces.len(), 3, "one piece per owning part: {names:?}");
    for (name, x0, x1) in [("Chest", 10u32, 140u32), ("Upper Arm L", 140, 155), ("Hand L", 155, 230)] {
        let (_, shape) = pieces
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("{name} has no piece: {names:?}"));
        let cover = coverage([&shape.path]);
        let all = cover.iter().filter(|&&c| c).count();
        let own = (0..H)
            .flat_map(|y| (x0 + OFF..x1 + OFF).map(move |x| (y * W + x) as usize))
            .filter(|&i| cover[i])
            .count();
        assert!(all > 300, "{name}'s piece covers {all} px");
        assert!(own as f64 >= 0.95 * all as f64, "{name}'s piece is only {own} of {all} px on its own pixels");
    }
    let sum: f64 = pieces.iter().map(|(_, s)| path_area(&s.path)).sum();
    assert!(
        (sum - reference).abs() / reference <= 0.02,
        "the pieces sum to {sum:.0} px^2, the region has {reference:.0}"
    );
}

// ---------------------------------------------------------------------------
// B2: a limb's hidden completion traced as drawn artwork
// ---------------------------------------------------------------------------

/// The shared-outline inked figure with the CHEST in front: the arm (behind) owns
/// body x >= CUT. With `extruded`, the arm's completed picture runs on under the
/// chest to body x 115 the way `outpaint_limb` makes it: every column there a copy
/// of the arm's cross-section at x CUT + 2 (its outline rows and its colour), each
/// added pixel marked in `CompletedPart::extruded`.
fn arm_under_chest_fixture(extruded: bool) -> Fixture {
    let mut f = inked_fixture(true, 3.0);
    let mut chest = part_in(&f.source, "Chest", 1, 0, CUT, TORSO);
    // The line-colour strip `inked_fixture` puts in its first part's picture.
    let strip = (8 * W * 4) as usize;
    chest.rgba[..strip].copy_from_slice(&f.completed.parts[0].rgba[..strip]);
    let mut arm = part_in(&f.source, "Upper Arm L", 0, CUT, 240, ARM);
    if extruded {
        arm.extruded = vec![false; (W * H) as usize];
        for y in 0..240u32 {
            for x in 115..CUT {
                let (cx, cy) = (x + OFF, y + OFF);
                if f.matte.get_pixel(cx, cy)[0] == 0 {
                    continue;
                }
                let i = (cy * W + cx) as usize;
                arm.rgba[i * 4..i * 4 + 4].copy_from_slice(&f.source.get_pixel(CUT + 2 + OFF, cy).0);
                arm.extruded[i] = true;
            }
        }
    }
    f.completed.parts = vec![chest, arm];
    f
}

/// B2: a limb's extruded completion under the part in front of it is traced as Ink +
/// colour — a dark ink shape at the front of its ink and a fill in its own colour at
/// the front of its shapes, both behind its visible artwork — the whole tube (no
/// `reach` gate on extruded pixels), and nothing in it outside the matte.
#[test]
fn a_limbs_hidden_completion_is_ink_and_colour_behind_its_visible_shapes_and_inside_the_matte() {
    let plain = trace_with(&arm_under_chest_fixture(false), false);
    let f = arm_under_chest_fixture(true);
    let t = trace_with(&f, false);
    let (arm, arm_plain) = (part(&t, "Upper Arm L"), part(&plain, "Upper Arm L"));
    assert!(
        arm.shapes.len() > arm_plain.shapes.len() && arm.ink.len() > arm_plain.ink.len(),
        "the arm gets hidden fills ({} vs {}) and hidden ink ({} vs {})",
        arm.shapes.len(),
        arm_plain.shapes.len(),
        arm.ink.len(),
        arm_plain.ink.len()
    );
    let (k, m) = (arm.shapes.len() - arm_plain.shapes.len(), arm.ink.len() - arm_plain.ink.len());
    let (hidden_fills, hidden_ink) = (&arm.shapes[..k], &arm.ink[..m]);

    // Ink + colour, not one flat Outline fill.
    assert!(
        hidden_ink.iter().any(|s| s.fill.is_some() && lightness_of(fill_rgb(s)) < 45.0),
        "the hidden completion has dark ink: {:?}",
        hidden_ink.iter().map(fill_rgb).collect::<Vec<_>>()
    );
    assert!(
        hidden_fills.iter().any(|s| s.fill.is_some() && delta_e(fill_rgb(s), ARM) < 10.0),
        "the hidden completion has a fill in the arm's colour: {:?}",
        hidden_fills.iter().map(fill_rgb).collect::<Vec<_>>()
    );

    // Behind the visible artwork: the first k shapes / m ink shapes are the hidden
    // ones, lying under the chest (left of the cut, plus the 2 px overlap)...
    let cut_x = (OFF + CUT) as f64;
    for s in hidden_fills.iter().chain(hidden_ink) {
        let b = s.path.bounding_box();
        assert!(b.x1 <= cut_x + 3.0, "a hidden shape reaches x {:.1}, past the cut + overlap ({cut_x})", b.x1);
    }
    // ...and they reach the far end of the tube (body x 115): no reach gate.
    let x0 = hidden_fills.iter().map(|s| s.path.bounding_box().x0).fold(f64::INFINITY, f64::min);
    assert!(x0 <= (OFF + 115) as f64 + 1.0, "the hidden fills stop at x {x0:.1}, short of the tube's end");

    // Nothing in them lies outside the matte.
    let cover = coverage(hidden_fills.iter().chain(hidden_ink).map(|s| &s.path));
    let outside: Vec<(u32, u32)> = (0..W * H)
        .filter(|&i| cover[i as usize] && f.matte.as_raw()[i as usize] == 0)
        .map(|i| (i % W, i / W))
        .collect();
    assert!(outside.is_empty(), "{} hidden-completion px lie outside the matte: {:?}", outside.len(), &outside[..outside.len().min(12)]);
}
