// Three Roads --- a fifteen-second film, built by script and nothing else.
//
// Run it:
//
//   buzzanimate --script films/three-roads/three-roads.js \
//               --audio "<a take>.mp3" --from 30 --for 5 \
//               --audio "<a take>.mp3" --from 35 --for 5 \
//               --audio "<a take>.mp3" --from 40 --for 5 \
//               --save films/three-roads/three-roads.buzz \
//               --render films/three-roads/three-roads.mp4 --height 1080
//
// Nobody opens a window and nobody clicks anything. What comes out is an
// ordinary .buzz document -- layers, symbols, keyframes, poses -- that can be
// opened and drawn over, and an mp4 of it.
//
// The three shots:
//
//   1. A forest at first light. Everyone walks in. Meera speaks.
//   2. A village at noon. Written as prose and handed to the director, which
//      stages it, casts it, blocks it and frames it -- and now tells us who it
//      cast, so we can put the take in their mouths.
//   3. A city at night, under the lamps. The moon is recalled from the asset
//      library, where shot 1 filed it.
//
// Both rigs are used on every character, because they are for different jobs:
// bones for the limbs (a walk is a chain of angles), layer parenting for the
// face (a blink is not a joint, and a mouth shape is a drawing swap).

var doc = fl.getDocumentDOM();

var W = 1920, H = 1080, FPS = 24;
var SHOT = 120;                 // five seconds each, fifteen in all
var HORIZON = 0.62;             // where the ground meets the sky
var GROUND_Y = H * HORIZON;

doc.width = W;
doc.height = H;
doc.frameRate = FPS;

// ---------------------------------------------------------------------------
// The cast
// ---------------------------------------------------------------------------
//
// Two men and a woman, told apart by the three things that actually read at the
// size a figure is drawn: height, palette, and how large the head is. They are
// cast fresh in each shot, because a scene carries its own library -- which is
// exactly why the asset library exists, and why the moon below goes through it.

var CAST = [
  {
    name: "Arun", height: 430, headRatio: 1 / 7.2, facing: 1,
    skin: "#C98A5E", shirt: "#2E5F8A", trousers: "#242A38", eyes: "#2A1C12",
    blink: 11, breathe: 13,
  },
  {
    name: "Kabir", height: 402, headRatio: 1 / 7.0, facing: -1,
    skin: "#A9713F", shirt: "#8A4230", trousers: "#33384A", eyes: "#1E1208",
    blink: 14, breathe: 15,
  },
  {
    name: "Meera", height: 388, headRatio: 1 / 6.6, facing: -1,
    skin: "#D69A6A", shirt: "#C0577F", trousers: "#6B2A44", eyes: "#241408",
    blink: 12, breathe: 14,
  },
];

// Cast everybody onto the shot that is open, standing where `marks` says.
function castEveryone(marks) {
  var out = [];
  for (var i = 0; i < CAST.length; i++) {
    var who = CAST[i];
    var person = doc.addCharacter({
      name: who.name,
      x: marks[i].x, y: marks[i].y,
      height: who.height * (marks[i].scale === undefined ? 1 : marks[i].scale),
      headRatio: who.headRatio,
      facing: marks[i].facing === undefined ? who.facing : marks[i].facing,
      skin: who.skin, shirt: who.shirt, trousers: who.trousers, eyes: who.eyes,
      blink: who.blink, breathe: who.breathe,
      frames: SHOT,
    });
    person.name = who.name;
    out.push(person);
  }
  return out;
}

// ---------------------------------------------------------------------------
// A sky that moves, is masked, and is see-through
// ---------------------------------------------------------------------------
//
// Three things at once, and each is doing a different job:
//
//   * **Texture** --- a seamless procedural tile, so the sky has weather in it
//     rather than being a flat band of colour. It costs a handful of numbers in
//     the file, not a bitmap.
//   * **Alpha** --- the fills are #RRGGBBAA. Two sheets at different opacities
//     and different speeds is what makes a flat sky read as deep; one opaque
//     sheet sliding across is a card being pulled past a window.
//   * **A mask** --- the sheets are three stages wide so they can drift without
//     the far edge arriving, and the mask is what stops them painting over the
//     ground. Without it the drifting sky slides down over the actors' feet.
//
// The drift is a live modifier, so it costs no keyframes and cannot be knocked
// out by re-timing the shot. `span` is how far it travels before it starts
// again, `phase` how far into that loop it already is -- and the phase is what
// makes two sheets a *sky* rather than two sheets in convoy.
function movingSky(backdropLayer, options) {
  var o = options || {};
  var top = o.top || "#FFFFFF22";
  var bottom = o.bottom || "#FFFFFF14";
  var texture = o.texture || "Noise";

  // The mask first, so that when both are moved into place the mask ends up
  // directly above the run it clips -- which is the rule: a mask clips the
  // unbroken run of masked layers under it.
  var mask = doc.newLayer("Sky Mask", "mask");
  doc.addRectangleOn(mask, 0, { left: -W, top: -H, right: W * 2, bottom: GROUND_Y },
                     "#FFFFFF");

  var far = doc.newLayer("Sky Far", "masked", 900);
  var near = doc.newLayer("Sky Near", "masked", 600);

  var farSheet = doc.addRectangleOn(
    far, 0, { left: -W, top: -H * 0.6, right: W * 2, bottom: GROUND_Y }, bottom);
  var nearSheet = doc.addRectangleOn(
    near, 0, { left: -W, top: -H * 0.4, right: W * 2, bottom: GROUND_Y * 0.92 }, top);

  // **The see-through has to be in the texture, not on the rectangle.**
  //
  // A texture *replaces* a shape's fill, so the #RRGGBBAA the rectangle was
  // drawn with stops meaning anything the moment one is applied. The first
  // version set the alpha on the rectangle and got a solid white sheet over
  // the whole sky whatever number was in it -- the render came back as fog.
  // The alpha belongs on the texture's two colours, where it survives.

  // **Big tiles, low detail.** The first version used a 520-unit tile at
  // detail 4 and the render came back with the same blob repeating eleven times
  // across the sky, which is the one thing a tiling texture may never do to the
  // eye. A tile wider than the stage, at the coarsest detail, is weather; a
  // small one is wallpaper.
  doc.textureObject(farSheet, { kind: texture, fg: bottom,
                                bg: o.bg || "#FFFFFF00", detail: 1, cell: 2600 });
  doc.textureObject(nearSheet, { kind: texture, fg: top,
                                 bg: o.bg || "#FFFFFF00", detail: 2, cell: 1700 });

  // Far cloud is slower and further into its loop than near cloud. That is
  // parallax, it is free, and it is most of what makes a flat sky read as deep.
  doc.addModifier(farSheet, "drift", { dx: 9, dy: 0, span: W, phase: 0.35 });
  doc.addModifier(nearSheet, "drift", { dx: 22, dy: 0, span: W, phase: 0.0 });

  // In front of the backdrop and behind everything else, in that order:
  // mask, near sheet, far sheet, sky.
  inFrontOf(far, backdropLayer);
  inFrontOf(near, far);
  inFrontOf(mask, near);

  return { mask: mask, far: far, near: near };
}

// Move `layer` to sit directly in front of `reference`.
//
// The minus one is not a fudge. A reorder takes the layer out of the stack
// before it puts it back, so everything behind where it *was* shifts up by one
// -- and a layer moved from the front of the stack to the reference's own row
// therefore lands one place too far back, which is behind the thing it was
// meant to sit in front of. Ask which way it is travelling and the arithmetic
// comes out either way.
function inFrontOf(layer, reference) {
  var to = doc.layerRow(reference);
  var from = doc.layerRow(layer);
  doc.moveLayerTo(layer, from < to ? to - 1 : to);
}

// ---------------------------------------------------------------------------
// Shot 1 --- a forest at first light
// ---------------------------------------------------------------------------

fl.trace("Shot 1: the forest");
doc.scenes.rename(0, "1 - Forest, first light");
doc.scenes.setLength(SHOT);

var forest = doc.setTheScene({
  setting: "daylight", cast: 0, horizon: HORIZON, frames: SHOT,
  lit: true, clouds: true,
});
// **The scenery first, then the weather.** Both end up between the sky and the
// cast, and the order they are laid in is what decides which is nearer: the
// scenery takes the slot in front of the backdrop, and the sky sheets then take
// it back off it -- so the weather sits behind the treeline, which is where
// weather belongs.
doc.layScenery("forest", forest.horizonY, forest.backdrop);
movingSky(forest.backdrop, {
  texture: "Noise", top: "#FFFFFF18", bottom: "#EAF2FF12",
  bg: "#FFFFFF00",
});

// The marks. Staggered in depth -- further back is smaller and stands higher
// up the ground -- and kept clear of the bottom of the stage, because the
// camera comes in and a walk with its feet off the bottom of the frame is a
// walk nobody can watch.
var one = castEveryone([
  { x: W * 0.16, y: GROUND_Y + 235, facing: 1 },
  { x: W * 0.78, y: GROUND_Y + 185, scale: 0.92, facing: -1 },
  { x: W * 0.52, y: GROUND_Y + 210, scale: 0.96, facing: -1 },
]);

// Arun arrives. A walk travels on the object's own transform, so the face --
// which is on a layer following his -- arrives with him, with nothing keyed on
// it at all. That is the layer-parenting rig doing its job.
doc.performFully(one[0].body, "walk", 0, 78, { distance: W * 0.28 });
doc.perform(one[0].body, "idle", 78, SHOT);

// Kabir has been waiting. He turns to look, then settles.
doc.perform(one[1].body, "idle", 0, 40);
doc.perform(one[1].body, "turn", 40, 62);
doc.perform(one[1].body, "idle", 62, SHOT);

// Meera watches him come, and speaks.
doc.perform(one[2].body, "idle", 0, 54);
doc.perform(one[2].body, "react", 54, 72);
doc.perform(one[2].body, "talk", 72, SHOT);

// The take, and her mouth on it. `lipSync` analyses the clip into visemes and
// writes one keyframe per *change* of shape onto her own mouth layer, placed on
// her own face -- not one keyframe per frame, which is unreadable and
// unadjustable.
if (doc.sounds.count > 0) {
  doc.sounds.attach(0, { frame: 0, volume: 0.9, name: "Shot 1" });
  // From the frame she starts speaking, not from the head of the shot: the
  // take is the whole five seconds and her line is the last two, so a mouth
  // keyed from zero is a woman mouthing along to somebody else.
  var said = doc.lipSync({
    sound: 0, layer: one[2].mouthLayer, mouth: one[2].mouthSymbol,
    x: one[2].mouthX, y: one[2].mouthY, scale: one[2].mouthScale, start: 72,
  });
  fl.trace("  lip sync:", said.message);
}

// The moon, drawn once here and filed in the asset library so shot 3 can have
// it without drawing it again. A scene carries its own library; the asset
// library is the one that outlives the document.
var moonLayer = doc.newLayer("Moon (drawn here, used at night)", "normal", 950);
var moonDisc = doc.addOvalOn(moonLayer, 0,
  { left: W * 0.70, top: H * 0.08, right: W * 0.70 + 150, bottom: H * 0.08 + 150 },
  "#F6F1DE");
// The halo, at an eighth of nothing: a disc with no glow around it is a hole
// punched in the sky rather than a moon in it.
var moonHalo = doc.addOvalOn(moonLayer, 0,
  { left: W * 0.70 - 60, top: H * 0.08 - 60, right: W * 0.70 + 210, bottom: H * 0.08 + 210 },
  "#F6F1DE20");
var moon = doc.symbolFromObjects("Moon", "graphic", [moonHalo, moonDisc], moonLayer, 0);
doc.assets.save(moon.symbol, "Moon", "Three Roads");
// It has no business in a daylit forest; it was only ever drawn here.
doc.setLayerKind(moonLayer, "guide");
fl.trace("  filed the moon as an asset");

// The camera follows him in, then holds. Eased at both ends: a pan that starts
// at full speed and stops dead is the most reliable tell that a shot was
// generated.
doc.camera.setEasedKey(0, { x: W * 0.34, y: H * 0.52, zoom: 1.02, ease: "smooth" });
doc.camera.setEasedKey(78, { x: W * 0.50, y: H * 0.52, zoom: 1.06, ease: "smooth" });
doc.camera.setEasedKey(SHOT - 1, { x: W * 0.54, y: H * 0.51, zoom: 1.10, ease: "linear" });

// **Exactly five seconds.** Lip sync makes a layer long enough to hold the
// whole take, which is right and is longer than this shot; the film is cut to
// its shots rather than to whatever ran over the end of one.
fl.trace("  shot 1 is " + doc.scenes.setLength(SHOT) + " frames");

// ---------------------------------------------------------------------------
// Shot 2 --- a village at noon, directed from prose
// ---------------------------------------------------------------------------
//
// Everything above was written out by hand. This one is written the way a
// writer writes, and the director stages it, casts it, blocks it to a schedule
// and frames it. It answers with the cast it made and the frames it planned
// them talking over, so the take goes in the right mouth over the right frames.

fl.trace("Shot 2: the village");
doc.scenes.add("2 - Village, noon");
doc.scenes.setLength(SHOT);

// Written the way a writer writes. "Meanwhile" is what puts Kabir's gesture
// alongside Arun's entrance instead of after it -- and that is the difference
// between a shot that fits in five seconds and one that runs to eight.
var directed = doc.directScene(
  "A village at noon.\n" +
  "Arun walks in from the left. Meanwhile Kabir points at the well.\n" +
  "Arun talks to Meera for 2 seconds. Meera listens.\n"
);
fl.trace("  " + directed.message);
for (var i = 0; i < directed.ignored.length; i++) {
  fl.trace("  the director could not read: " + directed.ignored[i]);
}

// The village goes in behind what the director staged, at the horizon it used.
var villageSky = doc.layerNamed("Sky");
doc.layScenery("village", GROUND_Y, villageSky);
movingSky(villageSky, {
  texture: "Noise", top: "#FFFFFF20", bottom: "#FFF6E018",
  bg: "#FFFFFF00",
});

// The dialogue-to-performance half: the director knew who was speaking and over
// which frames, and until it answered with them that was computed and thrown
// away. Now every talk beat becomes a lip-synced mouth on the actor who has it.
if (doc.sounds.count > 1 && directed.talking.length > 0) {
  doc.sounds.attach(1, { frame: 0, volume: 0.9, name: "Shot 2" });
  for (var t = 0; t < directed.talking.length; t++) {
    var beat = directed.talking[t];
    var who = directed.cast[beat.actor];
    if (!who) continue;
    var report = doc.lipSync({
      sound: 1, layer: who.mouthLayer, mouth: who.mouthSymbol,
      x: who.mouthX, y: who.mouthY, scale: who.mouthScale,
      start: beat.from,
    });
    fl.trace("  " + (who.name || "actor " + beat.actor) + ": " + report.message);
  }
}

// The director frames the shot itself; this is one move over the top of it, and
// a small one on purpose. If the audience can see a drift happening it is too
// fast.
doc.camera.move("drift", 0, SHOT - 1);

// The director schedules everyone idling to the end of its own shot, which
// comes out longer than five seconds. Trimmed to the shot -- and the trim takes
// the director's camera keys with it, or the scene would stay the length its
// last key implies.
fl.trace("  shot 2 is " + doc.scenes.setLength(SHOT) + " frames");

// ---------------------------------------------------------------------------
// Shot 3 --- a city at night
// ---------------------------------------------------------------------------

fl.trace("Shot 3: the city at night");
doc.scenes.add("3 - City, night");
doc.scenes.setLength(SHOT);

// **No cloud on this one.** At night the staged cumulus are near-black, and a
// dark blob with a lit rim over a lit skyline reads as a hole in the picture
// rather than as weather. The sky here is stars, a moon, and the drifting
// sheets below.
var city = doc.setTheScene({
  setting: "night", cast: 0, horizon: HORIZON, frames: SHOT,
  lit: true, clouds: false,
});
// A lit skyline along the horizon, and street lamps down the path.
doc.layScenery("city", city.horizonY, city.backdrop);
movingSky(city.backdrop, {
  texture: "Noise", top: "#8FA8D02C", bottom: "#5C6E9A22",
  bg: "#00000000",
});
// Stars over it, and a practical lamp on the near pavement doing the work.
doc.layWeather("stars");
// A practical on the near pavement. Kept modest on purpose: at full strength
// it floods the whole frame sodium-orange and the night stops reading as night,
// which is the one thing a night shot has to do.
doc.addLight("lamp", {
  x: W * 0.26, y: GROUND_Y + 230, height: 240, reach: 620,
  color: "#FFD9A0", intensity: 0.75,
});

// The moon, back out of the asset library rather than drawn again.
var recalled = doc.assets.place("Moon", "Three Roads");
var moonSymbol = doc.findSymbol("Moon");
if (moonSymbol) {
  var skyMoon = doc.newLayer("Moon", "normal", 980);
  inFrontOf(skyMoon, city.backdrop);
  doc.placeSymbol(moonSymbol, skyMoon, 0, { x: 0, y: 0, scale: 1.25 });
  fl.trace("  recalled the moon from the asset library");
} else {
  fl.trace("  the moon was not in the asset library; the sky keeps its stars");
}

var three = castEveryone([
  { x: W * 0.26, y: GROUND_Y + 235, facing: 1 },
  { x: W * 0.82, y: GROUND_Y + 180, scale: 0.90, facing: -1 },
  { x: W * 0.58, y: GROUND_Y + 205, scale: 0.94, facing: -1 },
]);

// The last of the ten actions the performer knows, so that between the three
// shots every one of them is on screen: walk, run, idle, talk, sit, stand up,
// turn, point, reach and react.
doc.perform(three[0].body, "stand", 0, 26);
doc.performFully(three[0].body, "run", 26, 86, { distance: W * 0.30 });
doc.perform(three[0].body, "idle", 86, SHOT);

doc.perform(three[1].body, "sit", 0, 34);
doc.perform(three[1].body, "point", 34, 58);
doc.perform(three[1].body, "idle", 58, SHOT);

doc.perform(three[2].body, "idle", 0, 30);
doc.perform(three[2].body, "talk", 30, 96);
doc.perform(three[2].body, "reach", 96, SHOT);

if (doc.sounds.count > 2) {
  doc.sounds.attach(2, { frame: 0, volume: 0.9, name: "Shot 3" });
  var lastLine = doc.lipSync({
    sound: 2, layer: three[2].mouthLayer, mouth: three[2].mouthSymbol,
    x: three[2].mouthX, y: three[2].mouthY, scale: three[2].mouthScale, start: 30,
  });
  fl.trace("  lip sync:", lastLine.message);
}

// Open on the lamp, and pull back to the street. A reveal is defined by where
// it *ends*, so the wide is the framing set up and the opening is derived.
doc.camera.setEasedKey(0, { x: W * 0.34, y: H * 0.54, zoom: 1.22, ease: "smooth" });
doc.camera.setEasedKey(70, { x: W * 0.48, y: H * 0.52, zoom: 1.12, ease: "smooth" });
doc.camera.setEasedKey(SHOT - 1, { x: W * 0.52, y: H * 0.51, zoom: 1.02, ease: "linear" });

fl.trace("  shot 3 is " + doc.scenes.setLength(SHOT) + " frames");

// ---------------------------------------------------------------------------

fl.trace("Built " + doc.scenes.count + " shots, " + (doc.scenes.count * SHOT) +
         " frames, " + ((doc.scenes.count * SHOT) / FPS).toFixed(1) + " seconds.");
