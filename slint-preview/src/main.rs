#![windows_subsystem = "windows"]
use serde::{Deserialize, Serialize};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Image, ModelRc, PhysicalPosition, Timer, TimerMode, VecModel};
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
mod hotkey;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
struct Note {
    text: String,
    images: Vec<String>,
    done: bool,
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
        if note.text.trim().is_empty() && note.images.is_empty() {
            return Err("剪贴板没有可收下的内容".into());
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
    fn thumbnail(&mut self, name: &str) -> Image {
        if let Some(image) = self.cache.get(name) {
            return image.clone();
        }
        let result = (|| -> Result<Image> {
            let image = image::open(self.dir.join("images").join(name))?
                .thumbnail(240, 160)
                .to_rgba8();
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
        let mut clipboard = arboard::Clipboard::new()?;
        if let Ok(image) = clipboard.get_image() {
            let name = format!(
                "{}.png",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            );
            image::save_buffer(
                self.dir.join("images").join(&name),
                &image.bytes,
                image.width as u32,
                image.height as u32,
                image::ColorType::Rgba8,
            )?;
            return Ok(Some(Note {
                text: String::new(),
                images: vec![name],
                done: false,
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
    ui.set_draft(store.buffer.text.clone().into());
    ui.set_editing(store.selected.is_some());
    ui.set_has_attachment(!store.buffer.images.is_empty());
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
    let mut cards = vec![];
    for i in (0..store.data.notes.len()).rev() {
        let n = store.data.notes[i].clone();
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
            body: n.text.into(),
            preview,
            has_image: !n.images.is_empty(),
            image_count: n.images.len() as i32,
            done: n.done,
        });
    }
    cards.sort_by_key(|n| n.done);
    ui.set_cards(ModelRc::new(VecModel::from(cards)));
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
fn reveal(ui: &PocketWindow, pet: &PetWindow) {
    let _ = ui.show();
    place(ui, pet);
    ui.set_revealed(true);
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
fn run() -> Result<()> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
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
    let save_timer = Rc::new(Timer::default());
    let toast_timer = Rc::new(Timer::default());
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
            if s.buffer.text.trim().is_empty() && s.buffer.images.is_empty() {
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
            if editing {
                ui.set_task_view(true);
                place(&ui, &pet);
            } else {
                hide(&ui);
            }
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
                s.buffer.images.pop();
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
        let animal = pet.as_weak();
        ui.on_toggle(move |id| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let mut s = state.borrow_mut();
                let i = id as usize;
                if i >= s.data.notes.len() {
                    return;
                }
                let old = s.data.notes[i].done;
                if s.undo.as_ref().is_some_and(|undo| undo.index == i) {
                    s.undo = None;
                    pet.set_can_undo(false);
                    pet.set_toast("记录已更新".into());
                }
                s.data.notes[i].done = !old;
                if s.selected == Some(i) {
                    s.buffer.done = !old;
                }
                if let Err(e) = s.flush() {
                    s.data.notes[i].done = old;
                    if s.selected == Some(i) {
                        s.buffer.done = old;
                    }
                    report(&ui, e);
                }
                sync(&ui, &pet, &mut s);
            }
        });
    }
    {
        let state = store.clone();
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        ui.on_edit(move |id| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                let mut s = state.borrow_mut();
                if let Err(e) = s.flush() {
                    report(&ui, e);
                    return;
                }
                let i = id as usize;
                if i >= s.data.notes.len() {
                    return;
                }
                s.buffer = s.data.notes[i].clone();
                if s.undo.as_ref().is_some_and(|undo| undo.index == i) {
                    s.undo = None;
                    pet.set_can_undo(false);
                    pet.set_toast("记录已保留".into());
                }
                s.selected = Some(i);
                ui.set_task_view(false);
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
                ui.set_task_view(tasks);
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
                ui.invoke_change_view(false);
                reveal(&ui, &pet);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        pet.on_open_tasks(move || {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                ui.invoke_change_view(true);
                reveal(&ui, &pet);
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
                                    Ok(()) => "PASS: autofocus, Shift+Enter, Enter save, Escape draft persistence".to_string(),
                                    Err(e) => format!("FAIL: {e}"),
                                });
                            }
                            let _ = slint::quit_event_loop();
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
    let count = store.borrow().data.notes.len();
    press(ui, Key::Return.into());
    if store.borrow().data.notes.len() != count + 1 || ui.get_revealed() {
        return Err("Enter did not save and dismiss".into());
    }
    reveal(ui, pet);
    press(ui, "unfinished".into());
    press(ui, Key::Escape.into());
    let saved = Store::load(store.borrow().dir.clone())?;
    if saved.data.draft.text != "unfinished" || ui.get_revealed() {
        return Err("Escape did not preserve draft".into());
    }
    Ok(())
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
    fn temporary_store() -> Store {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
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
