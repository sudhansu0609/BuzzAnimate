//! Throwaway P-review probe: N exports of one document through the real
//! `ExportQueue` + `run_export` threads in one process, the way `App` pumps it.
//! `cargo run --release -p buzz-app --example zz_parallel_export -- doc.buzz out_dir N`
use buzz_app::export_service::{run_export, ExportQueue, ExportRequest, ExportTarget};
use buzz_app::tasks::{TaskKind, TaskOutcome, TaskRegistry};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (doc, out, n): (&str, &str, usize) = (&a[1], &a[2], a[3].parse().unwrap());
    let loaded = buzz_doc::format::load_scenes_full(std::path::Path::new(doc)).unwrap();
    let scenes = buzz_doc::Document::from_loaded_scenes(loaded).film_with_lengths();
    let frames = buzz_export::Reel::of_lengths(scenes.iter().map(|(s, l)| (s, *l))).frames();
    let mut settings = buzz_export::ExportSettings::for_stage(&scenes[0].0);
    let aspect = settings.width as f64 / settings.height as f64;
    settings.height = 540;
    settings.width = ((540.0 * aspect).round() as u32 + 1) & !1;

    let mut queue = ExportQueue::default();
    queue.set_max_parallel(n.min(buzz_app::export_service::vram_cap()));
    for i in 0..n {
        queue.enqueue(ExportRequest {
            scenes: scenes.clone(),
            settings: settings.clone(),
            range: 0..frames,
            target: ExportTarget::Video {
                path: std::path::PathBuf::from(format!("{out}/inproc_n{n}_{i}.mp4")),
                video: buzz_export::VideoSettings::default(),
            },
            gpu: buzz_render::GpuPreference::default(),
            label: format!("job {i}"),
        });
    }
    let mut tasks = TaskRegistry::default();
    let t0 = std::time::Instant::now();
    let mut peak = 0;
    while !queue.is_idle() {
        while let Some(req) = queue.next_to_start() {
            let reveal = req.reveal_path();
            let label = req.label.clone();
            let id = tasks.spawn_thread(TaskKind::Export, label.clone(), move |ctx| run_export(req, &ctx));
            queue.started(id, reveal, label);
        }
        peak = peak.max(queue.outstanding() - queue.waiting());
        for (id, _, outcome) in tasks.poll() {
            let ok = matches!(outcome, TaskOutcome::Finished(_));
            println!("{:>6.1}s done ok={ok}", t0.elapsed().as_secs_f64());
            queue.complete(id, ok, String::new());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    println!("n={n} cap_vram={} peak_running={peak} wall={:.1}s", buzz_app::export_service::vram_cap(), t0.elapsed().as_secs_f64());
}
