//! Reproducible measurements with synthetic data in a newly-created directory.
use super::*;
use serde_json::json;

use diagnostics::argument;
fn phase(dir: &Path, name: &str) -> Result<()> {
    // Rename prevents the sampling script from seeing partially-written phases.
    fs::write(dir.join("phase.tmp"), name)?;
    fs::rename(dir.join("phase.tmp"), dir.join("phase.txt"))?;
    Ok(())
}
fn finish(dir: &Path, result: Result<serde_json::Value>) {
    let output = match result {
        Ok(value) => serde_json::to_vec_pretty(&value)
            .map_err(Into::into)
            .and_then(|bytes| fs::write(dir.join("timings.json"), bytes).map_err(Into::into)),
        Err(error) => Err(error),
    };
    if let Err(error) = output {
        let _ = fs::write(dir.join("benchmark-error.txt"), error.to_string());
    }
    let _ = phase(dir, "done");
    let _ = slint::quit_event_loop();
}

pub(super) fn run() -> Result<()> {
    use winit::platform::windows::WindowAttributesExtWindows;
    let records: usize = argument("--benchmark-records")?
        .unwrap_or_else(|| "100".into())
        .parse()?;
    let images: usize = argument("--benchmark-images")?
        .unwrap_or_else(|| "0".into())
        .parse()?;
    let seconds: u64 = argument("--benchmark-seconds")?
        .unwrap_or_else(|| "8".into())
        .parse()?;
    if records > 10_000 || images > records.min(256) || !(2..=60).contains(&seconds) {
        return Err("基准限制：0–10000 条记录，最多 256 张图，采样阶段 2–60 秒".into());
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = argument("--benchmark-output")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("preview-output/benchmarks")
                .join(format!("{}-{stamp}", std::process::id()))
        });
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    // Reject existing directories rather than opening any existing state.json.
    fs::create_dir(&dir)?;
    phase(&dir, "starting")?;
    let mut store = Store::load(dir.clone())?;
    let now = reminders::now();
    store.data.notes = (0..records)
        .map(|i| Note {
            text: format!("模拟记录 {i:04}：稍后查看接口反馈与测试结果。"),
            created_at: Some(now.saturating_sub((records - i) as u64 * 60)),
            ..Note::default()
        })
        .collect();
    for i in 0..images {
        let name = format!("synthetic-{i}.png");
        image::RgbaImage::from_pixel(640, 480, image::Rgba([180, (i % 256) as u8, 80, 255]))
            .save(store.dir.join("images").join(&name))?;
        store.data.notes[i].images.push(name);
    }
    let start = Instant::now();
    store.flush()?;
    let save_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    let mut store = Store::load(dir.clone())?;
    let load_ms = start.elapsed().as_secs_f64() * 1000.;
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|a| a.with_drag_and_drop(false).with_skip_taskbar(true))
        .select()?;
    let ui = PocketWindow::new()?;
    let pet = PetWindow::new()?;
    pet_shape::install(&pet)?;
    ui.set_topmost(false);
    pet.set_topmost(false);
    let settings = Rc::new(RefCell::new(tray::Preferences::default()));
    let _rail = rail::install(&ui, &pet, settings, dir.clone())?;
    _rail.set_topmost(false);
    let start = Instant::now();
    sync(&ui, &pet, &mut store);
    let model_ms = start.elapsed().as_secs_f64() * 1000.;
    let initial_thumbnail_decodes = store.thumbnail_decodes;
    let store = Rc::new(RefCell::new(store));
    pet.window().set_position(PhysicalPosition::new(900, 600));
    pet.show()?;
    pet.invoke_shape_changed();
    restore_pet_position(&pet);
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    let output = dir.clone();
    Timer::single_shot(Duration::from_millis(1500), move || {
        if let Err(e) = phase(&output, "idle") {
            finish(&output, Err(e));
            return;
        }
        let weak = weak.clone();
        let animal = animal.clone();
        let output = output.clone();
        let state = store.clone();
        Timer::single_shot(Duration::from_secs(seconds), move || {
            let result = (|| -> Result<_> {
                let ui = weak.upgrade().ok_or("基准面板已关闭")?;
                let pet = animal.upgrade().ok_or("基准小猫已关闭")?;
                phase(&output, "opening")?;
                let start = Instant::now();
                refresh_and_reveal(&ui, &pet, &state);
                let open_ms = start.elapsed().as_secs_f64() * 1000.;
                let reopen_thumbnail_decodes =
                    state.borrow().thumbnail_decodes - initial_thumbnail_decodes;
                let weak = ui.as_weak();
                let output = output.clone();
                Timer::single_shot(Duration::from_millis(600), move || {
                    let result = (|| -> Result<_> {
                        let ui = weak.upgrade().ok_or("基准面板已关闭")?;
                        let start = Instant::now();
                        let frame = ui.window().take_snapshot()?;
                        let render_ms = start.elapsed().as_secs_f64() * 1000.;
                        let frame_size = [frame.width(), frame.height()];
                        drop(frame);
                        phase(&output, "panel")?;
                        let weak = ui.as_weak();
                        let output = output.clone();
                        Timer::single_shot(Duration::from_secs(seconds), move || {
                            let result = (|| -> Result<_> {
                                let ui = weak.upgrade().ok_or("基准面板已关闭")?;
                                let pet = animal.upgrade().ok_or("基准小猫已关闭")?;
                                phase(&output, "capture")?;
                                let start = Instant::now();
                                state.borrow_mut().capture_note(Note {
                                    text: "模拟快速收下".into(),
                                    ..Note::default()
                                })?;
                                sync(&ui, &pet, &mut state.borrow_mut());
                                let capture_ms = start.elapsed().as_secs_f64() * 1000.;
                                let capture_thumbnail_decodes = state.borrow().thumbnail_decodes
                                    - initial_thumbnail_decodes
                                    - reopen_thumbnail_decodes;
                                let incoming: Vec<_> = (0..records as i32).collect();
                                let old: Vec<_> = incoming.iter().rev().copied().collect();
                                let start = Instant::now();
                                for _ in 0..200 {
                                    std::hint::black_box(rail::reconcile(
                                        &old,
                                        &incoming,
                                        old.first().copied(),
                                    ));
                                }
                                let reconcile_us =
                                    start.elapsed().as_secs_f64() * 1_000_000. / 200.;
                                Ok(
                                    json!({ "records": records, "images": images, "phase_seconds": seconds,
                                    "save_ms": save_ms, "load_ms": load_ms, "initial_model_ms": model_ms,
                                    "open_callback_ms": open_ms, "snapshot_render_ms": render_ms,
                                    "capture_save_and_model_ms": capture_ms, "rail_reconcile_us": reconcile_us,
                                    "initial_thumbnail_decodes": initial_thumbnail_decodes,
                                    "reopen_thumbnail_decodes": reopen_thumbnail_decodes,
                                    "capture_thumbnail_decodes": capture_thumbnail_decodes,
                                    "frame_size": frame_size,
                                    "scale_factor": ui.window().scale_factor(),
                                    "renderer": "software", "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                                    "data": "synthetic", "global_hotkey": false, "system_tray": false }),
                                )
                            })();
                            finish(&output, result);
                        });
                        Ok(())
                    })();
                    if let Err(e) = result {
                        finish(&output, Err(e));
                    }
                });
                Ok(())
            })();
            if let Err(e) = result {
                finish(&output, Err(e));
            }
        });
    });
    slint::run_event_loop_until_quit()?;
    if dir.join("benchmark-error.txt").exists() {
        return Err("性能基准失败，详见 benchmark-error.txt".into());
    }
    Ok(())
}
