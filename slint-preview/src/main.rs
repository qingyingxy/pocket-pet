#![windows_subsystem = "windows"]
use serde::{Deserialize, Serialize};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Image, ModelRc, PhysicalPosition, Timer, TimerMode, VecModel};
use std::os::windows::ffi::OsStrExt;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
slint::include_modules!();
mod attention;
mod clipboard;
mod drop_import;
mod drop_target;
mod hotkey;
mod reminders;
mod snap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Attachment {
    name: String,
    path: String,
}
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
struct Note {
    text: String,
    images: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<Attachment>,
    done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remind_at: Option<u64>,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Data {
    notes: Vec<Note>,
    draft: Note,
}
struct Store {
    data: Data,
    buffer: Note,
    selected: Option<usize>,
    dir: PathBuf,
    cache: HashMap<String, Image>,
    undo: Option<CaptureUndo>,
    completion: Option<CompletionUndo>,
    snap_rects: HashMap<usize, [f32; 6]>,
    snaps: HashMap<usize, SnapVisual>,
    finishing: HashMap<usize, (Note, bool, Instant)>,
}
struct SnapVisual {
    backdrop: Image,
    layers: ModelRc<DustLayer>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    running: bool,
    bytes: usize,
    prepare_ms: f64,
}
struct CompletionUndo {
    index: usize,
    before: Note,
    after: Note,
    deadline: Instant,
}
struct CaptureUndo {
    index: usize,
    note: Note,
    deadline: Instant,
}
const UNDO_DURATION: Duration = Duration::from_secs(6);
impl Store {
    fn load(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("images"))?;
        let file = dir.join("state.json");
        let data = if file.exists() {
            serde_json::from_slice(&fs::read(file)?)?
        } else {
            Data::default()
        };
        Ok(Self {
            buffer: data.draft.clone(),
            data,
            selected: None,
            dir,
            cache: HashMap::new(),
            undo: None,
            completion: None,
            snap_rects: HashMap::new(),
            snaps: HashMap::new(),
            finishing: HashMap::new(),
        })
    }
    fn flush(&mut self) -> Result<()> {
        if let Some(i) = self.selected {
            self.data.notes[i] = self.buffer.clone();
        } else {
            self.data.draft = self.buffer.clone();
        }
        save_data(&self.dir, &self.data)
    }
    fn capture_note(&mut self, note: Note) -> Result<()> {
        if note.text.trim().is_empty() && note.images.is_empty() && note.files.is_empty() {
            return Err("没有可收下的内容".into());
        }
        let before = self.data.clone();
        let index = self.data.notes.len();
        self.data.notes.push(note.clone());
        if let Err(error) = self.flush() {
            self.data = before;
            return Err(error);
        }
        self.undo = Some(CaptureUndo {
            index,
            note,
            deadline: Instant::now() + UNDO_DURATION,
        });
        Ok(())
    }
    fn undo_capture(&mut self) -> Result<()> {
        let undo = self.undo.as_ref().ok_or("没有可撤销的记录")?;
        if Instant::now() >= undo.deadline {
            self.undo = None;
            return Err("撤销时间已过，记录仍在待办中".into());
        }
        if self.selected == Some(undo.index) || self.data.notes.get(undo.index) != Some(&undo.note)
        {
            self.undo = None;
            return Err("记录已修改，已保留".into());
        }
        let index = undo.index;
        let before = self.data.clone();
        let old_selected = self.selected;
        self.data.notes.remove(index);
        if let Some(selected) = self.selected {
            if selected > index {
                self.selected = Some(selected - 1);
            }
        }
        if let Err(error) = self.flush() {
            self.data = before;
            self.selected = old_selected;
            return Err(error);
        }
        self.undo = None;
        Ok(())
    }
    fn toggle_note(&mut self, index: usize) -> Result<()> {
        if index >= self.data.notes.len() {
            return Err("记录不存在".into());
        }
        let before = self.data.clone();
        let buffer = self.buffer.clone();
        let note = &mut self.data.notes[index];
        note.done = !note.done;
        if note.done {
            note.remind_at = None;
        }
        let done = note.done;
        let at = note.remind_at;
        if self.selected == Some(index) {
            self.buffer.done = done;
            self.buffer.remind_at = at;
        }
        if let Err(error) = self.flush() {
            self.data = before;
            self.buffer = buffer;
            return Err(error);
        }
        if self.undo.as_ref().is_some_and(|u| u.index == index) {
            self.undo = None;
        }
        Ok(())
    }
    fn set_reminder(&mut self, index: usize, at: Option<u64>) -> Result<()> {
        if index >= self.data.notes.len() {
            return Err("记录不存在".into());
        }
        if self.data.notes[index].done && at.is_some() {
            return Err("已完成的记录无需提醒".into());
        }
        let before = self.data.clone();
        let buffer = self.buffer.clone();
        self.data.notes[index].remind_at = at;
        if self.selected == Some(index) {
            self.buffer.remind_at = at;
        }
        if let Err(error) = self.flush() {
            self.data = before;
            self.buffer = buffer;
            return Err(error);
        }
        if self.undo.as_ref().is_some_and(|u| u.index == index) {
            self.undo = None;
        }
        Ok(())
    }
    fn update_record(&mut self, index: usize, update: impl FnOnce(&mut Note)) -> Result<()> {
        let old = self.data.notes.get(index).ok_or("记录不存在")?.clone();
        update(&mut self.data.notes[index]);
        if let Err(e) = save_data(&self.dir, &self.data) {
            self.data.notes[index] = old;
            return Err(e);
        }
        Ok(())
    }
    fn complete(&mut self, index: usize) -> Result<()> {
        let before = self.data.notes.get(index).ok_or("记录不存在")?.clone();
        if before.done || self.finishing.contains_key(&index) {
            return Err("这条记录已经完成".into());
        }
        self.toggle_note(index)?;
        self.completion = Some(CompletionUndo {
            index,
            before: before.clone(),
            after: self.data.notes[index].clone(),
            deadline: Instant::now() + UNDO_DURATION,
        });
        self.undo = None;
        self.finishing.insert(
            index,
            (before, false, self.completion.as_ref().unwrap().deadline),
        );
        Ok(())
    }
    fn undo_completion(&mut self) -> Result<()> {
        let undo = self.completion.as_ref().ok_or("没有可撤销的完成操作")?;
        if Instant::now() >= undo.deadline {
            return Err("撤销时间已过，可在已完成中恢复".into());
        }
        if self.data.notes.get(undo.index) != Some(&undo.after) {
            return Err("记录已修改，已保留".into());
        }
        let index = undo.index;
        let before = undo.before.clone();
        self.update_record(index, |n| *n = before)?;
        self.finishing.remove(&index);
        self.snaps.remove(&index);
        self.snap_rects.remove(&index);
        self.completion = None;
        Ok(())
    }
    fn thumbnail(&mut self, name: &str) -> Image {
        if let Some(image) = self.cache.get(name) {
            return image.clone();
        }
        let result = (|| -> Result<Image> {
            let mut reader = image::ImageReader::open(self.dir.join("images").join(name))?
                .with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(64 * 1024 * 1024);
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            reader.limits(limits);
            let image = reader.decode()?.thumbnail(240, 160).to_rgba8();
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                image.as_raw(),
                image.width(),
                image.height(),
            );
            Ok(Image::from_rgba8(buffer))
        })()
        .unwrap_or_default();
        if self.cache.len() >= 48 {
            self.cache.clear();
        }
        self.cache.insert(name.into(), result.clone());
        result
    }
    fn clipboard_image(&self) -> Result<Option<Note>> {
        if let Some(image) = clipboard::read_image()? {
            let name = format!(
                "{}.png",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            );
            image::save_buffer(
                self.dir.join("images").join(&name),
                image.as_raw(),
                image.width(),
                image.height(),
                image::ColorType::Rgba8,
            )?;
            return Ok(Some(Note {
                text: String::new(),
                images: vec![name],
                done: false,
                remind_at: None,
                files: vec![],
            }));
        }
        Ok(None)
    }
    fn clipboard(&self) -> Result<Note> {
        if let Some(note) = self.clipboard_image()? {
            return Ok(note);
        }
        let mut clipboard = arboard::Clipboard::new()?;
        let text = clipboard.get_text()?;
        if text.trim().is_empty() {
            return Err("剪贴板是空的".into());
        }
        Ok(Note {
            text,
            images: vec![],
            done: false,
            remind_at: None,
            files: vec![],
        })
    }
}
fn save_data(dir: &Path, data: &Data) -> Result<()> {
    fs::write(dir.join("state.tmp"), serde_json::to_vec_pretty(data)?)?;
    if dir.join("state.json").exists() {
        fs::copy(dir.join("state.json"), dir.join("state.backup.json"))?;
    }
    fs::rename(dir.join("state.tmp"), dir.join("state.json"))?;
    Ok(())
}
fn sync(ui: &PocketWindow, pet: &PetWindow, store: &mut Store) {
    let now = reminders::now();
    ui.set_reminder_label(reminders::label(store.buffer.remind_at, now).into());
    ui.set_later_label(
        if chrono::Local::now().time() < chrono::NaiveTime::from_hms_opt(21, 0, 0).unwrap() {
            "今天晚点"
        } else {
            "明早 9 点"
        }
        .into(),
    );
    ui.set_draft(store.buffer.text.clone().into());
    ui.set_editing(store.selected.is_some());
    ui.set_has_attachment(!store.buffer.images.is_empty() || !store.buffer.files.is_empty());
    ui.set_file_count(store.buffer.files.len() as i32);
    ui.set_attachment_count(store.buffer.images.len() as i32);
    ui.set_attachment(
        store
            .buffer
            .images
            .first()
            .cloned()
            .map(|n| store.thumbnail(&n))
            .unwrap_or_default(),
    );
    let count = store.data.notes.iter().filter(|n| !n.done).count() as i32;
    ui.set_pending_count(count);
    pet.set_count(count);
    let old_due = pet.get_due_count();
    pet.set_due_count(
        store
            .data
            .notes
            .iter()
            .filter(|n| reminders::is_due(n.remind_at, n.done, now))
            .count() as i32,
    );
    if pet.get_due_count() > old_due && !ui.get_revealed() && !pet.get_drop_busy() {
        let label = store
            .data
            .notes
            .iter()
            .find(|n| reminders::is_due(n.remind_at, n.done, now))
            .map(|n| {
                if n.text.is_empty() {
                    "查看这条记录".to_string()
                } else {
                    n.text.chars().take(8).collect()
                }
            })
            .unwrap_or_default();
        nudge(pet, format!("提醒：{label}"));
    }
    let mut cards = vec![];
    for i in (0..store.data.notes.len()).rev() {
        let n = store
            .finishing
            .get(&i)
            .map(|(n, _, _)| n)
            .unwrap_or(&store.data.notes[i])
            .clone();
        if n.done && !ui.get_show_done() {
            continue;
        }
        let preview = n
            .images
            .first()
            .map(|name| store.thumbnail(name))
            .unwrap_or_default();
        cards.push(CardData {
            id: i as i32,
            snap_ready: store.snaps.contains_key(&i),
            snap_running: store.snaps.get(&i).is_some_and(|s| s.running),
            snap_layers: store
                .snaps
                .get(&i)
                .map(|s| s.layers.clone())
                .unwrap_or_default(),
            snap_backdrop: store
                .snaps
                .get(&i)
                .map(|s| s.backdrop.clone())
                .unwrap_or_default(),
            snap_x: store.snaps.get(&i).map_or(0., |s| s.x),
            snap_y: store.snaps.get(&i).map_or(0., |s| s.y),
            snap_width: store.snaps.get(&i).map_or(0., |s| s.width),
            snap_height: store.snaps.get(&i).map_or(0., |s| s.height),
            frozen_height: store.snap_rects.get(&i).map_or(0., |r| r[3]),
            dissolving: store.finishing.contains_key(&i),
            collapsing: store
                .finishing
                .get(&i)
                .is_some_and(|(_, collapse, _)| *collapse),
            body: if n.text.is_empty() && !n.files.is_empty() {
                n.files
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
                    .into()
            } else {
                n.text.into()
            },
            preview,
            has_image: !n.images.is_empty(),
            image_count: n.images.len() as i32,
            file_count: n.files.len() as i32,
            done: n.done,
            due: reminders::is_due(n.remind_at, n.done, now),
            reminder: if n.done || n.remind_at.is_none() {
                "".into()
            } else {
                reminders::label(n.remind_at, now).into()
            },
        });
    }
    cards.sort_by_key(|n| (n.done, !n.due));
    use slint::Model;
    let current = ui.get_cards();
    if let Some(model) = current.as_any().downcast_ref::<VecModel<CardData>>() {
        for (row, card) in cards.iter().enumerate() {
            if !model.row_data(row).is_some_and(|old| old.id == card.id) {
                if let Some(old_row) = (row..model.row_count())
                    .find(|&r| model.row_data(r).is_some_and(|old| old.id == card.id))
                {
                    model.remove(old_row);
                }
                model.insert(row, card.clone());
            } else {
                model.set_row_data(row, card.clone());
            }
        }
        while model.row_count() > cards.len() {
            model.remove(model.row_count() - 1);
        }
    } else {
        ui.set_cards(ModelRc::new(VecModel::from(cards)));
    }
    ui.set_completion_undo(store.completion.as_ref().is_some_and(|u| {
        Instant::now() < u.deadline && store.data.notes.get(u.index) == Some(&u.after)
    }));
    let next = reminders::next_delay(
        store
            .data
            .notes
            .iter()
            .filter(|n| !n.done)
            .filter_map(|n| n.remind_at),
        now,
    );
    ui.invoke_schedule_reminders(next.map(|d| d.as_secs_f32()).unwrap_or(-1.0));
}
fn startup_position(work: RECT, width: i32, height: i32, scale: f32) -> PhysicalPosition {
    let margin = (32.0 * scale).round() as i32;
    // Leave room for the wider bubble to remain centred above the pet.
    let bubble_overhang = (((448.0 * scale).round() as i32 - width) / 2).max(0);
    PhysicalPosition::new(
        (work.right - width - margin - bubble_overhang).max(work.left),
        (work.bottom - height - margin).max(work.top),
    )
}

fn place_at_startup(pet: &PetWindow) {
    let p = pet.window().position();
    unsafe {
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(
            MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0
        {
            let size = pet.window().size();
            pet.window().set_position(startup_position(
                monitor.rcWork,
                size.width as i32,
                size.height as i32,
                pet.window().scale_factor(),
            ));
        }
    }
}

fn place(ui: &PocketWindow, pet: &PetWindow) {
    let p = pet.window().position();
    let scale = pet.window().scale_factor();
    let w = (448.0 * scale) as i32;
    let h = (ui.get_panel_height() * scale).round() as i32;
    unsafe {
        let mut m: MONITORINFO = std::mem::zeroed();
        m.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(
            MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST),
            &mut m,
        ) == 0
        {
            return;
        }
        let r = m.rcWork;
        let x = (p.x + (pet.window().size().width as i32 - w) / 2)
            .clamp(r.left, (r.right - w).max(r.left));
        let y = if p.y - h >= r.top {
            p.y - h - 4
        } else {
            p.y + pet.window().size().height as i32 + 4
        };
        ui.window().set_position(PhysicalPosition::new(
            x,
            y.clamp(r.top, (r.bottom - h).max(r.top)),
        ));
    }
}
fn rgba_image(pixels: &image::RgbaImage) -> Image {
    Image::from_rgba8(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
        ),
    )
}
fn capture_snap(ui: &PocketWindow, rect: [f32; 6], seed: u32) -> Result<SnapVisual> {
    let started = Instant::now();
    let scale = ui.window().scale_factor();
    ui.set_capturing(true);
    let frame_result = ui.window().take_snapshot();
    ui.set_capturing(false);
    let frame = frame_result?;
    let [x, y, w, h, clip_top, clip_bottom] = rect;
    if !rect.iter().all(|v| v.is_finite()) || w < 1. || h < 1. || scale <= 0. {
        return Err("卡片坐标无效".into());
    }
    let left = (x * scale).round().clamp(0., frame.width() as f32) as u32;
    let top = (y.max(clip_top) * scale)
        .round()
        .clamp(0., frame.height() as f32) as u32;
    let right = ((x + w) * scale).round().clamp(0., frame.width() as f32) as u32;
    let bottom = ((y + h).min(clip_bottom) * scale)
        .round()
        .clamp(0., frame.height() as f32) as u32;
    if right <= left || bottom <= top {
        return Err("卡片已不在可见区域".into());
    }
    let mut crop = image::RgbaImage::new(right - left, bottom - top);
    for py in 0..crop.height() {
        for px in 0..crop.width() {
            let local_x = (left + px) as f32 / scale - x;
            let local_y = (top + py) as f32 / scale - y;
            let radius = 16f32.min(w / 2.).min(h / 2.);
            let dx = local_x - local_x.clamp(radius, w - radius);
            let dy = local_y - local_y.clamp(radius, h - radius);
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let offset =
                (((top + py) as usize * frame.width() as usize) + (left + px) as usize) * 4;
            crop.put_pixel(
                px,
                py,
                image::Rgba(frame.as_bytes()[offset..offset + 4].try_into()?),
            );
        }
    }
    let width = crop.width() as f32 / scale;
    let height = crop.height() as f32 / scale;
    let split = snap::split(crop, seed);
    let bytes = split.backdrop.as_raw().len() * (snap::LAYERS + 1);
    let backdrop = rgba_image(&split.backdrop);
    let layers = split
        .layers
        .into_iter()
        .enumerate()
        .map(|(i, pixels)| DustLayer {
            picture: rgba_image(&pixels),
            delay: i as f32 / (snap::LAYERS - 1) as f32 * 0.36,
            drift_x: 38. + ((i * 17 + 7) % 43) as f32,
            drift_y: -18. - ((i * 13 + 3) % 35) as f32,
            swirl: ((i * 11 % 15) as f32) - 7.,
        })
        .collect::<Vec<_>>();
    Ok(SnapVisual {
        backdrop,
        layers: ModelRc::new(VecModel::from(layers)),
        x: left as f32 / scale - x,
        y: top as f32 / scale - y,
        width,
        height,
        running: false,
        bytes,
        prepare_ms: started.elapsed().as_secs_f64() * 1000.,
    })
}
fn finish_snap_later(
    ui: &PocketWindow,
    pet: &PetWindow,
    state: Rc<RefCell<Store>>,
    i: usize,
    deadline: Instant,
) {
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    let phase_state = state.clone();
    Timer::single_shot(Duration::from_millis(1160), move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let mut s = phase_state.borrow_mut();
        if let Some((_, collapse, token)) = s.finishing.get_mut(&i) {
            if *token == deadline {
                *collapse = true;
            }
        }
        sync(&ui, &pet, &mut s);
    });
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    Timer::single_shot(Duration::from_millis(1380), move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let mut s = state.borrow_mut();
        if s.finishing
            .get(&i)
            .is_some_and(|(_, _, token)| *token == deadline)
        {
            s.finishing.remove(&i);
            s.snaps.remove(&i);
            s.snap_rects.remove(&i);
            if ui.get_expanded_id() == i as i32 {
                ui.set_expanded_id(-1);
            }
        }
        sync(&ui, &pet, &mut s);
        place(&ui, &pet);
        pet.set_happy(false);
        pet.set_celebrating(false);
    });
}
fn nudge(pet: &PetWindow, message: String) {
    let token = pet.get_nudge_token().wrapping_add(1);
    pet.set_nudge_token(token);
    pet.set_nudge_text(message.into());
    pet.set_nudging(true);
    let weak = pet.as_weak();
    Timer::single_shot(Duration::from_millis(450), move || {
        if let Some(p) = weak.upgrade() {
            if p.get_nudge_token() == token {
                p.set_nudging(false);
            }
        }
    });
    let weak = pet.as_weak();
    Timer::single_shot(Duration::from_secs(7), move || {
        if let Some(p) = weak.upgrade() {
            if p.get_nudge_token() == token {
                p.set_nudge_text("".into());
                p.set_nudging(false);
            }
        }
    });
}
fn reveal(ui: &PocketWindow, pet: &PetWindow) {
    let _ = ui.show();
    place(ui, pet);
    ui.set_reminder_options(false);
    ui.set_revealed(true);
    ui.invoke_viewed();
    if !ui.get_task_view() {
        ui.set_focus_token(ui.get_focus_token() + 1);
    }
    ui.window()
        .with_winit_window(|window| window.focus_window());
}
fn hide(ui: &PocketWindow) {
    ui.set_revealed(false);
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_millis(170), move || {
        if let Some(ui) = weak.upgrade() {
            if !ui.get_revealed() {
                let _ = ui.hide();
            }
        }
    });
}
fn report(ui: &PocketWindow, error: impl std::fmt::Display) {
    ui.set_notice(format!("未保存：{error}").into());
}
fn toast(pet: &PetWindow, timer: &Timer, text: &str, undo: bool, error: bool) {
    pet.set_toast(text.into());
    pet.set_can_undo(undo);
    pet.set_toast_error(error);
    let weak = pet.as_weak();
    timer.start(TimerMode::SingleShot, UNDO_DURATION, move || {
        if let Some(pet) = weak.upgrade() {
            pet.set_toast("".into());
            pet.set_can_undo(false);
            pet.set_toast_error(false);
        }
    });
}
async fn create_drop_target(
    ui: &PocketWindow,
    pet: &PetWindow,
    store: &Rc<RefCell<Store>>,
    import_result: &std::sync::Arc<std::sync::Mutex<Option<std::result::Result<Note, String>>>>,
    toast_timer: &Rc<Timer>,
) -> Result<Option<drop_target::Registration>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let native = pet.window().winit_window().await?;
    let hwnd = match native.window_handle()?.as_raw() {
        RawWindowHandle::Win32(h) => windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _),
        _ => return Err("无法取得小猫窗口句柄".into()),
    };
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    let result = import_result.clone();
    let dir = store.borrow().dir.clone();
    let timer = toast_timer.clone();
    Ok(
        match drop_target::Registration::new(hwnd, move |event| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return false;
            };
            match event {
                drop_target::Event::Hover(active) => {
                    pet.set_drop_hover(active && !pet.get_drop_busy());
                    !pet.get_drop_busy()
                }
                drop_target::Event::Dropped(payload) => {
                    if pet.get_drop_busy() {
                        return false;
                    }
                    let payload = match payload {
                        Ok(payload) => payload,
                        Err(error) => {
                            ui.set_notice(format!("未收下：{error}").into());
                            toast(&pet, &timer, "未收下 · 点击查看", false, true);
                            return false;
                        }
                    };
                    pet.set_drop_busy(true);
                    ui.set_drop_busy(true);
                    let result = result.clone();
                    let dir = dir.clone();
                    let weak = weak.clone();
                    let spawn = std::thread::Builder::new()
                        .name("drop-import".into())
                        .spawn(move || {
                            let imported = drop_import::import(payload, &dir);
                            *result.lock().unwrap() = Some(imported);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = weak.upgrade() {
                                    ui.invoke_drop_ready();
                                }
                            });
                        });
                    if let Err(error) = spawn {
                        pet.set_drop_busy(false);
                        ui.set_drop_busy(false);
                        ui.set_notice(format!("无法开始收下：{error}").into());
                        toast(&pet, &timer, "未收下 · 点击查看", false, true);
                        return false;
                    }
                    true
                }
            }
        }) {
            Ok(registration) => Some(registration),
            Err(error) => {
                ui.set_notice(format!("拖放不可用：{error}").into());
                toast(&pet, &toast_timer, "拖放不可用 · 查看", false, true);
                None
            }
        },
    )
}
fn run() -> Result<()> {
    use winit::platform::windows::WindowAttributesExtWindows;
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attributes| attributes.with_drag_and_drop(false))
        .select()?;
    let snapshot = std::env::args().any(|a| a == "--snapshot");
    let dir = if snapshot {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output")
    } else {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA 不可用")?)
            .join("PocketPetSlintPreview")
    };
    let mut data = Store::load(dir)?;
    if snapshot {
        data.data = Data::default();
        data.data.notes = vec![
            Note {
                text: "回复小王的接口问题".into(),
                ..Note::default()
            },
            Note {
                text: "看一下登录页 PR\ngithub.com/team/pull/42".into(),
                ..Note::default()
            },
            Note {
                text: "整理测试结果，发给同事".into(),
                ..Note::default()
            },
        ];
        data.buffer.text = "下午帮小王看一下登录接口，\n顺便确认超时重试逻辑。".into();
    }
    let store = Rc::new(RefCell::new(data));
    let ui = PocketWindow::new()?;
    let pet = PetWindow::new()?;
    sync(&ui, &pet, &mut store.borrow_mut());
    let reminder_timer = Rc::new(Timer::default());
    {
        let timer = reminder_timer.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let state = store.clone();
        ui.on_schedule_reminders(move |seconds| {
            if seconds < 0.0 {
                timer.stop();
                return;
            }
            let weak = weak.clone();
            let animal = animal.clone();
            let state = state.clone();
            timer.start(
                TimerMode::SingleShot,
                Duration::from_secs_f32(seconds.max(1.0)),
                move || {
                    if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                        sync(&ui, &pet, &mut state.borrow_mut());
                    }
                },
            );
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let state = store.clone();
        ui.on_choose_reminder(move |preset| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            if s.buffer.done && preset != 0 {
                ui.set_notice("已完成的记录无需提醒".into());
                return;
            }
            let at = match preset {
                0 => None,
                1 => Some(reminders::now() + reminders::HALF_HOUR),
                2 => match reminders::later_today(chrono::Local::now()) {
                    Some(at) => Some(at),
                    None => {
                        ui.set_notice("无法计算提醒时间".into());
                        return;
                    }
                },
                _ => return,
            };
            let old = s.buffer.clone();
            let before = s.data.clone();
            s.buffer.remind_at = at;
            if let Err(error) = s.flush() {
                s.buffer = old;
                s.data = before;
                report(&ui, error);
                return;
            }
            ui.set_reminder_options(false);
            ui.set_notice("提醒设置已保存".into());
            sync(&ui, &pet, &mut s);
            place(&ui, &pet);
        });
    }
    for snooze in [false, true] {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let state = store.clone();
        let action = move |id: i32| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            let at = snooze.then(|| reminders::now() + reminders::HALF_HOUR);
            if let Err(error) = s.set_reminder(id as usize, at) {
                report(&ui, error);
                return;
            }
            ui.set_notice(
                if snooze {
                    "30 分钟后再提醒"
                } else {
                    "已知晓，事项仍保留在待办中"
                }
                .into(),
            );
            sync(&ui, &pet, &mut s);
        };
        if snooze {
            ui.on_snooze(action);
        } else {
            ui.on_acknowledge(action);
        }
    }
    sync(&ui, &pet, &mut store.borrow_mut());
    let save_timer = Rc::new(Timer::default());
    let toast_timer = Rc::new(Timer::default());
    let attention_clock = Instant::now();
    let attention = Rc::new(RefCell::new(attention::Attention::new(0)));
    {
        let policy = attention.clone();
        let animal = pet.as_weak();
        ui.on_viewed(move || {
            policy
                .borrow_mut()
                .viewed(attention_clock.elapsed().as_secs());
            if let Some(p) = animal.upgrade() {
                p.set_nudge_token(p.get_nudge_token().wrapping_add(1));
                p.set_nudge_text("".into());
                p.set_nudging(false);
            }
        });
    }
    let attention_timer = Timer::default();
    {
        let policy = attention.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        attention_timer.start(TimerMode::Repeated, Duration::from_secs(30), move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let busy = pet.get_drop_busy()
                || pet.get_drop_hover()
                || !pet.get_toast().is_empty()
                || pet.get_due_count() > 0
                || ui.get_completion_undo();
            if policy.borrow_mut().tick(
                attention_clock.elapsed().as_secs(),
                pet.get_count() > 0,
                ui.get_revealed(),
                busy,
            ) {
                nudge(&pet, format!("{} 件待办，空了看看", pet.get_count()));
            }
        });
    }

    let import_result = std::sync::Arc::new(std::sync::Mutex::new(
        None::<std::result::Result<Note, String>>,
    ));
    {
        let result = import_result.clone();
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let timer = toast_timer.clone();
        ui.on_drop_ready(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            ui.set_drop_busy(false);
            pet.set_drop_busy(false);
            let Some(result) = result.lock().unwrap().take() else {
                return;
            };
            let mut s = state.borrow_mut();
            match result.and_then(|note| {
                let copy = note.clone();
                s.capture_note(note).map_err(|e| {
                    drop_import::discard(&copy, &s.dir);
                    e.to_string()
                })
            }) {
                Ok(()) => {
                    sync(&ui, &pet, &mut s);
                    ui.set_notice("拖入内容已收下".into());
                    toast(&pet, &timer, "已收下 · 点击撤销", true, false);
                    pet.set_happy(true);
                    let weak = pet.as_weak();
                    Timer::single_shot(Duration::from_millis(420), move || {
                        if let Some(pet) = weak.upgrade() {
                            pet.set_happy(false);
                        }
                    });
                }
                Err(error) => {
                    ui.set_notice(format!("未收下：{error}").into());
                    toast(&pet, &timer, "未收下 · 点击查看", false, true);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = store.clone();
        ui.on_show_files(move |id| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let s = state.borrow();
            if let Some(file) = (if id < 0 {
                Some(&s.buffer)
            } else {
                s.data.notes.get(id as usize)
            })
            .and_then(|n| n.files.first())
            {
                let result = (|| -> Result<()> {
                    let base = s.dir.join("files").canonicalize()?;
                    let target = base.join(&file.path).canonicalize()?;
                    if !target.starts_with(&base) {
                        return Err("附件路径无效".into());
                    }
                    let folder = target.parent().ok_or("附件目录不可用")?;
                    let text: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
                    use windows::{
                        core::{w, PCWSTR},
                        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
                    };
                    let result = unsafe {
                        ShellExecuteW(
                            None,
                            w!("open"),
                            PCWSTR(text.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        )
                    };
                    if result.0 as isize <= 32 {
                        return Err("无法打开附件文件夹".into());
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    ui.set_notice(error.to_string().into());
                }
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let timer = save_timer.clone();
        ui.on_draft_edited(move |text| {
            state.borrow_mut().buffer.text = text.into();
            let state = state.clone();
            let weak = weak.clone();
            timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(600),
                move || {
                    if let Err(e) = state.borrow_mut().flush() {
                        if let Some(ui) = weak.upgrade() {
                            report(&ui, e);
                        }
                    }
                },
            );
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_save(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            if s.buffer.text.trim().is_empty()
                && s.buffer.images.is_empty()
                && s.buffer.files.is_empty()
            {
                return;
            }
            let editing = s.selected.is_some();
            let before = s.data.clone();
            if !editing {
                let note = s.buffer.clone();
                s.data.notes.push(note);
                s.data.draft = Note::default();
            } else if let Some(i) = s.selected {
                s.data.notes[i] = s.buffer.clone();
            }
            if let Err(e) = save_data(&s.dir, &s.data) {
                s.data = before;
                report(&ui, e);
                return;
            }
            s.selected = None;
            s.buffer = s.data.draft.clone();
            sync(&ui, &pet, &mut s);
            pet.set_happy(true);
            let animal = pet.as_weak();
            Timer::single_shot(Duration::from_millis(420), move || {
                if let Some(p) = animal.upgrade() {
                    p.set_happy(false);
                }
            });
            ui.set_notice("收好啦，继续安心做事吧".into());
            ui.set_expanded_id(-1);
            ui.set_focus_token(ui.get_focus_token() + 1);
            place(&ui, &pet);
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_paste_image(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return false;
            };
            let mut s = state.borrow_mut();
            match s.clipboard_image() {
                Ok(Some(note)) => {
                    s.buffer.images.extend(note.images);
                    if let Err(e) = s.flush() {
                        report(&ui, e);
                    }
                    sync(&ui, &pet, &mut s);
                    place(&ui, &pet);
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    ui.set_notice(format!("无法贴入图片：{e}").into());
                    true
                }
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let timer = toast_timer.clone();
        ui.on_capture(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            let result = s.clipboard().and_then(|note| s.capture_note(note));
            match result {
                Ok(()) => {
                    sync(&ui, &pet, &mut s);
                    ui.set_notice("剪贴板内容已收下".into());
                    toast(&pet, &timer, "已收下 · 点击撤销", true, false);
                    pet.set_happy(true);
                    let animal = pet.as_weak();
                    Timer::single_shot(Duration::from_millis(420), move || {
                        if let Some(p) = animal.upgrade() {
                            p.set_happy(false);
                        }
                    });
                }
                Err(error) => {
                    ui.set_notice(format!("无法收下：{error}").into());
                    toast(&pet, &timer, "未收下 · 点击查看", false, true);
                }
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let timer = toast_timer.clone();
        pet.on_undo(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            match s.undo_capture() {
                Ok(()) => {
                    sync(&ui, &pet, &mut s);
                    ui.set_notice("已撤销快捷收下".into());
                    toast(&pet, &timer, "已撤销", false, false);
                }
                Err(error) => {
                    ui.set_notice(format!("未撤销：{error}").into());
                    toast(&pet, &timer, "未撤销 · 点击查看", false, true);
                }
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_remove_image(move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let mut s = state.borrow_mut();
                if !s.buffer.files.is_empty() {
                    s.buffer.files.pop();
                } else {
                    s.buffer.images.pop();
                }
                if let Err(e) = s.flush() {
                    report(&ui, e);
                }
                sync(&ui, &pet, &mut s);
                place(&ui, &pet);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        ui.on_begin_completion(move |id, x, y, w, h, top, bottom| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut s = state.borrow_mut();
                    if s.finishing.contains_key(&(id as usize)) {
                        return;
                    }
                    if s.data.notes.get(id as usize).is_some_and(|n| !n.done) {
                        s.snap_rects.insert(id as usize, [x, y, w, h, top, bottom]);
                    }
                }
                ui.invoke_toggle(id);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_toggle(move |id| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let i = id as usize;
            let mut s = state.borrow_mut();
            if s.finishing.contains_key(&i) {
                return;
            }
            let Some(note) = s.data.notes.get(i) else {
                return;
            };
            if note.done {
                if let Err(e) = s.toggle_note(i) {
                    report(&ui, e);
                    return;
                }
                sync(&ui, &pet, &mut s);
                place(&ui, &pet);
                return;
            }
            if let Err(e) = s.complete(i) {
                report(&ui, e);
                return;
            }
            let deadline = s.completion.as_ref().unwrap().deadline;

            pet.set_can_undo(false);
            let all_done = s.data.notes.iter().all(|n| n.done);
            ui.set_notice(
                if all_done {
                    "都收拾好啦"
                } else {
                    "完成一件，轻松一点"
                }
                .into(),
            );
            pet.set_happy(true);
            pet.set_celebrating(all_done);
            sync(&ui, &pet, &mut s);
            if s.snaps.len() < 3 {
                if let Some(rect) = s.snap_rects.get(&i).copied() {
                    match capture_snap(&ui, rect, id as u32) {
                        Ok(visual) => {
                            s.snaps.insert(i, visual);
                        }
                        Err(e) => {
                            ui.set_notice(format!("已完成（动画未加载：{e}）").into());
                        }
                    }
                }
            }
            sync(&ui, &pet, &mut s);
            if std::env::args().any(|a| a == "--snapshot") {
                if let Some(v) = s.snaps.get(&i) {
                    let _ = fs::write(
                        s.dir.join("snap-metrics.txt"),
                        format!(
                            "layers={} raw_bytes={} capture_and_split_ms={:.2}",
                            snap::LAYERS,
                            v.bytes,
                            v.prepare_ms
                        ),
                    );
                }
            }
            let weak = ui.as_weak();
            let animal = pet.as_weak();
            let moving_state = state.clone();
            Timer::single_shot(Duration::from_millis(32), move || {
                let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                    return;
                };
                let mut s = moving_state.borrow_mut();
                if s.finishing
                    .get(&i)
                    .is_some_and(|(_, _, token)| *token == deadline)
                {
                    if let Some(visual) = s.snaps.get_mut(&i) {
                        visual.running = true;
                    }
                    sync(&ui, &pet, &mut s);
                }
            });
            finish_snap_later(&ui, &pet, state.clone(), i, deadline);
            let weak = ui.as_weak();
            let state = state.clone();
            Timer::single_shot(UNDO_DURATION, move || {
                if let Some(ui) = weak.upgrade() {
                    let mut s = state.borrow_mut();
                    if s.completion
                        .as_ref()
                        .is_some_and(|u| u.deadline == deadline)
                    {
                        s.completion = None;
                        ui.set_completion_undo(false);
                    }
                }
            });
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_undo_complete(move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            match s.undo_completion() {
                Ok(()) => ui.set_notice("已恢复待办和原来的提醒".into()),
                Err(e) => report(&ui, e),
            }
            sync(&ui, &pet, &mut s);
            place(&ui, &pet);
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        ui.on_inline_text(move |id, text| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut s = state.borrow_mut();
            if let Err(e) = s.update_record(id as usize, |note| note.text = text.into()) {
                report(&ui, e);
            } else {
                ui.set_notice("修改已保存".into());
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_inline_paste(move |id| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return false;
            };
            let mut s = state.borrow_mut();
            match s.clipboard_image() {
                Ok(Some(note)) => {
                    if let Err(e) =
                        s.update_record(id as usize, |n| n.images.extend(note.images.clone()))
                    {
                        drop_import::discard(&note, &s.dir);
                        report(&ui, e);
                    }
                    sync(&ui, &pet, &mut s);
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    report(&ui, e);
                    true
                }
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_inline_remove(move |id| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let mut s = state.borrow_mut();
            if let Err(e) = s.update_record(id as usize, |n| {
                if !n.files.is_empty() {
                    n.files.pop();
                } else {
                    n.images.pop();
                }
            }) {
                report(&ui, e);
            }
            sync(&ui, &pet, &mut s);
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_inline_reminder(move |id, preset| {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let at = match preset {
                0 => None,
                1 => Some(reminders::now() + reminders::HALF_HOUR),
                2 => reminders::later_today(chrono::Local::now()),
                _ => return,
            };
            let mut s = state.borrow_mut();
            if let Err(e) = s.set_reminder(id as usize, at) {
                report(&ui, e);
            }
            sync(&ui, &pet, &mut s);
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_edit(move |id| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let mut s = state.borrow_mut();
                let Some(note) = s.data.notes.get(id as usize) else {
                    return;
                };
                if ui.get_expanded_id() == id {
                    ui.set_expanded_id(-1);
                    ui.set_focus_token(ui.get_focus_token() + 1);
                } else {
                    ui.set_edit_body(note.text.clone().into());
                    ui.set_expanded_id(id);
                    if s.undo
                        .as_ref()
                        .is_some_and(|undo| undo.index == id as usize)
                    {
                        s.undo = None;
                        pet.set_can_undo(false);
                        pet.set_toast("记录已保留".into());
                    }
                }
                sync(&ui, &pet, &mut s);
                place(&ui, &pet);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_fold(move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                ui.set_show_done(!ui.get_show_done());
                sync(&ui, &pet, &mut state.borrow_mut());
                place(&ui, &pet);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_change_view(move |tasks| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let mut s = state.borrow_mut();
                if let Err(e) = s.flush() {
                    report(&ui, e);
                    return;
                }
                if !tasks {
                    s.selected = None;
                    s.buffer = s.data.draft.clone();
                }
                ui.set_task_view(false);
                sync(&ui, &pet, &mut s);
                place(&ui, &pet);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        ui.on_dismiss(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(e) = state.borrow_mut().flush() {
                    report(&ui, e);
                    return;
                }
                hide(&ui);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        ui.on_quit(move || {
            if weak.upgrade().is_some_and(|ui| ui.get_drop_busy()) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_notice("正在收下文件，请稍等再退出".into());
                }
                return;
            }
            if let Err(e) = state.borrow_mut().flush() {
                if let Some(ui) = weak.upgrade() {
                    report(&ui, e);
                }
                return;
            }
            let _ = slint::quit_event_loop();
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        pet.on_open_record(move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                if ui.get_revealed() {
                    ui.invoke_dismiss();
                } else {
                    ui.invoke_change_view(false);
                    reveal(&ui, &pet);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        pet.on_open_tasks(move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                if ui.get_revealed() {
                    ui.invoke_dismiss();
                } else {
                    ui.invoke_change_view(false);
                    reveal(&ui, &pet);
                }
            }
        });
    }
    let dragging = Rc::new(Cell::new(false));
    let restore = Rc::new(Cell::new(false));
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let drag = dragging.clone();
        let restore = restore.clone();
        pet.on_drag(move |dx, dy| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                if !drag.replace(true) {
                    restore.set(ui.get_revealed());
                    let _ = ui.hide();
                }
                let pos = pet.window().position();
                let scale = pet.window().scale_factor();
                pet.window().set_position(PhysicalPosition::new(
                    pos.x + (dx * scale) as i32,
                    pos.y + (dy * scale) as i32,
                ));
            }
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let drag = dragging.clone();
        pet.on_drag_finished(move || {
            if drag.replace(false) && restore.replace(false) {
                if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                    reveal(&ui, &pet);
                }
            }
        });
    }
    let _hotkey = if snapshot {
        None
    } else {
        let weak = ui.as_weak();
        match hotkey::Hotkey::register(move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.invoke_capture();
                }
            });
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                ui.set_notice(format!("Ctrl+Alt+V 注册失败（可能已被占用）：{error}").into());
                toast(&pet, &toast_timer, "快捷键不可用 · 查看", false, true);
                None
            }
        }
    };
    pet.show()?;
    let drop_guard = Rc::new(RefCell::new(None));
    {
        let holder = drop_guard.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let store = store.clone();
        let import_result = import_result.clone();
        let toast_timer = toast_timer.clone();
        slint::spawn_local(async move {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                match create_drop_target(&ui, &pet, &store, &import_result, &toast_timer).await {
                    Ok(registration) => {
                        *holder.borrow_mut() = registration;
                    }
                    Err(error) => {
                        ui.set_notice(format!("拖放不可用：{error}").into());
                        toast(&pet, &toast_timer, "拖放不可用 · 查看", false, true);
                    }
                }
                if snapshot {
                    let _ = fs::write(
                        store.borrow().dir.join("drop-registration.txt"),
                        if holder.borrow().is_some() {
                            "PASS: native drop target registered"
                        } else {
                            "FAIL: native drop target registration"
                        },
                    );
                }
            }
        })?;
    }
    place_at_startup(&pet);
    reveal(&ui, &pet);
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.window().on_winit_window_event(move |_, event| {
            match event {
                winit::event::WindowEvent::Resized(_) => {
                    let weak = weak.clone();
                    let animal = animal.clone();
                    Timer::single_shot(Duration::ZERO, move || {
                        if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                            if ui.get_revealed() {
                                place(&ui, &pet);
                            }
                        }
                    });
                }
                winit::event::WindowEvent::Focused(false) if !snapshot => {
                    let weak = weak.clone();
                    let animal = animal.clone();
                    Timer::single_shot(Duration::from_millis(100), move || {
                        if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                            let focused = ui
                                .window()
                                .with_winit_window(|w| w.has_focus())
                                .unwrap_or(false)
                                || pet
                                    .window()
                                    .with_winit_window(|w| w.has_focus())
                                    .unwrap_or(false);
                            if !focused && ui.get_revealed() {
                                ui.invoke_dismiss();
                            }
                        }
                    });
                }
                winit::event::WindowEvent::CloseRequested => {
                    if let Some(ui) = weak.upgrade() {
                        ui.invoke_dismiss();
                    }
                    return EventResult::PreventDefault;
                }
                winit::event::WindowEvent::KeyboardInput { event, .. }
                    if event.state == winit::event::ElementState::Pressed
                        && event.logical_key
                            == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =>
                {
                    if let Some(ui) = weak.upgrade() {
                        if ui.get_task_view() {
                            ui.invoke_dismiss();
                            return EventResult::PreventDefault;
                        }
                    }
                }
                _ => {}
            }
            EventResult::Propagate
        });
    }
    // Winit applies the first native resize/DPI events once its event loop starts.
    let initial_ui = ui.as_weak();
    let initial_pet = pet.as_weak();
    Timer::single_shot(Duration::from_millis(30), move || {
        if let (Some(ui), Some(pet)) = (initial_ui.upgrade(), initial_pet.upgrade()) {
            place_at_startup(&pet);
            place(&ui, &pet);
        }
    });
    if snapshot {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let state = store.clone();
        let out = store.borrow().dir.clone();
        Timer::single_shot(Duration::from_millis(700), move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let p = pet.window().position();
                let size = pet.window().size();
                let panel = ui.window().position();
                let panel_size = ui.window().size();
                let _ = fs::write(
                    out.join("layout.txt"),
                    format!(
                        "scale={}\npet=({}, {}) {}x{}\npanel=({}, {}) {}x{}\n",
                        pet.window().scale_factor(),
                        p.x,
                        p.y,
                        size.width,
                        size.height,
                        panel.x,
                        panel.y,
                        panel_size.width,
                        panel_size.height,
                    ),
                );
                let result = snapshot_window(ui.window(), &out.join("record.png"))
                    .and_then(|_| snapshot_window(pet.window(), &out.join("pet.png")));
                pet.set_drop_hover(true);
                let _ = snapshot_window(pet.window(), &out.join("drop-hover.png"));
                pet.set_drop_hover(false);
                pet.set_toast("已收下 · 点击撤销".into());
                pet.set_can_undo(true);
                let _ = snapshot_window(pet.window(), &out.join("capture-toast.png"));
                pet.set_toast("".into());
                pet.set_can_undo(false);
                if let Err(e) = result {
                    let _ = fs::write(out.join("snapshot-error.txt"), e.to_string());
                }
                ui.set_task_view(true);
                place(&ui, &pet);
                let weak = ui.as_weak();
                let animal = pet.as_weak();
                Timer::single_shot(Duration::from_millis(400), move || {
                    if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                        if let Err(e) = snapshot_window(ui.window(), &out.join("tasks.png")) {
                            let _ = fs::write(out.join("snapshot-error.txt"), e.to_string());
                        }
                        ui.invoke_change_view(false);
                        reveal(&ui, &pet);
                        let weak = ui.as_weak();
                        let animal = pet.as_weak();
                        Timer::single_shot(Duration::from_millis(100), move || {
                            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                                let result = check_input_flow(&ui, &pet, &state);
                                let _ = fs::write(out.join("input-check.txt"), match result {
                                    Ok(()) => "PASS: autofocus, Shift+Enter, Enter saves without closing, independent inline edit, Escape draft persistence".to_string(),
                                    Err(e) => format!("FAIL: {e}"),
                                });
                                start_completion_check(&ui, &pet, state.clone());
                            }
                        });
                    }
                });
            }
        });
    }
    slint::run_event_loop()?;
    Ok(())
}
fn snapshot_window(window: &slint::Window, path: &Path) -> Result<()> {
    let image = window.take_snapshot()?;
    image::save_buffer(
        path,
        image.as_bytes(),
        image.width(),
        image.height(),
        image::ColorType::Rgba8,
    )?;
    Ok(())
}

fn check_input_flow(ui: &PocketWindow, pet: &PetWindow, store: &Rc<RefCell<Store>>) -> Result<()> {
    use slint::platform::{Key, WindowEvent};
    fn press(ui: &PocketWindow, text: slint::SharedString) {
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
    ui.set_draft("".into());
    ui.invoke_draft_edited("".into());
    // Dispatch real Slint input events: this verifies focus and keyboard routing.
    press(ui, "hello".into());
    if ui.get_draft() != "hello" {
        return Err("input did not receive focus".into());
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(ui, Key::Return.into());
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    press(ui, "world".into());
    if ui.get_draft() != "hello\nworld" {
        return Err("Shift+Enter did not insert newline".into());
    }
    if std::env::args().any(|arg| arg == "--clipboard-check") {
        let before = store.borrow().buffer.images.len();
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(ui, "v".into());
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        if store.borrow().buffer.images.len() != before + 1 {
            return Err(format!("Ctrl+V image paste failed: {}", ui.get_notice()).into());
        }
        if ui.get_draft() != "hello\nworld" {
            return Err("image paste changed draft text".into());
        }
        let saved = Store::load(store.borrow().dir.clone())?;
        if saved.data.draft.images.len() != before + 1 {
            return Err("pasted image not persisted".into());
        }
        fs::write(
            store.borrow().dir.join("clipboard-check.txt"),
            "PASS: Ctrl+V received clipboard image, preserved text and persisted attachment",
        )?;
    }
    let count = store.borrow().data.notes.len();
    press(ui, Key::Return.into());
    if store.borrow().data.notes.len() != count + 1 || !ui.get_revealed() {
        return Err("Enter did not save and keep the unified panel open".into());
    }
    reveal(ui, pet);
    press(ui, "unfinished".into());
    let id = count as i32;
    ui.invoke_edit(id);
    if ui.get_expanded_id() != id || ui.get_draft() != "unfinished" {
        return Err("inline edit replaced the new-record draft".into());
    }
    ui.set_edit_body("edited in place".into());
    ui.invoke_inline_text(id, "edited in place".into());
    let saved = Store::load(store.borrow().dir.clone())?;
    if saved.data.notes[id as usize].text != "edited in place"
        || store.borrow().buffer.text != "unfinished"
    {
        return Err("inline edit did not persist independently".into());
    }
    snapshot_window(ui.window(), &store.borrow().dir.join("inline-edit.png"))?;
    ui.invoke_edit(id);
    {
        let mut s = store.borrow_mut();
        let notes = std::mem::take(&mut s.data.notes);
        let buffer = std::mem::take(&mut s.buffer);
        sync(ui, pet, &mut s);
        place(ui, pet);
        snapshot_window(ui.window(), &s.dir.join("empty-panel.png"))?;
        s.data.notes = notes;
        s.buffer = buffer;
        sync(ui, pet, &mut s);
        place(ui, pet);
    }
    press(ui, Key::Escape.into());
    let saved = Store::load(store.borrow().dir.clone())?;
    if saved.data.draft.text != "unfinished" || ui.get_revealed() {
        return Err("Escape did not preserve draft".into());
    }
    Ok(())
}
fn start_completion_check(ui: &PocketWindow, pet: &PetWindow, state: Rc<RefCell<Store>>) {
    {
        let mut s = state.borrow_mut();
        let demo = image::RgbaImage::from_fn(180, 110, |x, y| {
            if y < 22 {
                image::Rgba([55, 80, 115, 255])
            } else if y > 45 && y < 92 && x > 18 && x < 155 && x % 35 < 22 {
                image::Rgba([70 + (x % 100) as u8, 155, 190, 255])
            } else {
                image::Rgba([233, 240, 247, 255])
            }
        });
        let name = "snap-demo.png";
        demo.save(s.dir.join("images").join(name)).unwrap();
        if let Some(note) = s.data.notes.last_mut() {
            note.text = "确认这张截图，发给同事".into();
            note.images = vec![name.into()];
        }
        s.flush().unwrap();
        sync(ui, pet, &mut s);
    }
    reveal(ui, pet);
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    Timer::single_shot(Duration::from_millis(250), move || {
        if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
            run_completion_check(&ui, &pet, state);
        }
    });
}
fn run_completion_check(ui: &PocketWindow, pet: &PetWindow, state: Rc<RefCell<Store>>) {
    let id = (state.borrow().data.notes.len() - 1) as i32;
    ui.set_completion_request(id);
    let weak = ui.as_weak();
    let state_check = state.clone();
    let prepared = Rc::new(Cell::new(false));
    let prepared_check = prepared.clone();
    Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(ui) = weak.upgrade() {
            let s = state_check.borrow();
            prepared_check.set(
                s.snaps
                    .get(&(id as usize))
                    .is_some_and(|v| v.bytes > 0 && v.running),
            );
            if !prepared_check.get() {
                let _ = fs::write(
                    s.dir.join("snap-error.txt"),
                    format!("Missing pixel layers: {}", ui.get_notice()),
                );
            }
        }
    });
    for (delay, name) in [
        (420, "completion-dust.png"),
        (850, "completion-late.png"),
        (1240, "completion-collapse.png"),
    ] {
        let weak = ui.as_weak();
        let out = state.borrow().dir.clone();
        Timer::single_shot(Duration::from_millis(delay), move || {
            if let Some(ui) = weak.upgrade() {
                let _ = snapshot_window(ui.window(), &out.join(name));
            }
        });
    }
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    Timer::single_shot(Duration::from_millis(1600), move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let result = (|| -> Result<()> {
            if !prepared.get()
                || !state.borrow().snaps.is_empty()
                || !state.borrow().data.notes[id as usize].done
                || !state.borrow().finishing.is_empty()
            {
                return Err("completion timer failed".into());
            }
            ui.invoke_undo_complete();
            if state.borrow().data.notes[id as usize].done {
                return Err("completion undo failed".into());
            }
            nudge(&pet, "3 件待办，空了看看".into());
            snapshot_window(pet.window(), &state.borrow().dir.join("attention-pet.png"))?;
            Ok(())
        })();
        let _ = fs::write(
            state.borrow().dir.join("completion-check.txt"),
            match result {
                Ok(()) => "PASS: real pixel layers, timed collapse, image buffer release, undo and attention rendering".into(),
                Err(e) => format!("FAIL: {e}"),
            },
        );
        hide(&ui);
        start_reminder_check(&ui, &pet, state);
    });
}
fn start_reminder_check(ui: &PocketWindow, pet: &PetWindow, state: Rc<RefCell<Store>>) {
    let out = state.borrow().dir.clone();
    {
        let mut s = state.borrow_mut();
        for note in &mut s.data.notes {
            note.remind_at = None;
        }
        if let Err(e) = s.set_reminder(0, Some(reminders::now() + 1)) {
            let _ = fs::write(out.join("reminder-check.txt"), format!("FAIL: {e}"));
            let _ = slint::quit_event_loop();
            return;
        }
        sync(ui, pet, &mut s);
    }
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    Timer::single_shot(Duration::from_millis(1500), move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        if pet.get_due_count() != 1 || ui.get_revealed() {
            let _ = fs::write(
                out.join("reminder-check.txt"),
                "FAIL: timer did not notify quietly",
            );
            let _ = slint::quit_event_loop();
            return;
        }
        let _ = snapshot_window(pet.window(), &out.join("reminder-pet.png"));
        ui.invoke_change_view(true);
        reveal(&ui, &pet);
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        Timer::single_shot(Duration::from_millis(220), move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let result = (|| -> Result<()> {
                    snapshot_window(ui.window(), &out.join("reminder-tasks.png"))?;
                    ui.invoke_snooze(0);
                    if pet.get_due_count() != 0
                        || state.borrow().data.notes[0].remind_at.unwrap_or(0) <= reminders::now()
                    {
                        return Err("snooze failed".into());
                    }
                    ui.invoke_acknowledge(0);
                    let saved = Store::load(out.clone())?;
                    if saved.data.notes[0].remind_at.is_some() || saved.data.notes[0].done {
                        return Err("acknowledge did not preserve unfinished note".into());
                    }
                    Ok(())
                })();
                let _ = fs::write(
                    out.join("reminder-check.txt"),
                    match result {
                        Ok(()) => "PASS: real timer, quiet badge, snooze, acknowledge persistence"
                            .to_string(),
                        Err(e) => format!("FAIL: {e}"),
                    },
                );
            }
            let _ = slint::quit_event_loop();
        });
    });
}
fn main() {
    if let Err(e) = run() {
        let msg: Vec<u16> = format!("Slint 预览启动失败：{e}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                msg.as_ptr(),
                msg.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cleanup_store(dir: &Path) {
        for name in ["state.json", "state.backup.json"] {
            if dir.join(name).exists() {
                fs::remove_file(dir.join(name)).unwrap();
            }
        }
        fs::remove_dir(dir.join("images")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn multiple_completions_undo_only_latest_and_recompletion_has_new_token() {
        let mut s = temporary_store();
        for text in ["first", "second"] {
            s.data.notes.push(Note {
                text: text.into(),
                ..Note::default()
            });
        }
        s.flush().unwrap();
        s.complete(0).unwrap();
        s.complete(1).unwrap();
        let old = s.finishing[&1].2;
        s.undo_completion().unwrap();
        assert!(s.data.notes[0].done);
        assert!(!s.data.notes[1].done);
        assert!(s.finishing.contains_key(&0));
        assert!(!s.finishing.contains_key(&1));
        s.complete(1).unwrap();
        assert_ne!(s.finishing[&1].2, old);
        cleanup_store(&s.dir);
    }
    #[test]
    fn completion_is_durable_and_undo_restores_reminder_without_touching_draft() {
        let mut s = temporary_store();
        s.buffer.text = "keep my draft".into();
        s.data.notes.push(Note {
            text: "finish me".into(),
            remind_at: Some(12345),
            ..Note::default()
        });
        s.flush().unwrap();
        s.complete(0).unwrap();
        let loaded = Store::load(s.dir.clone()).unwrap();
        assert!(loaded.data.notes[0].done);
        assert_eq!(loaded.data.notes[0].remind_at, None);
        assert!(s.complete(0).is_err());
        s.undo_completion().unwrap();
        let loaded = Store::load(s.dir.clone()).unwrap();
        assert!(!loaded.data.notes[0].done);
        assert_eq!(loaded.data.notes[0].remind_at, Some(12345));
        assert_eq!(loaded.data.draft.text, "keep my draft");
        assert!(s.finishing.is_empty());
        cleanup_store(&s.dir);
    }
    #[test]
    fn failed_completion_and_expired_or_modified_undo_are_safe() {
        let mut s = temporary_store();
        s.data.notes.push(Note {
            text: "test".into(),
            ..Note::default()
        });
        s.flush().unwrap();
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.complete(0).is_err());
        assert!(!s.data.notes[0].done);
        assert!(s.finishing.is_empty());
        fs::remove_dir(s.dir.join("state.tmp")).unwrap();
        s.complete(0).unwrap();
        s.completion.as_mut().unwrap().deadline = Instant::now() - Duration::from_secs(1);
        assert!(s.undo_completion().is_err());
        assert!(s.data.notes[0].done);
        s.completion.as_mut().unwrap().deadline = Instant::now() + UNDO_DURATION;
        s.update_record(0, |n| n.text = "changed".into()).unwrap();
        assert!(s.undo_completion().is_err());
        assert_eq!(s.data.notes[0].text, "changed");
        cleanup_store(&s.dir);
    }
    #[test]
    fn inline_edit_preserves_draft_and_rolls_back_failed_save() {
        let mut s = temporary_store();
        s.buffer.text = "unfinished new note".into();
        s.data.notes.push(Note {
            text: "old".into(),
            ..Note::default()
        });
        s.flush().unwrap();
        s.update_record(0, |n| n.text = "edited".into()).unwrap();
        let loaded = Store::load(s.dir.clone()).unwrap();
        assert_eq!(loaded.data.notes[0].text, "edited");
        assert_eq!(loaded.data.draft.text, "unfinished new note");
        assert_eq!(s.buffer.text, "unfinished new note");
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.update_record(0, |n| n.text = "unsaved".into()).is_err());
        assert_eq!(s.data.notes[0].text, "edited");
        fs::remove_dir(s.dir.join("state.tmp")).unwrap();
        cleanup_store(&s.dir);
    }
    #[test]
    fn reminder_persistence_legacy_and_completion() {
        let old: Data = serde_json::from_str(r#"{"notes":[{"text":"旧记录","images":[],"done":false}],"draft":{"text":"","images":[],"done":false}}"#).unwrap();
        assert!(old.notes[0].remind_at.is_none());
        let mut s = temporary_store();
        s.data = old;
        s.set_reminder(0, Some(100)).unwrap();
        let mut restarted = Store::load(s.dir.clone()).unwrap();
        assert!(reminders::is_due(
            restarted.data.notes[0].remind_at,
            false,
            200
        ));
        restarted
            .set_reminder(0, Some(200 + reminders::HALF_HOUR))
            .unwrap();
        assert!(!reminders::is_due(
            restarted.data.notes[0].remind_at,
            false,
            200
        ));
        restarted.toggle_note(0).unwrap();
        restarted.toggle_note(0).unwrap();
        let loaded = Store::load(s.dir.clone()).unwrap();
        assert!(!loaded.data.notes[0].done);
        assert!(loaded.data.notes[0].remind_at.is_none());
        cleanup_store(&s.dir);
    }
    #[test]
    fn reminder_failed_save_preserves_selected_buffer() {
        let mut s = temporary_store();
        s.data.notes.push(Note {
            text: "需要提醒".into(),
            remind_at: Some(100),
            ..Note::default()
        });
        s.selected = Some(0);
        s.buffer = s.data.notes[0].clone();
        s.flush().unwrap();
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.set_reminder(0, None).is_err());
        assert_eq!(s.buffer.remind_at, Some(100));
        assert_eq!(s.data.notes[0].remind_at, Some(100));
        assert!(s.toggle_note(0).is_err());
        assert!(!s.buffer.done);
        fs::remove_dir(s.dir.join("state.tmp")).unwrap();
        s.set_reminder(0, None).unwrap();
        assert!(Store::load(s.dir.clone()).unwrap().data.notes[0]
            .remind_at
            .is_none());
        cleanup_store(&s.dir);
    }
    #[test]
    fn file_only_drop_persists_and_undo_keeps_copy() {
        let mut s = temporary_store();
        fs::create_dir(s.dir.join("files")).unwrap();
        fs::write(s.dir.join("files/测试.txt"), "附件内容").unwrap();
        s.buffer.text = "未写完的草稿".into();
        s.capture_note(Note {
            files: vec![Attachment {
                name: "测试.txt".into(),
                path: "测试.txt".into(),
            }],
            ..Note::default()
        })
        .unwrap();
        let loaded = Store::load(s.dir.clone()).unwrap();
        assert_eq!(loaded.data.notes[0].files.len(), 1);
        assert_eq!(loaded.buffer.text, "未写完的草稿");
        s.undo_capture().unwrap();
        assert!(Store::load(s.dir.clone()).unwrap().data.notes.is_empty());
        assert_eq!(
            fs::read_to_string(s.dir.join("files/测试.txt")).unwrap(),
            "附件内容"
        );
        fs::remove_file(s.dir.join("files/测试.txt")).unwrap();
        fs::remove_dir(s.dir.join("files")).unwrap();
        cleanup_store(&s.dir);
    }
    fn temporary_store() -> Store {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let suffix = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Store::load(
            std::env::temp_dir().join(format!("pocket-capture-{}-{suffix}", std::process::id())),
        )
        .unwrap()
    }
    #[test]
    fn capture_undo_preserves_draft_and_only_removes_latest_capture() {
        let mut store = temporary_store();
        store.buffer.text = "正在写的草稿".into();
        store
            .capture_note(Note {
                text: "第一次".into(),
                ..Note::default()
            })
            .unwrap();
        store
            .capture_note(Note {
                images: vec!["screenshot.png".into()],
                ..Note::default()
            })
            .unwrap();
        store.undo_capture().unwrap();
        let loaded = Store::load(store.dir.clone()).unwrap();
        assert_eq!(loaded.data.notes.len(), 1);
        assert_eq!(loaded.data.notes[0].text, "第一次");
        assert_eq!(loaded.buffer.text, "正在写的草稿");
        assert!(store.undo_capture().is_err());
        cleanup_store(&store.dir);
    }
    #[test]
    fn expired_or_modified_capture_is_never_deleted() {
        let mut store = temporary_store();
        store
            .capture_note(Note {
                text: "保留".into(),
                ..Note::default()
            })
            .unwrap();
        store.undo.as_mut().unwrap().deadline = Instant::now();
        assert!(store.undo_capture().is_err());
        store
            .capture_note(Note {
                text: "可编辑".into(),
                ..Note::default()
            })
            .unwrap();
        store.data.notes[1].done = true;
        assert!(store.undo_capture().is_err());
        assert_eq!(store.data.notes.len(), 2);
        cleanup_store(&store.dir);
    }
    #[test]
    fn failed_write_rolls_back_capture_and_undo() {
        let mut store = temporary_store();
        store
            .capture_note(Note {
                text: "已保存".into(),
                ..Note::default()
            })
            .unwrap();
        fs::create_dir(store.dir.join("state.tmp")).unwrap();
        assert!(store
            .capture_note(Note {
                text: "不能保存".into(),
                ..Note::default()
            })
            .is_err());
        assert_eq!(store.data.notes.len(), 1);
        assert!(store.undo_capture().is_err());
        assert_eq!(store.data.notes.len(), 1);
        assert!(store.undo.is_some());
        fs::remove_dir(store.dir.join("state.tmp")).unwrap();
        store.undo_capture().unwrap();
        assert!(Store::load(store.dir.clone())
            .unwrap()
            .data
            .notes
            .is_empty());
        cleanup_store(&store.dir);
    }
    #[test]
    fn round_trip_preserves_draft_and_backup() {
        let dir = std::env::temp_dir().join(format!("slint-pet-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut data = Data {
            notes: vec![Note {
                text: "稍后回复".into(),
                images: vec!["one.png".into()],
                done: false,
                remind_at: None,
                files: vec![],
            }],
            draft: Note {
                text: "还没写完".into(),
                ..Note::default()
            },
        };
        save_data(&dir, &data).unwrap();
        data.notes[0].done = true;
        save_data(&dir, &data).unwrap();
        let loaded: Data =
            serde_json::from_slice(&fs::read(dir.join("state.json")).unwrap()).unwrap();
        let old: Data =
            serde_json::from_slice(&fs::read(dir.join("state.backup.json")).unwrap()).unwrap();
        assert!(loaded.notes[0].done);
        assert!(!old.notes[0].done);
        assert_eq!(loaded.draft.text, "还没写完");
        for name in ["state.json", "state.backup.json"] {
            fs::remove_file(dir.join(name)).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
}
