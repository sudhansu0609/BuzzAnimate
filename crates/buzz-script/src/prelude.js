// The API a script actually sees.
//
// Built here, in JavaScript, from the flat primitives Rust puts on `__host`.
// The reason for the split is that *this* file is the part that has to match
// Adobe Animate, and it should be readable against Animate's own JSFL
// documentation rather than buried in binding code.
//
// Names, argument shapes and the rectangle object `{left, top, right, bottom}`
// are Animate's, so a JSFL script written for Animate reads correctly here
// even where BuzzAnimate does not implement the call yet.

(function () {
  "use strict";

  var host = __host;

  // A rectangle as JSFL passes it. Animate uses {left, top, right, bottom};
  // {x, y, width, height} is accepted too because it is the shape people
  // reach for, and refusing it would be pedantry rather than fidelity.
  function readRect(r) {
    if (!r || typeof r !== "object") {
      throw new Error(
        "expected a rectangle like {left:0, top:0, right:100, bottom:50}"
      );
    }
    if (typeof r.left === "number") {
      return [r.left, r.top, r.right, r.bottom];
    }
    if (typeof r.x === "number" && typeof r.width === "number") {
      return [r.x, r.y, r.x + r.width, r.y + r.height];
    }
    throw new Error(
      "a rectangle needs {left, top, right, bottom} or {x, y, width, height}"
    );
  }

  // ---- one element, on a frame ----------------------------------------------
  //
  // Bound to the element's id and the frame it was fetched on, not to a
  // layer index: an id alone finds the layer, and the frame is what tells
  // two keyframes sharing that id apart once `insertKeyframe` (Animate's F6)
  // has cloned one onto a second span with its own edits. Every property
  // re-reads the host on each access, so an element stays live across edits
  // made through a different one, exactly as JSFL's own elements do.
  function elementInfo(id, frame) {
    return JSON.parse(host.elementDescribe(id, frame));
  }
  function Element(id, frame) {
    Object.defineProperties(this, {
      // Not part of JSFL's own Element — BuzzAnimate's way of letting
      // `document.selection = [el]` and `doc.enterEditMode(...)` name which
      // object an Element stands for, without inventing a public property
      // Animate does not have.
      __objectId: { value: id },
      __frame: { value: frame },
      elementType: {
        get: function () { return elementInfo(id, frame).elementType; },
        enumerable: true,
      },
      symbolType: {
        get: function () { return elementInfo(id, frame).symbolType; },
        enumerable: true,
      },
      libraryItem: {
        get: function () {
          var info = elementInfo(id, frame);
          if (!info.libraryItemName) return null;
          return {
            name: info.libraryItemName,
            itemType: info.libraryItemType,
            // Only `.frameCount` is real: a nested `Timeline` over the
            // symbol's own layers is not exposed here, and no ported
            // command reads more than this one number off it.
            timeline: { frameCount: info.libraryFrameCount },
          };
        },
        enumerable: true,
      },
      x: {
        get: function () { return elementInfo(id, frame).x; },
        set: function (v) { host.setElementXY(id, frame, Number(v), NaN); },
        enumerable: true,
      },
      y: {
        get: function () { return elementInfo(id, frame).y; },
        set: function (v) { host.setElementXY(id, frame, NaN, Number(v)); },
        enumerable: true,
      },
      matrix: {
        get: function () {
          var info = elementInfo(id, frame);
          return { a: info.a, b: info.b, c: info.c, d: info.d, tx: info.tx, ty: info.ty };
        },
        set: function (m) {
          var mm = m || {};
          host.setElementMatrix(id, frame, JSON.stringify([
            Number(mm.a), Number(mm.b), Number(mm.c),
            Number(mm.d), Number(mm.tx), Number(mm.ty),
          ]));
        },
        enumerable: true,
      },
      // Which frame of the symbol's own timeline this instance shows.
      // `undefined` on anything that is not an instance, matching JSFL.
      firstFrame: {
        get: function () { return elementInfo(id, frame).firstFrame; },
        set: function (v) { host.setElementFirstFrame(id, frame, Number(v)); },
        enumerable: true,
      },
    });
  }
  Element.prototype.getTransformationPoint = function () {
    return JSON.parse(host.elementTransformationPoint(this.__objectId, this.__frame));
  };

  // ---- one frame, on one layer ----------------------------------------------
  function Frame(layerIndex, frame) {
    Object.defineProperties(this, {
      isKeyframe: {
        get: function () { return host.frameIsKeyframe(layerIndex, frame); },
        enumerable: true,
      },
      startFrame: {
        get: function () { return host.frameStart(layerIndex, frame); },
        enumerable: true,
      },
      duration: {
        get: function () { return host.frameDuration(layerIndex, frame); },
        enumerable: true,
      },
      // "none" | "classic" | "motion" | "shape". Assignable, as the ported
      // commands assign it directly (`frame.tweenType = "none"`) rather than
      // going through `document.setTween`.
      tweenType: {
        get: function () { return host.frameTweenKind(layerIndex, frame); },
        set: function (v) { host.setFrameTweenKind(layerIndex, frame, String(v)); },
        enumerable: true,
      },
      // `{name, sourceFilePath}` if a sound starts on this keyframe, else
      // `null`. BuzzAnimate's sound library holds decoded audio rather than
      // a path on disk, so `sourceFilePath` is a synthesised `file:///name`
      // — enough to pull a base name from, which is all any ported command
      // does with it.
      soundLibraryItem: {
        get: function () { return JSON.parse(host.frameSoundInfo(layerIndex, frame)); },
        enumerable: true,
      },
      elements: {
        get: function () {
          var ids = JSON.parse(host.frameElementIds(layerIndex, frame));
          var out = [];
          for (var i = 0; i < ids.length; i++) out.push(new Element(ids[i], frame));
          return out;
        },
        enumerable: true,
      },
    });
  }

  // ---- one layer ----------------------------------------------------------
  function Layer(index) {
    Object.defineProperties(this, {
      index: { value: index, enumerable: true },
      name: {
        get: function () { return host.layerName(index); },
        set: function (v) { host.setLayerName(index, String(v)); },
        enumerable: true,
      },
      visible: {
        get: function () { return host.layerVisible(index); },
        set: function (v) { host.setLayerVisible(index, !!v); },
        enumerable: true,
      },
      locked: {
        get: function () { return host.layerLocked(index); },
        set: function (v) { host.setLayerLocked(index, !!v); },
        enumerable: true,
      },
      // BuzzAnimate's own: how far the layer sits from the camera. Animate
      // exposes layer depth in its interface but not, historically, to JSFL.
      depth: {
        get: function () { return host.layerDepth(index); },
        set: function (v) { host.setLayerDepth(index, Number(v)); },
        enumerable: true,
      },
      // "normal" | "folder" | "mask" | "masked" | "guide" | "guided" — the
      // check almost every one of the ported commands starts a layer loop
      // with, to skip folders.
      layerType: {
        get: function () { return host.layerType(index); },
        enumerable: true,
      },
      // The enclosing folder, as a Layer, or null — Animate's own shape, so
      // `layer.parentFolder !== null` reads the way the originals wrote it.
      parentFolder: {
        get: function () {
          var p = host.layerParentIndex(index);
          return p < 0 ? null : new Layer(p);
        },
        enumerable: true,
      },
      // This layer's own frame count, distinct from the timeline's
      // document-wide `frameCount` below.
      frameCount: {
        get: function () { return host.layerFrameCount(index); },
        enumerable: true,
      },
      // `layer.frames[f]`: one Frame per frame on this layer, rebuilt on
      // each access for the same reason `Timeline.layers` below is.
      frames: {
        get: function () {
          var count = host.layerFrameCount(index);
          var out = [];
          for (var f = 0; f < count; f++) out.push(new Frame(index, f));
          return out;
        },
        enumerable: true,
      },
    });
  }

  // ---- the timeline -------------------------------------------------------
  function Timeline() {
    Object.defineProperties(this, {
      layerCount: {
        get: function () { return host.layerCount(); },
        enumerable: true,
      },
      frameCount: {
        get: function () { return host.frameCount(); },
        enumerable: true,
      },
      currentFrame: {
        get: function () { return host.currentFrame(); },
        set: function (v) { host.setCurrentFrame(Number(v)); },
        enumerable: true,
      },
      // Rebuilt on each access so it reflects layers added since. A cached
      // array would go stale the moment a script called addNewLayer, which is
      // exactly what a script does.
      layers: {
        get: function () {
          var out = [];
          for (var i = 0; i < host.layerCount(); i++) out.push(new Layer(i));
          return out;
        },
        enumerable: true,
      },
    });
  }

  Timeline.prototype.addNewLayer = function (name) {
    host.addNewLayer(name === undefined ? "" : String(name));
    return 0; // the new layer goes to the front, as in Animate
  };
  Timeline.prototype.deleteLayer = function (index) {
    host.deleteLayer(index === undefined ? 0 : Number(index));
  };
  // `insertFrames(numFrames, allLayers, frameIndex)` — every argument past
  // the first is optional, as in JSFL. With none of them the frame is
  // wherever `setSelectedFrames` last pointed (or the active layer's current
  // frame, if it never has); with `frameIndex` given, that is where the
  // insertion happens instead.
  Timeline.prototype.insertFrames = function (count, allLayers, atFrame) {
    host.insertFramesAt(
      count === undefined ? 1 : Number(count),
      !!allLayers,
      atFrame === undefined ? -1 : Number(atFrame)
    );
  };
  // `insertKeyframe(frameIndex)` — the frame is optional; left off, it is
  // wherever the playhead is.
  Timeline.prototype.insertKeyframe = function (frame) {
    host.insertKeyframeAt(frame === undefined ? -1 : Number(frame));
  };
  Timeline.prototype.insertBlankKeyframe = function (frame) {
    host.insertBlankKeyframeAt(frame === undefined ? -1 : Number(frame));
  };
  // Animate's name for the same thing.
  Timeline.prototype.convertToBlankKeyframes = function (frame) {
    host.insertBlankKeyframeAt(frame === undefined ? -1 : Number(frame));
  };

  // ---- the timeline's own selection ----------------------------------------
  //
  // What `removeFrames`, `clearKeyframes`, `copyFrames`, `pasteFrames` and
  // `createMotionTween` below all act on, exactly as their JSFL originals
  // act on Animate's own timeline selection rather than on an argument.
  Timeline.prototype.setSelectedLayers = function (indexOrArray) {
    var arr = Array.isArray(indexOrArray) ? indexOrArray : [indexOrArray];
    host.setSelectedLayers(JSON.stringify(arr.map(Number)));
  };
  // Two shapes, both Animate's own: `setSelectedFrames(start, end)` on the
  // layer `setSelectedLayers` last named, or
  // `setSelectedFrames([layer, start, end, layer2, start2, end2, ...], replace)`
  // naming a layer per range. `end` is exclusive — one past the last frame in
  // the range — which is also what `getSelectedFrames` returns, so a value it
  // gave back can be handed straight to this. The trailing `replace` flag
  // this takes in JSFL is accepted and ignored: nothing here ever adds to an
  // existing selection instead of replacing it.
  Timeline.prototype.setSelectedFrames = function (a, b) {
    if (Array.isArray(a)) {
      host.setSelectedFramesArray(JSON.stringify(a.map(Number)));
    } else {
      var start = Number(a);
      var end = b === undefined ? start + 1 : Number(b);
      host.setSelectedFramesRange(start, end);
    }
  };
  // The flat `[layer, start, end, layer2, start2, end2, ...]` array Animate
  // returns, `end` exclusive.
  Timeline.prototype.getSelectedFrames = function () {
    return JSON.parse(host.getSelectedFrames());
  };
  Timeline.prototype.removeFrames = function () { host.removeFrames(); };
  Timeline.prototype.clearKeyframes = function () { host.clearKeyframes(); };
  Timeline.prototype.copyFrames = function () { host.copyFrames(); };
  Timeline.prototype.pasteFrames = function () { host.pasteFrames(); };
  // A classic tween across the selected span — see the note on `createMotionTween`
  // in `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md`'s Impl J: named for JSFL's own
  // call, which every one of the ported commands uses to mean the purple
  // Classic Tween rather than Animate's newer Motion Tween.
  Timeline.prototype.createMotionTween = function () { host.createMotionTween(); };

  // ---- the library --------------------------------------------------------
  function Library() {
    Object.defineProperties(this, {
      itemCount: {
        get: function () { return host.libraryItemCount(); },
        enumerable: true,
      },
      items: {
        get: function () {
          var out = [];
          for (var i = 0; i < host.libraryItemCount(); i++) {
            out.push({ index: i, name: host.libraryItemName(i) });
          }
          return out;
        },
        enumerable: true,
      },
    });
  }

  // ---- the document -------------------------------------------------------
  function Document() {
    // The stroke and fill new shapes are drawn with. Animate keeps these on
    // the document too, set through setFillColor / setStrokeColor.
    var fill = "#000000";
    var stroke = "";
    var strokeWidth = 1;

    Object.defineProperties(this, {
      width: {
        get: function () { return host.docWidth(); },
        set: function (v) { host.setDocSize(Number(v), host.docHeight()); },
        enumerable: true,
      },
      height: {
        get: function () { return host.docHeight(); },
        set: function (v) { host.setDocSize(host.docWidth(), Number(v)); },
        enumerable: true,
      },
      frameRate: {
        get: function () { return host.frameRate(); },
        set: function (v) { host.setFrameRate(Number(v)); },
        enumerable: true,
      },
      backgroundColor: {
        get: function () { return host.backgroundColor(); },
        set: function (v) { host.setBackgroundColor(String(v)); },
        enumerable: true,
      },
      // The selected elements, as real Elements — evaluated at the playhead,
      // which is where a script that reads `document.selection` is looking.
      // Assignable too: `document.selection = [el, ...]` is how a ported
      // command hands `enterEditMode` something to enter, exactly as it does
      // in Animate.
      selection: {
        get: function () {
          var ids = JSON.parse(host.selectionIds());
          var frame = host.currentFrame();
          var out = [];
          for (var i = 0; i < ids.length; i++) out.push(new Element(ids[i], frame));
          return out;
        },
        set: function (list) {
          var arr = list || [];
          var ids = [];
          for (var i = 0; i < arr.length; i++) {
            var e = arr[i];
            if (e && typeof e.__objectId === "number") ids.push(e.__objectId);
          }
          host.setSelectionIds(JSON.stringify(ids));
        },
        enumerable: true,
      },
      library: { value: new Library(), enumerable: true },
      // `doc.timelines[0]` — Animate's root timeline, alongside every symbol
      // a script may be editing in place. Simplified here to the timeline
      // currently open for editing: every one of the ported commands reads
      // this before it ever calls `enterEditMode`, at which point the two
      // are the same thing anyway.
      timelines: { value: [], enumerable: true, writable: true },
    });
    // Not `this.getTimeline()`: that is defined further down this same
    // constructor, and this line runs before it would exist.
    this.timelines[0] = new Timeline();

    this.setFillColor = function (colour) {
      fill = colour === null || colour === undefined ? "" : String(colour);
    };
    this.setStrokeColor = function (colour) {
      stroke = colour === null || colour === undefined ? "" : String(colour);
    };
    this.setStrokeSize = function (size) {
      strokeWidth = Number(size);
    };

    this.addNewRectangle = function (rect) {
      var r = readRect(rect);
      host.addRectangle(r[0], r[1], r[2], r[3], fill, stroke, strokeWidth);
    };
    this.addNewOval = function (rect) {
      var r = readRect(rect);
      host.addOval(r[0], r[1], r[2], r[3], fill, stroke, strokeWidth);
    };

    this.selectAll = function () { host.selectAll(); };
    this.selectNone = function () { host.selectNone(); };
    this.deleteSelection = function () { host.deleteSelection(); };
    this.moveSelectionBy = function (delta) {
      if (!delta || typeof delta !== "object") {
        throw new Error("expected a distance like {x: 10, y: 0}");
      }
      host.moveSelectionBy(Number(delta.x) || 0, Number(delta.y) || 0);
    };

    // Open the selected symbol instance's own timeline for editing —
    // `'inPlace'` where the instance sits, or Animate's plain Edit Symbol
    // (at the origin) for anything else. `document.selection` must name
    // exactly the one instance to enter, as it does in Animate.
    this.enterEditMode = function (mode) {
      host.enterEditMode(mode === undefined ? "" : String(mode));
    };
    this.exitEditMode = function () { host.exitEditMode(); };

    this.convertToSymbol = function (type, name) {
      host.convertToSymbol(
        type === undefined ? "graphic" : String(type),
        name === undefined ? "" : String(name)
      );
    };

    // ---- animation --------------------------------------------------------
    //
    // Animate's JSFL has none of this, so these are BuzzAnimate's own and are
    // named for what they do rather than for a function somebody already knows.
    // The rule they follow is the one the rest of the API follows: they drive
    // the document model, and what they leave behind is ordinary layers,
    // keyframes and poses that a later line of the same script can edit.

    this.setTween = function (layerIndex, frame, kind) {
      host.setTween(Number(layerIndex), Number(frame), String(kind || "motion"));
    };
    this.removeTween = function (layerIndex, frame) {
      host.setTween(Number(layerIndex), Number(frame), "none");
    };
    // Animate's ease slider: -100 slows in, +100 slows out, 0 is linear.
    this.setEase = function (layerIndex, frame, strength) {
      host.setEase(Number(layerIndex), Number(frame), Number(strength) || 0);
    };

    this.camera = {
      setEnabled: function (on) { host.setCameraEnabled(!!on); },
      // A shot: where the camera looks, how close, and how the horizon tips.
      setKey: function (frame, shot) {
        var s = shot || {};
        host.setCameraKey(
          Number(frame),
          Number(s.x) || 0,
          Number(s.y) || 0,
          s.zoom === undefined ? 1 : Number(s.zoom),
          Number(s.rotation) || 0
        );
      },
      removeKey: function (frame) { return host.removeCameraKey(Number(frame)); },
      // A focus pull: key the depth that stays sharp, and how open the lens is.
      setFocusKey: function (frame, depth, aperture) {
        host.setFocusKey(Number(frame), Number(depth) || 0, Number(aperture) || 0);
      },
      // How long the shutter stays open, as a fraction of a frame. 0.5 is the
      // 180-degree shutter most film is shot at.
      setShutter: function (shutter, samples) {
        host.setShutter(Number(shutter) || 0, samples === undefined ? 0 : Number(samples));
      },
    };

    // Live modifiers, on whatever is selected.
    this.addWiggle = function (amplitude, frequency) {
      host.addWiggle(
        amplitude === undefined ? 4 : Number(amplitude),
        frequency === undefined ? 2 : Number(frequency)
      );
    };
    this.addSpring = function (options) {
      var o = options || {};
      host.addSpring(
        o.stiffness === undefined ? 8 : Number(o.stiffness),
        o.damping === undefined ? 0.5 : Number(o.damping),
        o.coupling === undefined ? 0.5 : Number(o.coupling)
      );
    };
    this.clearModifiers = function () { host.clearModifiers(); };

    this.addText = function (x, y, content, options) {
      var o = options || {};
      return host.addText(
        Number(x) || 0,
        Number(y) || 0,
        String(content === undefined ? "" : content),
        o.size === undefined ? 48 : Number(o.size),
        o.font === undefined ? "" : String(o.font)
      );
    };

    // ---- the performance --------------------------------------------------
    //
    // The two that make a script worth writing.

    // Animate a character: "walk", "run", "talk" or "idle", over a range of
    // frames. Returns how many keyframes were written.
    this.perform = function (objectId, action, from, to) {
      return host.perform(
        Number(objectId),
        String(action),
        Number(from) || 0,
        Number(to)
      );
    };

    // A whole staged, animated scene from a few lines of prose. Returns how
    // long the shot came out, in frames.
    //
    //   document.direct("Night. Ana walks in from the left.\nAna talks to Ben.");
    this.direct = function (story) {
      return host.direct(String(story === undefined ? "" : story));
    };

    // The same, answering with who it cast and when it planned them talking --
    // so a script can lip-sync the people the director just staged, over the
    // frames the director itself chose.
    this.directScene = function (story) {
      return JSON.parse(host.directScene(String(story === undefined ? '' : story)));
    };

    // ---- the film ---------------------------------------------------------
    //
    // Everything above edits *a* scene. A film is several, and until these
    // existed a script could build the first shot and had no way to reach the
    // second -- so an unattended run could automate a shot and never a film.
    //
    // The wide calls take and return plain objects; JSON is only how they cross
    // into Rust, and no caller should ever see it.

    function fromJson(text) {
      return text === undefined || text === "" ? {} : JSON.parse(text);
    }
    function toJson(value) {
      return JSON.stringify(value || {});
    }

    this.scenes = {
      get count() { return host.sceneCount(); },
      get index() { return host.sceneIndex(); },
      // Switch to a shot. The selection and the playhead belong to the scene
      // they were in, so they are cleared rather than carried across.
      go: function (index) { host.setSceneIndex(Number(index)); },
      // Add a shot and switch to it. It inherits the stage size and the frame
      // rate, which are properties of the film, and nothing else.
      add: function (name) { return host.addScene(String(name || "")); },
      name: function (index) { return host.sceneName(Number(index)); },
      rename: function (index, name) { host.setSceneName(Number(index), String(name)); },
      // How long this shot runs. Every layer is stretched to reach it.
      // Set the shot's length, and answer with what it actually came out as.
      // Trimming a directed shot means trimming its camera too: the scene is
      // as long as its longest layer *or* its last camera key, so leaving the
      // keys behind would leave the shot the length it was.
      setLength: function (frames) { return host.setSceneFrames(Number(frames)); },
    };

    // Ground, backdrop, a light rig, and optionally cloud and water:
    //
    //   var set = document.setTheScene({setting: "night", clouds: true});
    //   document.layScenery("city", set.horizonY, set.backdrop);
    this.setTheScene = function (options) {
      return fromJson(host.setTheScene(toJson(options)));
    };
    // Trees, houses, a skyline, grass -- laid as effect-brush strokes, which is
    // the same call the Brush tool makes.
    this.layScenery = function (kind, horizonY, backdropLayer) {
      return fromJson(host.layScenery(String(kind), Number(horizonY) || 0,
                                      Number(backdropLayer) || 0));
    };
    // Rain, snow, stars, fireflies -- across the whole stage.
    this.layWeather = function (kind) { return host.layWeather(String(kind)); };
    this.addLight = function (kind, options) {
      return host.addLight(String(kind), toJson(options));
    };

    // Cast somebody: a bone-rigged body, a face parented to it, a blink and a
    // breath. Answers with every id the rest of the script will need.
    this.addCharacter = function (options) {
      return fromJson(host.addCharacter(toJson(options)));
    };
    // A performance with its size and its travel, where `perform` takes the
    // defaults.
    this.performFully = function (objectId, action, from, to, options) {
      var o = options || {};
      return host.performFully(
        Number(objectId), String(action), Number(from) || 0, Number(to),
        o.amount === undefined ? 0 : Number(o.amount),
        o.distance === undefined ? 0 : Number(o.distance)
      );
    };
    // A live modifier on one object, named rather than selected.
    this.addModifier = function (objectId, kind, options) {
      host.addModifierTo(Number(objectId), String(kind), toJson(options));
    };

    // ---- rigging ----------------------------------------------------------
    //
    // Layer parenting: the half of a puppet rig that is not bones. Pass 0 to
    // unlink.
    this.parentLayer = function (child, parent, frame) {
      return host.parentLayer(Number(child), Number(parent) || 0, Number(frame) || 0);
    };
    // A new layer, and its id back -- which JSFL's addNewLayer does not give,
    // because in Animate you go and look at the timeline instead.
    this.newLayer = function (name, kind, depth) {
      return host.newLayer(String(name || ""), String(kind || "normal"), Number(depth) || 0);
    };
    this.setLayerKind = function (layer, kind) {
      host.setLayerKindOf(Number(layer), String(kind));
    };
    // Every layer, front first, with its id, name and what it follows.
    this.layers = function () { return JSON.parse(host.layerIds()); };
    this.layerNamed = function (name) {
      var all = this.layers();
      for (var i = 0; i < all.length; i++) if (all[i].name === name) return all[i].id;
      return 0;
    };
    this.moveLayerTo = function (layer, row) {
      return host.moveLayerTo(Number(layer), Number(row));
    };
    this.layerRow = function (layer) { return host.layerRow(Number(layer)); };
    // Draw on a named layer at a named frame. The JSFL addNewRectangle draws
    // wherever the editor is pointing, which a script building three scenes
    // has no way to say.
    this.addRectangleOn = function (layer, frame, rect, fill) {
      var r = readRect(rect);
      return host.addRectangleOn(Number(layer), Number(frame) || 0,
                                 r[0], r[1], r[2], r[3], String(fill));
    };
    this.addOvalOn = function (layer, frame, rect, fill) {
      var r = readRect(rect);
      return host.addOvalOn(Number(layer), Number(frame) || 0,
                            r[0], r[1], r[2], r[3], String(fill));
    };
    // A seamless procedural texture on a shape.
    this.textureObject = function (objectId, options) {
      var o = options || {};
      host.textureObject(
        Number(objectId), String(o.kind || "Noise"),
        String(o.fg || "#FFFFFF"), String(o.bg || "#000000"),
        o.detail === undefined ? 4 : Number(o.detail),
        o.cell === undefined ? 0 : Number(o.cell)
      );
    };

    // ---- dialogue ---------------------------------------------------------
    //
    // The host opens the file; a script is handed what came out of it. See the
    // note in film.rs on why the boundary is where it is.
    this.sounds = {
      get count() { return host.soundCount(); },
      info: function (index) { return fromJson(host.soundInfo(Number(index))); },
      // Put one on the timeline, on a sound layer of its own.
      attach: function (index, options) {
        var o = options || {};
        return fromJson(host.attachSound(
          Number(index), String(o.name || ""),
          Number(o.frame) || 0,
          o.volume === undefined ? 1 : Number(o.volume)
        ));
      },
      // Where the voice speaks and where it breathes.
      phrases: function (index) { return JSON.parse(host.phrasesIn(Number(index))); },
    };
    // Analyse a track into visemes and write a keyframe per mouth shape onto
    // one character's own mouth layer:
    //
    //   document.lipSync({sound: 0, layer: ana.mouthLayer, mouth: ana.mouthSymbol,
    //                     x: ana.mouthX, y: ana.mouthY, scale: ana.mouthScale});
    this.lipSync = function (options) {
      return fromJson(host.lipSync(toJson(options)));
    };

    // ---- the library and the assets --------------------------------------
    this.findSymbol = function (name) { return host.findSymbol(String(name)); };
    // Place an instance. frameOfSymbol shows one drawing and holds there --
    // a mouth shape, a turnaround view -- and is left off for one that plays.
    this.placeSymbol = function (symbol, layer, frame, options) {
      var o = options || {};
      return host.placeSymbol(
        Number(symbol), Number(layer), Number(frame) || 0,
        Number(o.x) || 0, Number(o.y) || 0,
        o.scale === undefined ? 1 : Number(o.scale),
        o.frameOfSymbol === undefined ? -1 : Number(o.frameOfSymbol)
      );
    };
    this.symbolFromObjects = function (name, kind, ids, layer, frame) {
      return fromJson(host.symbolFromObjects(
        String(name), String(kind || "graphic"), JSON.stringify(ids || []),
        Number(layer), Number(frame) || 0
      ));
    };
    // The library on disk, outside any one document.
    this.assets = {
      list: function () { return JSON.parse(host.assetNames()); },
      save: function (symbol, name, folder) {
        return fromJson(host.saveAsset(Number(symbol), String(name), String(folder || "")));
      },
      place: function (name, folder) {
        return fromJson(host.placeAsset(String(name), String(folder || "")));
      },
    };

    // ---- the camera, moving ----------------------------------------------
    //
    // These sit beside camera.setKey above rather than replacing it: that one
    // writes a framing, and these write a *move*, already eased at both ends.
    this.camera.move = function (kind, from, to) {
      return host.cameraMove(String(kind), Number(from) || 0, Number(to));
    };
    this.camera.setEasedKey = function (frame, shot) {
      var s = shot || {};
      host.setEasedCameraKey(
        Number(frame), Number(s.x) || 0, Number(s.y) || 0,
        s.zoom === undefined ? 1 : Number(s.zoom),
        Number(s.rotation) || 0,
        String(s.ease || "smooth")
      );
    };

    this.getTimeline = function () { return new Timeline(); };
  }

  var theDocument = new Document();

  // ---- fl -----------------------------------------------------------------
  var fl = {
    version: "BuzzAnimate 0.1",
    trace: function () {
      // Animate's trace takes one value; joining several is friendlier and
      // costs nothing in fidelity, since passing one still behaves the same.
      var parts = [];
      for (var i = 0; i < arguments.length; i++) {
        var a = arguments[i];
        parts.push(a === null ? "null" : a === undefined ? "undefined" : String(a));
      }
      host.trace(parts.join(" "));
    },
    getDocumentDOM: function () { return theDocument; },

    // Where the scripts live, as Animate spells it. Scripts concatenate onto
    // this: fl.configURI + "Commands/commonVariables.jsfl".
    configURI: host.configUri(),

    // **Pull in another script.**
    //
    // The line almost every command on a shelf opens with, because the shelf
    // shares its settings through one file. Evaluated in *global* scope —
    // through indirect eval — because that file declares its settings with
    // `var`, and a `var` inside a function would be invisible to the caller,
    // which is the one thing this call exists to avoid.
    //
    // Animate's optional trailing arguments are honoured too: pass a function
    // name and its arguments and it is called once the file has loaded.
    runScript: function (uri, funcName) {
      var source = host.readScript(String(uri));
      var indirectEval = eval;
      indirectEval(source);
      if (funcName === undefined) {
        return undefined;
      }
      var fn = globalThis[String(funcName)];
      if (typeof fn !== "function") {
        throw new Error(
          String(uri) + " has no function called " + String(funcName)
        );
      }
      return fn.apply(null, Array.prototype.slice.call(arguments, 2));
    },

    // The nearest thing to Animate's menu a script gets: BuzzAnimate has no
    // menu for a script to press, so this reads the *name* of the three
    // commands the ported shelf actually calls (always as a fallback, after
    // a direct call already did the same thing) and does that action on the
    // timeline's current selection instead of leaving it a silent no-op.
    // Anything else is accepted and does nothing, exactly as an unhandled
    // menu name would if Animate's own dispatch swallowed it.
    runMenuCommand: function (name) {
      host.runMenuCommand(String(name === undefined ? "" : name));
    },

    // Animate dims the whole application while a script runs, so long ones
    // do not look hung. BuzzAnimate already shows its own "running" overlay
    // around the whole script, so this is a no-op rather than a second one.
    showIdleMessage: function () {},
  };
  // Animate routes trace through the Output panel; scripts written against it
  // sometimes call it that way.
  fl.outputPanel = { trace: fl.trace, clear: function () {} };

  // A script cannot reach the file system — that is the sandbox, and it
  // stays — so `FLfile` only ever sees the same one folder `fl.runScript`
  // does. None of the six ported commands call anything on `FLfile` but
  // `.exists`, from a helper function none of them actually reaches; the
  // rest are stubs so a shelf that does reach them fails on what it tried to
  // do rather than on `FLfile` not existing at all.
  globalThis.FLfile = {
    exists: function (uri) {
      return host.scriptExists(String(uri === undefined ? "" : uri));
    },
    uriToPlatformPath: function (uri) { return String(uri === undefined ? "" : uri); },
    platformPathToURI: function (path) { return String(path === undefined ? "" : path); },
    listFolder: function () { return []; },
  };

  // ---- the dialogs a JSFL script expects ----------------------------------
  //
  // `alert` is how a JSFL command reports that it could not find a layer, and
  // it is *everywhere* on a real shelf: on 20 of 62 commands measured, an
  // undefined `alert` was the first thing that stopped the script — usually
  // inside the error handling, so the script died reporting the problem rather
  // than doing the work.
  //
  // A script runs with nobody watching, so these cannot open a window. They
  // record the question, answer it the way an unattended run has to, and let
  // the script carry on; the host shows what was asked when the run ends.
  globalThis.alert = function (message) {
    host.alert(message === undefined ? "" : String(message));
  };
  // Animate's own spelling of the same thing.
  fl.alert = globalThis.alert;

  // A real question, distinct from `alert`/`confirm` above: a caller may
  // answer it. Unattended, this answers with `initial` exactly as it always
  // has; a caller that first ran the script to find out what it would ask
  // (see `ScriptContext::prompt_answers`) passes the person's own answers
  // back in, one per call, in the order the script asks them.
  globalThis.prompt = function (message, initial) {
    var fallback = initial === undefined || initial === null ? "" : String(initial);
    return host.prompt(message === undefined ? "" : String(message), fallback);
  };

  // **Yes, because No means the script does nothing.** These are the user's
  // own commands, run deliberately; a confirmation answered No every time
  // would make half a shelf silently no-op, which is a worse answer than
  // doing the work and saying it was asked.
  globalThis.confirm = function (message) {
    host.askedWith(message === undefined ? "" : String(message), "yes");
    return true;
  };

  globalThis.fl = fl;
  // `document` is what nearly every JSFL script assigns first.
  globalThis.document = theDocument;
})();
