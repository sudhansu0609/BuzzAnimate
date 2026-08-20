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
  Timeline.prototype.insertFrames = function (count) {
    host.insertFrames(count === undefined ? 1 : Number(count));
  };
  Timeline.prototype.insertKeyframe = function () {
    host.insertKeyframe();
  };
  Timeline.prototype.insertBlankKeyframe = function () {
    host.insertBlankKeyframe();
  };
  // Animate's name for the same thing.
  Timeline.prototype.convertToBlankKeyframes = function () {
    host.insertBlankKeyframe();
  };

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
      // An array of the selected items. Length is what scripts overwhelmingly
      // use it for, and it is the part we can answer honestly today.
      selection: {
        get: function () {
          var out = [];
          for (var i = 0; i < host.selectionCount(); i++) out.push({ index: i });
          return out;
        },
        enumerable: true,
      },
      library: { value: new Library(), enumerable: true },
    });

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

    this.convertToSymbol = function (type, name) {
      host.convertToSymbol(
        type === undefined ? "graphic" : String(type),
        name === undefined ? "" : String(name)
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
  };
  // Animate routes trace through the Output panel; scripts written against it
  // sometimes call it that way.
  fl.outputPanel = { trace: fl.trace, clear: function () {} };

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

  globalThis.prompt = function (message, initial) {
    var answer = initial === undefined || initial === null ? "" : String(initial);
    host.askedWith(message === undefined ? "" : String(message), answer);
    return answer;
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
