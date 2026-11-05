//! scratch probe — delete after use. Opens a .buzz whose first object is a
//! picture, runs the editor's Trace Bitmap (`quick`) or the dialog's Trace with
//! the workspace's saved options (`dialog`), prints time/shape count and
//! renders the stage to <out.png>.
//!
//! zz_trace_probe <doc.buzz> <quick|dialog> <out.png>
//! Set BUZZANIMATE_WORKSPACE to a copy of the workspace first.
use buzz_ui::Command;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let doc = buzz_doc::Document::open(&a[0]).unwrap();
    let mut e = buzz_app::editor::Editor::new(doc);
    let id = e.scene().layers().iter().next().unwrap().frames.keyframes()[0].objects[0].id;
    e.selection.toggle(id);
    let t = std::time::Instant::now();
    match a[1].as_str() {
        "quick" => {
            e.run(Command::TraceBitmap);
            e.finish_pending_trace_now();
        }
        _ => {
            let options = e.workspace.trace;
            println!("dialog options: mode {:?} colours {} drop_background {}", options.mode, options.colours, options.drop_background);
            e.confirm_trace_options(options);
            e.finish_pending_trace_now();
        }
    }
    let objects = e.scene().layers().iter().next().unwrap().frames.keyframes()[0].objects.len();
    println!("{} in {:.1} s: {objects} objects; status: {:?}", a[1], t.elapsed().as_secs_f64(), e.status);
    let scene = e.scene().clone();
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let settings = buzz_export::ExportSettings::for_stage(&scene);
    let f = ex.render(&scene, 0, &settings).unwrap();
    image::RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap().save(&a[2]).unwrap();
}
