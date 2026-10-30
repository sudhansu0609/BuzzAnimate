//! UI snapshots of the New Episode window (five steps) and the Producer panel, on
//! stubbed data -- no producer run, no model, no ComfyUI.
//!
//! episode_ui_snapshots <out dir>
use std::path::{Path, PathBuf};

use buzz_app::ui_snapshot::{SnapshotConfig, write_frame};
use buzz_ui::casting_dialog::{CastingDialogState, CastingPick, CastingRow};
use buzz_ui::episode_window::{EpisodeState, PartInfo, PlanRow, episode_window};
use buzz_ui::producer_panel::{ProducerState, Selection, producer_panel};
use serde_json::json;

fn picture(dir: &Path, name: &str, w: u32, h: u32, hue: u8) -> PathBuf {
    let path = dir.join(name);
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let t = y as f32 / h as f32;
        image::Rgba([(hue as f32 * (1.0 - t) + 40.0 * t) as u8, (90.0 + 80.0 * t) as u8, (200.0 - hue as f32 * 0.4 * t) as u8, 255])
    });
    img.save(&path).unwrap();
    path
}

fn pick(name: &str, folder: &str, thumb: Option<PathBuf>) -> CastingPick {
    CastingPick { name: name.into(), folder: folder.into(), path: format!("{folder}/{name}"), thumb, score: 0.8 }
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("output folder"));
    std::fs::create_dir_all(&out).unwrap();
    let tmp = std::env::temp_dir().join(format!("buzz-episode-snap-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = SnapshotConfig { width: 1280, height: 860, scale: 1.0 };

    let mut st = EpisodeState::default();
    st.open = true;
    st.story = r"B:\Stories\Guddu\story.md".into();
    st.out = r"B:\Episodes\Guddu_ep1".into();
    st.series = "Chirailiya".into();
    st.series_list = vec!["Chirailiya".into(), "Khayal3Baje".into()];
    st.set_screenplay(
        r"B:\Episodes\Guddu_ep1\screenplay.md",
        "## SCENE 1 \u{2014} THE TEA STALL\nINT. TEA STALL - DAY\n\nGUDDU (V.O.): Mere gaon mein ek chai ki dukaan thi.\n\nTHE OWNER: Beta, aaj der kar di?\n\nGUDDU: Haan chacha, bike kharab ho gayi thi.\n\n## SCENE 2 \u{2014} THE ROAD\nEXT. ROAD - EVENING\n\nGUDDU points down the road.\n".into(),
        vec!["Guddu (guddu), narrator".into(), "The Owner (owner), tea stall chacha".into(), "Mother (mother)".into()],
    );
    st.status = Some("Screenplay ready: 3 cast member(s). Edit it if you like, then Next.".into());

    let mut casting = CastingDialogState::default();
    casting.series = st.series.clone();
    casting.out = st.out.clone();
    casting.status = Some("3 people; 41 of your characters".into());
    let t = |n: &str, h: u8| Some(picture(&tmp, n, 160, 160, h));
    casting.rows = vec![
        CastingRow {
            key: "guddu".into(),
            member: "Guddu".into(),
            role: "narrator".into(),
            chosen: Some(pick("Boy2Symbol", "Boys", t("boy.png", 200))),
            reason: "A teenage boy in a casual shirt; the name and age match.".into(),
            locked: true,
            readiness: "Boy2Symbol: mouth ok, arms ok".into(),
            alternatives: vec![pick("Boy5Symbol", "Boys", t("b5.png", 120)), pick("Man1Symbol", "Men", t("m1.png", 60)), pick("Boy7Symbol", "Boys", t("b7.png", 160))],
            ..Default::default()
        },
        CastingRow {
            key: "owner".into(),
            member: "The Owner".into(),
            role: "tea stall chacha".into(),
            chosen: Some(pick("OldMan3Symbol", "Men", t("o3.png", 90))),
            reason: "An older man with a moustache: a chacha at a stall.".into(),
            suggested: true,
            readiness: "OldMan3Symbol: no mouth: run AI Layer Rig".into(),
            alternatives: vec![pick("Man4Symbol", "Men", t("m4.png", 30)), pick("OldMan1Symbol", "Men", t("o1.png", 140))],
            ..Default::default()
        },
        CastingRow { key: "mother".into(), member: "Mother".into(), reason: "Nobody of yours fits yet.".into(), ..Default::default() },
    ];
    st.story_dir = r"B:\Stories\Guddu".into();

    let shot = |st: &mut EpisodeState, casting: &mut CastingDialogState, name: &str| {
        write_frame(cfg, &out.join(name), |ui| {
            let ctx = ui.ctx().clone();
            ctx.all_styles_mut(|s| s.animation_time = 0.0);
            episode_window(&ctx, st, casting);
        })
        .unwrap();
        println!("wrote {name}");
    };

    st.step = 0;
    shot(&mut st, &mut casting, "1_story.png");
    st.step = 1;
    shot(&mut st, &mut casting, "2_cast.png");

    st.step = 2;
    st.audio_parts = r"B:\Stories\Guddu\audio".into();
    st.audio_full = r"B:\Stories\Guddu\guddu_full.wav".into();
    st.parts = (1..=4).map(|n| PartInfo { n, file: format!("part{n}.wav"), seconds: 170.0 + n as f64 * 7.0 }).collect();
    st.plan = vec![
        PlanRow { n: 1, file: "part1.wav".into(), seconds: 177.0, scenes: "1, 2".into() },
        PlanRow { n: 2, file: "part2.wav".into(), seconds: 184.0, scenes: "3".into() },
        PlanRow { n: 3, file: "part3.wav".into(), seconds: 191.0, scenes: "4, 5".into() },
        PlanRow { n: 4, file: "part4.wav".into(), seconds: 198.0, scenes: "6".into() },
    ];
    st.uncovered = vec![41, 42];
    shot(&mut st, &mut casting, "3_audio.png");

    st.step = 3;
    // The Look step reads the real profile of a series that has one.
    st.series = "Khayal3Baje".into();
    (st.pictures, st.picture_flows) = buzz_app::comfy_profile::pictures_line(&st.series);
    st.world = "a semi-rural small town in North India, 2010s: smartphones, motorbikes, small shops and tea stalls of that decade".into();
    st.style = 1;
    st.captions = false;
    st.objects = true;
    st.angles = true;
    shot(&mut st, &mut casting, "4_look.png");
    st.series = "Chirailiya".into();

    st.step = 4;
    st.whole = false;
    st.part = 2;
    st.busy = true;
    st.running_step = Some(3);
    st.detail = "scene 3 of 6: the tea stall, wide".into();
    st.status = Some("Producing: you can keep working. Cancel stops it between steps.".into());
    shot(&mut st, &mut casting, "5_produce_running.png");

    st.busy = false;
    st.finished = true;
    st.running_step = None;
    st.stills = (0..6).map(|k| picture(&tmp, &format!("still{k}.png"), 320, 180, 40 + k * 30)).collect();
    st.status = Some("Produced B:\\Episodes\\Guddu_ep1\\episode.buzz (14 cut(s), 38 line(s) from the recording)".into());
    shot(&mut st, &mut casting, "5_produce_done.png");

    // The Producer panel on a small fake episode.
    let bg = |n: &str, h: u8| picture(&tmp, n, 320, 180, h).display().to_string();
    let still1 = picture(&tmp, "scene1_still.png", 640, 360, 170).display().to_string();
    let still2 = picture(&tmp, "scene2_still.png", 640, 360, 100).display().to_string();
    let view = json!({
        "cameras": ["wide", "medium", "close", "reverse", "over_the_shoulder", "low", "high"],
        "speakers": ["Guddu", "The Owner", "Mother"],
        "overrides": ["scene 2: camera of shot 2 -> close", "recast Mother -> Woman3Symbol (scene 2)"],
        "scenes": [
            {"number": 1, "location": "tea stall", "dirty": false, "still": still1,
             "shots": [{"n": 1, "camera": "wide", "at_line": 1, "background_key": "tea stall", "background_png": bg("bg1.png", 200), "prompt": "a roadside tea stall in a small North Indian town, morning light", "tries": []}],
             "lines": [
                {"item": 1, "speaker": "Guddu", "text": "Mere gaon mein ek chai ki dukaan thi.", "start": 0.4, "end": 3.1, "pose": "", "poses": ["standing (default)", "sitting", "pointing", "Sit"]},
                {"item": 2, "speaker": "The Owner", "text": "Beta, aaj der kar di?", "start": 3.6, "end": 5.0, "pose": "shrug"}]},
            {"number": 2, "location": "road", "dirty": true, "still": still2,
             "shots": [
                {"n": 1, "camera": "wide", "at_line": 1, "background_key": "road", "background_png": bg("bg2.png", 120), "prompt": "a dusty village road at evening", "tries": []},
                {"n": 2, "camera": "reverse", "at_line": 3, "background_key": "road__reverse", "background_png": bg("bg3.png", 60), "prompt": "", "tries": [bg("try1.png", 90), bg("try2.png", 150)]}],
             "lines": [
                {"item": 1, "speaker": "Guddu", "text": "Haan chacha, bike kharab ho gayi thi.", "start": null, "end": null, "pose": "", "poses": ["standing (default)", "sitting", "scared", "Sit"]},
                {"item": 2, "speaker": "Mother", "text": "Ghar jaldi aana.", "start": 9.2, "end": 10.4, "pose": ""}]}
        ]
    });
    let mut panel = ProducerState::default();
    panel.open = true;
    panel.out = r"B:\Episodes\Guddu_ep1".into();
    panel.set_view(view.clone());
    panel.catalogue = [("Boy2Symbol", "Boys", 200u8), ("Boy5Symbol", "Boys", 120), ("Man1Symbol", "Men", 60), ("OldMan3Symbol", "Men", 90), ("Woman3Symbol", "Women", 150), ("Girl1Symbol", "Girls", 30), ("Man4Symbol", "Men", 20), ("Woman1Symbol", "Women", 240)]
        .iter()
        .map(|(n, f, h)| pick(n, f, Some(picture(&tmp, &format!("cat_{n}.png"), 160, 160, *h))))
        .collect();
    panel.status = Some("Loaded B:\\Episodes\\Guddu_ep1".into());
    panel.last_change = Some("Camera of scene 2, shot 2 is now close. Scene(s) [2] rebuilt; the rest is untouched.".into());
    let pcfg = SnapshotConfig { width: 1280, height: 780, scale: 1.0 };
    panel.preset_move(2, "Guddu", 0.62, 0.86);
    for (name, sel) in [
        ("6_producer_shot.png", Selection::Shot(2, 2)),
        ("7_producer_line.png", Selection::Line(2, 1)),
        ("8_producer_scene.png", Selection::Scene(2)),
        ("9_producer_move.png", Selection::Scene(2)),
        ("10_producer_recast.png", Selection::Scene(2)),
    ] {
        panel.selected = sel;
        panel.set_view(view.clone());
        panel.detail_scroll = if name.starts_with("9_") { Some(250.0) } else if name.starts_with("10_") { Some(480.0) } else { Some(0.0) };
        if name.starts_with("10_") {
            panel.preset_recast(2, "Mother", "Woman3Symbol");
        }
        write_frame(pcfg, &out.join(name), |ui| {
            let ctx = ui.ctx().clone();
            ctx.all_styles_mut(|s| s.animation_time = 0.0);
            producer_panel(&ctx, &mut panel);
        })
        .unwrap();
        println!("wrote {name}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
