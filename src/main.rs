#![windows_subsystem = "windows"]

use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::HashMap,
    fs,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::atomic::{AtomicU32, Ordering},
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_FOCUS};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{Input::KeyboardAndMouse::*, Shell::ShellExecuteW, WindowsAndMessaging::*},
};
mod ui;
use windows_sys::Win32::UI::{HiDpi::*, Input::Ime::*};
static UI_DPI: AtomicU32 = AtomicU32::new(96);
static UI_ZOOM: AtomicU32 = AtomicU32::new(125);
#[derive(Serialize, Deserialize)]
struct Settings {
    zoom_percent: u32,
}
fn bounded_zoom(percent: i32) -> u32 {
    percent.clamp(75, 175) as u32
}
fn scaled_px(v: i32, dpi: u32, zoom: u32) -> i32 {
    ((v as i64 * dpi as i64 * zoom as i64 + 4800) / 9600) as i32
}
fn px(v: i32) -> i32 {
    scaled_px(
        v,
        UI_DPI.load(Ordering::Relaxed),
        UI_ZOOM.load(Ordering::Relaxed),
    )
}
fn logical(v: i32) -> i32 {
    ((v as i64 * 9600)
        / (UI_DPI.load(Ordering::Relaxed) as i64 * UI_ZOOM.load(Ordering::Relaxed) as i64))
        as i32
}
unsafe fn drawing_scale(dc: HDC) {
    SetMapMode(dc, MM_ANISOTROPIC);
    SetWindowExtEx(dc, 1000, 1000, null_mut());
    SetViewportExtEx(dc, px(1000), px(1000), null_mut());
}
unsafe fn move_control(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, repaint: i32) {
    MoveWindow(hwnd, px(x), px(y), px(w), px(h), repaint);
}
fn save_settings(dir: &std::path::Path, zoom: u32) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        dir.join("settings.tmp"),
        serde_json::to_vec(&Settings { zoom_percent: zoom })?,
    )?;
    fs::rename(dir.join("settings.tmp"), dir.join("settings.json"))?;
    Ok(())
}
unsafe fn apply_scale(a: &App, mut x: i32, mut y: i32) {
    a.layout_key.set(None);
    a.bubble_xy.set(None);
    a.bubble_shape.set(None);
    a.capsule_xy.set(None);
    let mut monitor: MONITORINFO = std::mem::zeroed();
    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(
        MonitorFromWindow(a.pet, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) != 0
    {
        x = x.clamp(
            monitor.rcWork.left,
            (monitor.rcWork.right - px(100)).max(monitor.rcWork.left),
        );
        y = y.clamp(
            monitor.rcWork.top,
            (monitor.rcWork.bottom - px(104)).max(monitor.rcWork.top),
        );
    }
    move_control(a.edit, 18, 38, 304, 50, 1);
    SendMessageW(a.list, LB_SETITEMHEIGHT, 0, px(108) as isize);
    move_control(GetDlgItem(a.panel, 105), 298, 8, 30, 24, 1);
    SetWindowPos(
        a.pet,
        null_mut(),
        x,
        y,
        px(100),
        px(104),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    refresh(a);
}
unsafe fn resize_font(a: &mut App) {
    let next = ui::font(px(14), 400);
    if !next.is_null() {
        let old = a.edit_font;
        a.edit_font = next;
        SendMessageW(a.edit, WM_SETFONT, next as usize, 1);
        DeleteObject(old as _);
    }
}
unsafe fn set_zoom(a: &mut App, percent: u32) {
    let zoom = bounded_zoom(percent as i32);
    if zoom == UI_ZOOM.load(Ordering::Relaxed) {
        return;
    }
    if let Err(e) = save_settings(&a.dir, zoom) {
        message(&format!("缩放设置保存失败：{e}"));
        return;
    }
    let mut pet: RECT = std::mem::zeroed();
    GetWindowRect(a.pet, &mut pet);
    UI_ZOOM.store(zoom, Ordering::Relaxed);
    resize_font(a);
    // Preserve the pet's centre rather than having zoom make it drift away.
    apply_scale(
        a,
        (pet.left + pet.right - px(100)) / 2,
        (pet.top + pet.bottom - px(104)) / 2,
    );
}
unsafe fn zoom_wheel(a: &mut App, wp: usize) {
    a.wheel_delta += (wp >> 16) as u16 as i16 as i32;
    let steps = a.wheel_delta / 120;
    a.wheel_delta %= 120;
    if steps != 0 {
        set_zoom(
            a,
            bounded_zoom(UI_ZOOM.load(Ordering::Relaxed) as i32 + steps * 10),
        );
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Note {
    text: String,
    images: Vec<String>,
    done: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Record,
    Tasks,
}
struct App {
    pet: HWND,
    capsule: HWND,
    panel: HWND,
    edit: HWND,
    list: HWND,
    status: HWND,
    notes: Vec<Note>,
    pending: Vec<String>,
    dir: PathBuf,
    selected: Option<usize>,
    draft: Note,
    visible: Vec<usize>,
    show_done: bool,
    loading: bool,
    font: HFONT,
    edit_font: HFONT,
    title_font: HFONT,
    thumbs: HashMap<String, (u32, u32, Vec<u8>)>,
    view: Cell<View>,
    bubble_shape: Cell<Option<i32>>,
    bubble_xy: Cell<Option<(i32, i32, i32)>>,
    capsule_xy: Cell<Option<(i32, i32)>>,
    layout_key: Cell<Option<(View, i32)>>,
    restore_after_drag: Cell<bool>,
    dragging: Cell<bool>,
    follow_pending: Cell<bool>,
    notice: Option<String>,
    undo_capture: Option<(usize, Note)>,
    wheel_delta: i32,
}
static mut APP: *mut App = null_mut();
const BUBBLE_W: i32 = 340;
const BUBBLE_H: i32 = 148;
const CAPSULE_W: i32 = 108;
const CAPSULE_H: i32 = 38;
fn surface_layout_sizes(
    pet: RECT,
    work: RECT,
    height: i32,
    panel_w: i32,
    badge_w: i32,
    badge_h: i32,
    gap: i32,
) -> (RECT, RECT) {
    let cx = (pet.left + pet.right) / 2;
    let capsule_x = (cx - badge_w / 2).clamp(work.left, (work.right - badge_w).max(work.left));
    let capsule_y = if pet.bottom + gap + badge_h <= work.bottom {
        pet.bottom + gap
    } else {
        pet.top - badge_h - gap
    };
    let capsule_y = capsule_y.clamp(work.top, (work.bottom - badge_h).max(work.top));
    let capsule = RECT {
        left: capsule_x,
        top: capsule_y,
        right: capsule_x + badge_w,
        bottom: capsule_y + badge_h,
    };
    let mut x = (cx - panel_w / 2).clamp(work.left, (work.right - panel_w).max(work.left));
    let below = pet.bottom.max(capsule.bottom) + gap * 2;
    let above = pet.top.min(capsule.top) - height - gap * 2;
    let y = if below + height <= work.bottom {
        below
    } else if above >= work.top {
        above
    } else {
        // At large DPI a tall list may fit only beside the pet, not above/below.
        let left = pet.left.min(capsule.left) - gap * 2 - panel_w;
        let right = pet.right.max(capsule.right) + gap * 2;
        if right + panel_w <= work.right {
            x = right;
        } else if left >= work.left {
            x = left;
        }
        (pet.top + pet.bottom - height) / 2
    };
    let y = y.clamp(work.top, (work.bottom - height).max(work.top));
    (
        capsule,
        RECT {
            left: x,
            top: y,
            right: x + panel_w,
            bottom: y + height,
        },
    )
}
fn panel_height(a: &App) -> i32 {
    match a.view.get() {
        View::Record => {
            if a.pending.is_empty() {
                148
            } else {
                216
            }
        }
        View::Tasks => 82 + (a.visible.len().clamp(1, 3) as i32) * 108,
    }
}
unsafe fn position_bubble(a: &App) {
    let mut pet: RECT = std::mem::zeroed();
    GetWindowRect(a.pet, &mut pet);
    let mut monitor: MONITORINFO = std::mem::zeroed();
    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(
        MonitorFromWindow(a.pet, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) == 0
    {
        return;
    }
    let height = px(panel_height(a));
    let badge_w = px(if a.notice.is_some() { 230 } else { CAPSULE_W });
    let (capsule, panel) = surface_layout_sizes(
        pet,
        monitor.rcWork,
        height,
        px(BUBBLE_W),
        badge_w,
        px(CAPSULE_H),
        px(4),
    );
    if a.capsule_xy.get() != Some((capsule.left, capsule.top)) {
        if SetWindowPos(
            a.capsule,
            HWND_TOPMOST,
            capsule.left,
            capsule.top,
            badge_w,
            px(CAPSULE_H),
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        ) != 0
        {
            a.capsule_xy.set(Some((capsule.left, capsule.top)));
            let region = CreateRoundRectRgn(0, 0, badge_w + 1, px(CAPSULE_H) + 1, px(38), px(38));
            if SetWindowRgn(a.capsule, region, 0) == 0 {
                DeleteObject(region as _);
            }
            MoveWindow(
                GetDlgItem(a.capsule, 202),
                px(4),
                px(4),
                badge_w - px(8),
                px(30),
                1,
            );
        }
    }
    // During dragging, move only the small capsule; the expanded surface is hidden.
    if a.dragging.get() {
        return;
    }
    if a.bubble_shape.get() != Some(height) {
        let region = CreateRoundRectRgn(0, 0, px(BUBBLE_W) + 1, height + 1, px(24), px(24));
        if SetWindowRgn(a.panel, region, 0) == 0 {
            DeleteObject(region as _);
        } else {
            a.bubble_shape.set(Some(height));
            InvalidateRect(a.panel, null(), 0);
        }
    }
    if a.bubble_xy.get() != Some((panel.left, panel.top, height)) {
        if SetWindowPos(
            a.panel,
            HWND_TOPMOST,
            panel.left,
            panel.top,
            px(BUBBLE_W),
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        ) != 0
        {
            a.bubble_xy.set(Some((panel.left, panel.top, height)));
        }
    }
}
unsafe fn show_view(a: &App, view: View) {
    a.view.set(view);
    ui::layout(a);
    position_bubble(a);
    ShowWindow(a.panel, SW_SHOW);
    SetForegroundWindow(a.panel);
    SetFocus(if view == View::Record { a.edit } else { a.list });
    InvalidateRect(a.panel, null(), 0);
}
unsafe fn open_bubble(a: &mut App) {
    if !store_note(a) {
        return;
    }
    new_note(a);
    show_view(a, View::Record);
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn label(hwnd: HWND, text: &str) {
    SetWindowTextW(hwnd, wide(text).as_ptr());
}
unsafe fn control(
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: HWND,
    id: usize,
) -> HWND {
    CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        px(x),
        px(y),
        px(w),
        px(h),
        parent,
        id as _,
        GetModuleHandleW(null()),
        null(),
    )
}
unsafe fn message(text: &str) {
    MessageBoxW(
        null_mut(),
        wide(text).as_ptr(),
        wide("口袋宠物").as_ptr(),
        MB_OK | MB_ICONINFORMATION,
    );
}
fn persist(a: &App) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec_pretty(&a.notes)?;
    let tmp = a.dir.join("notes.tmp");
    fs::write(&tmp, bytes)?;
    // Keep a recovery copy before replacing the main file.
    let dst = a.dir.join("notes.json");
    if dst.exists() {
        fs::copy(&dst, a.dir.join("notes.backup.json"))?;
    }
    fs::rename(tmp, dst)?;
    Ok(())
}
unsafe fn save(a: &App) -> bool {
    let result = persist(a);
    if let Err(e) = result {
        message(&format!("保存失败，内容仍在窗口中：{e}"));
        false
    } else {
        true
    }
}
unsafe fn refresh(a: &App) {
    ui::layout(a);
    position_bubble(a);
    InvalidateRect(a.pet, null(), 1);
    InvalidateRect(a.panel, null(), 0);
    InvalidateRect(a.list, null(), 0);
    for id in [102, 108] {
        InvalidateRect(GetDlgItem(a.panel, id), null(), 1);
    }
    InvalidateRect(GetDlgItem(a.capsule, 202), null(), 1);
}
unsafe fn rebuild(a: &mut App) {
    a.loading = true;
    a.visible = (0..a.notes.len())
        .rev()
        .filter(|&i| a.show_done || !a.notes[i].done)
        .collect();
    a.visible.sort_by_key(|&i| a.notes[i].done);
    SendMessageW(a.list, LB_RESETCONTENT, 0, 0);
    for &i in &a.visible {
        SendMessageW(a.list, LB_ADDSTRING, 0, i as isize);
    }
    a.loading = false;
    refresh(a);
}
unsafe fn text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    let mut v = vec![0u16; len as usize + 1];
    GetWindowTextW(hwnd, v.as_mut_ptr(), v.len() as i32);
    String::from_utf16_lossy(&v[..len as usize])
}
unsafe fn select(a: &mut App, i: usize) {
    a.loading = true;
    a.selected = Some(i);
    a.pending = a.notes[i].images.clone();
    label(a.edit, &a.notes[i].text);
    label(a.status, "正在编辑 · 修改自动保存");
    a.loading = false;
    refresh(a);
    show_view(a, View::Record);
}
unsafe fn new_note(a: &mut App) {
    a.loading = true;
    a.selected = None;
    a.pending = a.draft.images.clone();
    label(a.edit, &a.draft.text);
    label(a.status, "Ctrl+V 贴截图 · Ctrl+Enter 添加");
    SendMessageW(a.list, LB_SETCURSEL, usize::MAX, 0);
    refresh(a);
    SetFocus(a.edit);
    a.loading = false;
}
unsafe fn store_note(a: &mut App) -> bool {
    let value = text(a.edit);
    KillTimer(a.panel, 7);
    if a.selected.is_none() {
        a.draft = Note {
            text: value,
            images: a.pending.clone(),
            done: false,
        };
        return save_draft(a);
    }
    let old = serde_json::to_string(&a.notes).unwrap();
    if let Some(i) = a.selected {
        a.notes[i].text = value;
        a.notes[i].images = a.pending.clone();
    }
    if !save(a) {
        a.notes = serde_json::from_str(&old).unwrap();
        if a.selected.is_some_and(|i| i >= a.notes.len()) {
            a.selected = None;
        }
        return false;
    }
    refresh(a);
    label(a.status, "已保存到本机 · 收起后不会丢失");
    true
}
fn write_draft(dir: &PathBuf, draft: &Note) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(dir.join("draft.tmp"), serde_json::to_vec(draft)?)?;
    fs::rename(dir.join("draft.tmp"), dir.join("draft.json"))?;
    Ok(())
}
unsafe fn save_draft(a: &App) -> bool {
    match write_draft(&a.dir, &a.draft) {
        Ok(()) => true,
        Err(e) => {
            message(&format!("草稿保存失败：{e}"));
            false
        }
    }
}
unsafe fn submit(a: &mut App) {
    if a.selected.is_some() {
        if store_note(a) {
            new_note(a);
            rebuild(a);
            show_view(a, View::Tasks);
        }
        return;
    }
    let value = text(a.edit);
    if value.trim().is_empty() && a.pending.is_empty() {
        return;
    }
    a.notes.push(Note {
        text: value,
        images: a.pending.clone(),
        done: false,
    });
    if !save(a) {
        a.notes.pop();
        return;
    }
    a.draft = Note::default();
    save_draft(a);
    new_note(a);
    rebuild(a);
    label(a.status, "收好啦，继续安心做事吧");
    ShowWindow(a.panel, SW_HIDE);
}
unsafe fn paste_image(a: &mut App) {
    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        let mut cb = arboard::Clipboard::new()?;
        let im = cb.get_image()?;
        let name = format!(
            "{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        image::save_buffer_with_format(
            a.dir.join("images").join(&name),
            &im.bytes,
            im.width as u32,
            im.height as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )?;
        Ok(name)
    })();
    match result {
        Ok(name) => {
            a.pending.push(name);
            label(a.status, &format!("已附加 {} 张图片", a.pending.len()));
            store_note(a);
            refresh(a);
        }
        Err(e) => message(&format!("无法读取剪贴板图片。请先截图或复制图片。\n{e}")),
    }
}
fn clipboard_note(dir: &std::path::Path) -> Result<Note, Box<dyn std::error::Error>> {
    let mut cb = arboard::Clipboard::new()?;
    if let Ok(im) = cb.get_image() {
        let name = format!(
            "{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        image::save_buffer_with_format(
            dir.join("images").join(&name),
            &im.bytes,
            im.width as u32,
            im.height as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )?;
        return Ok(Note {
            text: String::new(),
            images: vec![name],
            done: false,
        });
    }
    let value = cb.get_text().map_err(|_| "剪贴板没有可记录的文字或图片")?;
    if value.trim().is_empty() {
        return Err("剪贴板是空的".into());
    }
    Ok(Note {
        text: value,
        images: vec![],
        done: false,
    })
}
unsafe fn show_notice(a: &mut App, notice: &str) {
    a.notice = Some(notice.to_owned());
    a.capsule_xy.set(None);
    position_bubble(a);
    InvalidateRect(a.capsule, null(), 0);
    InvalidateRect(GetDlgItem(a.capsule, 202), null(), 1);
    SetTimer(a.pet, 9, 6000, None);
}
unsafe fn capture_clipboard(a: &mut App) {
    match clipboard_note(&a.dir) {
        Ok(note) => {
            if !store_note(a) {
                return;
            }
            let index = a.notes.len();
            a.notes.push(note.clone());
            if !save(a) {
                a.notes.pop();
                return;
            }
            a.undo_capture = Some((index, note));
            rebuild(a);
            show_notice(a, "已收下");
        }
        Err(e) => {
            a.undo_capture = None;
            show_notice(a, &e.to_string());
        }
    }
}
fn undo_unchanged(notes: &mut Vec<Note>, index: usize, original: &Note) -> bool {
    if notes.get(index) != Some(original) {
        return false;
    }
    notes.remove(index);
    true
}
unsafe fn undo_clipboard(a: &mut App) {
    if !store_note(a) {
        return;
    }
    if let Some((index, original)) = a.undo_capture.clone() {
        let before = a.notes.clone();
        if undo_unchanged(&mut a.notes, index, &original) {
            if !save(a) {
                a.notes = before;
                return;
            }
            if a.selected == Some(index) {
                new_note(a);
            } else if let Some(i) = a.selected {
                if i > index {
                    a.selected = Some(i - 1);
                }
            }
            a.undo_capture = None;
            rebuild(a);
            show_notice(a, "已撤销");
        } else {
            a.undo_capture = None;
            show_notice(a, "内容已修改，保留记录");
        }
    }
}
unsafe fn ime_composing(edit: HWND) -> bool {
    let context = ImmGetContext(edit);
    if context.is_null() {
        return false;
    }
    let active = ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0) > 0;
    ImmReleaseContext(edit, context);
    active
}
unsafe fn rect(dc: HDC, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let brush = CreateSolidBrush(color);
    FillRect(
        dc,
        &RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        brush,
    );
    DeleteObject(brush as _);
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_MEASUREITEM {
        (*(lp as *mut MEASUREITEMSTRUCT)).itemHeight = px(108) as u32;
        return 1;
    }
    if APP.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let a = &mut *APP;
    match msg {
        WM_DRAWITEM => {
            let mut item = *(lp as *const DRAWITEMSTRUCT);
            let state = SaveDC(item.hDC);
            drawing_scale(item.hDC);
            item.rcItem = RECT {
                left: logical(item.rcItem.left),
                top: logical(item.rcItem.top),
                right: logical(item.rcItem.right),
                bottom: logical(item.rcItem.bottom),
            };
            ui::owner_draw(a, &item);
            RestoreDC(item.hDC, state);
            1
        }
        WM_DPICHANGED if hwnd == a.pet => {
            UI_DPI.store((wp >> 16) as u32, Ordering::Relaxed);
            let r = &*(lp as *const RECT);
            resize_font(a);
            apply_scale(a, r.left, r.top);
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
            let color = if msg == WM_CTLCOLOREDIT {
                0x00ffffff
            } else {
                ui::BG
            };
            SetBkColor(wp as HDC, color);
            SetTextColor(wp as HDC, 0x003b3532);
            SetDCBrushColor(wp as HDC, color);
            GetStockObject(DC_BRUSH) as isize
        }
        WM_ERASEBKGND if hwnd == a.panel || hwnd == a.pet || hwnd == a.capsule => 1,
        WM_ENTERSIZEMOVE if hwnd == a.pet => {
            a.dragging.set(true);
            a.restore_after_drag.set(IsWindowVisible(a.panel) != 0);
            ShowWindow(a.panel, SW_HIDE);
            0
        }
        WM_EXITSIZEMOVE if hwnd == a.pet => {
            a.dragging.set(false);
            a.follow_pending.set(false);
            KillTimer(a.pet, 8);
            position_bubble(a);
            if a.restore_after_drag.replace(false) {
                ShowWindow(a.panel, SW_SHOWNOACTIVATE);
            }
            0
        }
        WM_TIMER if hwnd == a.pet && wp == 8 => {
            KillTimer(a.pet, 8);
            a.follow_pending.set(false);
            position_bubble(a);
            0
        }
        WM_TIMER if wp == 7 => {
            KillTimer(a.panel, 7);
            store_note(a);
            0
        }
        WM_HOTKEY => {
            if wp == 2 {
                capture_clipboard(a);
            } else {
                open_bubble(a);
            }
            0
        }
        WM_TIMER if hwnd == a.pet && wp == 9 => {
            KillTimer(a.pet, 9);
            a.notice = None;
            a.undo_capture = None;
            a.capsule_xy.set(None);
            position_bubble(a);
            InvalidateRect(a.capsule, null(), 0);
            InvalidateRect(GetDlgItem(a.capsule, 202), null(), 1);
            0
        }
        WM_LBUTTONDOWN if hwnd == a.pet => {
            let mut before: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut before);
            ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            let mut after: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut after);
            if (before.left - after.left).abs() < GetSystemMetrics(SM_CXDRAG)
                && (before.top - after.top).abs() < GetSystemMetrics(SM_CYDRAG)
            {
                open_bubble(a);
            }
            0
        }
        WM_LBUTTONDBLCLK if hwnd == a.pet => {
            open_bubble(a);
            0
        }
        WM_RBUTTONUP if hwnd == a.pet => {
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, 1, wide("打开口袋").as_ptr());
            let sizes = CreatePopupMenu();
            let current = UI_ZOOM.load(Ordering::Relaxed);
            for value in [75, 100, 125, 150, 175] {
                AppendMenuW(
                    sizes,
                    MF_STRING | if value == current { MF_CHECKED } else { 0 },
                    (1000 + value) as usize,
                    wide(&format!(
                        "{}%{}",
                        value,
                        if value == 125 { "（默认）" } else { "" }
                    ))
                    .as_ptr(),
                );
            }
            AppendMenuW(
                menu,
                MF_POPUP,
                sizes as usize,
                wide(&format!("显示缩放：{}%", current)).as_ptr(),
            );
            AppendMenuW(menu, MF_STRING, 3, wide("恢复默认大小").as_ptr());
            AppendMenuW(menu, MF_STRING, 2, wide("退出").as_ptr());
            let mut p: POINT = std::mem::zeroed();
            GetCursorPos(&mut p);
            SetForegroundWindow(a.pet);
            let action = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY,
                p.x,
                p.y,
                0,
                a.pet,
                null(),
            );
            DestroyMenu(menu);
            if action == 1 {
                open_bubble(a);
            } else if action == 3 {
                set_zoom(a, 125);
            } else if (1075..=1175).contains(&action) {
                set_zoom(a, (action - 1000) as u32);
            } else if action == 2 && store_note(a) {
                PostQuitMessage(0);
            }
            0
        }
        WM_WINDOWPOSCHANGED if hwnd == a.pet => {
            let position = &*(lp as *const WINDOWPOS);
            if position.flags & SWP_NOMOVE == 0 {
                if a.dragging.get() {
                    // Coalesce high-frequency mouse messages; no timer at rest.
                    if !a.follow_pending.replace(true) && SetTimer(a.pet, 8, 16, None) == 0 {
                        a.follow_pending.set(false);
                        position_bubble(a);
                    }
                } else {
                    position_bubble(a);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_MOUSEWHEEL if wp & 0x0008 /* MK_CONTROL */ != 0 => {
            zoom_wheel(a, wp);
            0
        }
        WM_CLOSE => {
            if store_note(a) {
                ShowWindow(a.panel, SW_HIDE);
            }
            0
        }
        WM_COMMAND if hwnd == a.panel || hwnd == a.capsule => {
            let id = wp & 0xffff;
            match id {
                201 => {
                    if IsWindowVisible(a.panel) != 0 && a.view.get() == View::Record {
                        if store_note(a) {
                            ShowWindow(a.panel, SW_HIDE);
                        }
                    } else {
                        open_bubble(a);
                    }
                }
                202 => {
                    if a.undo_capture.is_some() {
                        undo_clipboard(a);
                        return 0;
                    }
                    if store_note(a) {
                        if IsWindowVisible(a.panel) != 0 && a.view.get() == View::Tasks {
                            ShowWindow(a.panel, SW_HIDE);
                        } else {
                            rebuild(a);
                            show_view(a, View::Tasks);
                        }
                    }
                }
                101 => {
                    if store_note(a) {
                        new_note(a);
                    }
                }
                102 => submit(a),
                103 => paste_image(a),
                107 => {
                    if !a.pending.is_empty() {
                        let menu = CreatePopupMenu();
                        for (j, _) in a.pending.iter().enumerate() {
                            AppendMenuW(
                                menu,
                                MF_STRING,
                                j + 1,
                                wide(&format!("移除图片 {}", j + 1)).as_ptr(),
                            );
                        }
                        let mut p: POINT = std::mem::zeroed();
                        GetCursorPos(&mut p);
                        let choice = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_NONOTIFY,
                            p.x,
                            p.y,
                            0,
                            a.panel,
                            null(),
                        );
                        DestroyMenu(menu);
                        if choice > 0 {
                            a.pending.remove(choice as usize - 1);
                            store_note(a);
                            refresh(a);
                        }
                    }
                }
                108 => {
                    a.show_done = !a.show_done;
                    rebuild(a);
                    InvalidateRect(GetDlgItem(a.panel, 108), null(), 1);
                }
                111 if (wp >> 16) == EN_CHANGE as usize && !a.loading => {
                    SetTimer(a.panel, 7, 600, None);
                    refresh(a);
                }
                104 => {
                    if let Some(i) = a.selected {
                        let previous = a.notes[i].done;
                        a.notes[i].done = !previous;
                        if !save(a) {
                            a.notes[i].done = previous;
                        }
                        refresh(a);
                    }
                }
                105 => {
                    if store_note(a) {
                        ShowWindow(a.panel, SW_HIDE);
                    }
                }
                106 => {
                    if store_note(a) {
                        UnregisterHotKey(a.pet, 1);
                        DestroyWindow(a.panel);
                        DestroyWindow(a.pet);
                        PostQuitMessage(0);
                    }
                }
                110 if (wp >> 16) == LBN_SELCHANGE as usize && !a.loading => {
                    let i = SendMessageW(a.list, LB_GETCURSEL, 0, 0);
                    if i >= 0 && store_note(a) {
                        if let Some(&index) = a.visible.get(i as usize) {
                            select(a, index);
                        }
                        SendMessageW(a.list, LB_SETCURSEL, i as usize, 0);
                    }
                }
                _ => {}
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let state = SaveDC(dc);
            drawing_scale(dc);
            if hwnd == a.pet {
                rect(dc, 0, 0, 100, 104, 0x00ff00ff);
                // Static pixel cat: no timer, render only when invalidated.
                rect(dc, 20, 25, 60, 47, 0x0083bbef);
                rect(dc, 20, 13, 16, 24, 0x0083bbef);
                rect(dc, 64, 13, 16, 24, 0x0083bbef);
                rect(dc, 28, 41, 7, 9, 0x00302a28);
                rect(dc, 65, 41, 7, 9, 0x00302a28);
                rect(dc, 46, 54, 9, 5, 0x00566ad8);
                rect(dc, 28, 72, 16, 8, 0x0083bbef);
                rect(dc, 58, 72, 16, 8, 0x0083bbef);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, 0x00302a28);
            } else if hwnd == a.capsule {
                ui::paint_capsule(dc, a);
            } else {
                ui::paint(dc, a);
            }
            RestoreDC(dc, state);
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        UI_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
        let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
            .join("PocketPet");
        if let Err(e) = fs::create_dir_all(dir.join("images")) {
            message(&format!("无法创建数据目录：{e}"));
            return;
        }
        if let Ok(bytes) = fs::read(dir.join("settings.json")) {
            if let Ok(settings) = serde_json::from_slice::<Settings>(&bytes) {
                UI_ZOOM.store(settings.zoom_percent.clamp(75, 175), Ordering::Relaxed);
            }
        }
        let path = dir.join("notes.json");
        let notes = if path.exists() {
            match fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
            {
                Some(n) => n,
                None => {
                    message("备忘数据读取失败。为避免覆盖原数据，程序退出。请检查 LOCALAPPDATA\\PocketPet 中的备份。");
                    return;
                }
            }
        } else {
            Vec::new()
        };
        let instance = GetModuleHandleW(null());
        let class = wide("PocketPetWindow");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let pet = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("口袋宠物").as_ptr(),
            WS_POPUP,
            GetSystemMetrics(SM_CXSCREEN) - px(140),
            GetSystemMetrics(SM_CYSCREEN) - px(206),
            px(100),
            px(104),
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        SetLayeredWindowAttributes(pet, 0x00ff00ff, 255, LWA_COLORKEY);
        let surface_class = wide("PocketPetSurface");
        let surface_wc = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpszClassName: surface_class.as_ptr(),
            ..wc
        };
        RegisterClassW(&surface_wc);
        let capsule = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            surface_class.as_ptr(),
            wide("口袋快捷栏").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            px(CAPSULE_W),
            px(CAPSULE_H),
            pet,
            null_mut(),
            instance,
            null(),
        );
        let region = CreateRoundRectRgn(0, 0, CAPSULE_W + 1, CAPSULE_H + 1, 38, 38);
        if SetWindowRgn(capsule, region, 0) == 0 {
            DeleteObject(region as _);
        }
        ui::capsule_controls(capsule);
        let panel = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            surface_class.as_ptr(),
            wide("口袋宠物 · 临时备忘盒").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            px(BUBBLE_W),
            px(BUBBLE_H),
            pet,
            null_mut(),
            instance,
            null(),
        );
        if pet.is_null() || panel.is_null() || capsule.is_null() {
            message("创建窗口失败");
            return;
        }
        let (edit, list, status) = ui::controls(panel);
        APP = Box::into_raw(Box::new(App {
            pet,
            capsule,
            panel,
            edit,
            list,
            status,
            notes,
            pending: Vec::new(),
            dir,
            selected: None,
            draft: Note::default(),
            visible: Vec::new(),
            show_done: false,
            loading: false,
            font: null_mut(),
            edit_font: null_mut(),
            notice: None,
            undo_capture: None,
            wheel_delta: 0,
            title_font: null_mut(),
            thumbs: HashMap::new(),
            view: Cell::new(View::Record),
            capsule_xy: Cell::new(None),
            layout_key: Cell::new(None),
            restore_after_drag: Cell::new(false),
            bubble_shape: Cell::new(None),
            bubble_xy: Cell::new(None),
            dragging: Cell::new(false),
            follow_pending: Cell::new(false),
        }));
        if RegisterHotKey(pet, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x4e) == 0 {
            message("Ctrl+Alt+N 已被占用，可以双击或右键宠物打开备忘盒。");
        }
        if RegisterHotKey(pet, 2, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x56) == 0 {
            message("Ctrl+Alt+V 已被占用，直接收下快捷键不可用。仍可点击小猫粘贴记录。");
        }
        (*APP).font = ui::font(14, 400);
        (*APP).title_font = ui::font(22, 600);
        (*APP).edit_font = ui::font(px(14), 400);
        for handle in [edit, list, status] {
            SendMessageW(handle, WM_SETFONT, (*APP).edit_font as usize, 1);
        }
        if let Ok(bytes) = fs::read((*APP).dir.join("draft.json")) {
            match serde_json::from_slice(&bytes) {
                Ok(d) => {
                    (*APP).draft = d;
                }
                Err(_) => {
                    message("草稿文件无法读取，原文件会保留为 draft.unreadable.json。");
                    let _ = fs::copy(
                        (*APP).dir.join("draft.json"),
                        (*APP).dir.join("draft.unreadable.json"),
                    );
                }
            }
        }
        new_note(&mut *APP);
        rebuild(&mut *APP);
        ShowWindow(pet, SW_SHOWNOACTIVATE);
        position_bubble(&*APP);
        ShowWindow(capsule, SW_SHOWNOACTIVATE);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_MOUSEWHEEL && msg.wParam & 0x0008 /* MK_CONTROL */ != 0 {
                zoom_wheel(&mut *APP, msg.wParam);
                continue;
            }
            if msg.hwnd == edit
                && msg.message == WM_KEYDOWN
                && msg.wParam == VK_RETURN as usize
                && GetKeyState(VK_SHIFT as i32) >= 0
                && !ime_composing(edit)
            {
                submit(&mut *APP);
                continue;
            }
            if msg.hwnd == edit && msg.message == WM_KEYDOWN && GetKeyState(VK_CONTROL as i32) < 0 {
                if msg.wParam == 0x56
                    && arboard::Clipboard::new()
                        .ok()
                        .and_then(|mut c| c.get_image().ok())
                        .is_some()
                {
                    paste_image(&mut *APP);
                    continue;
                }
            }
            if msg.hwnd == list && msg.message == WM_LBUTTONDOWN {
                let x = logical((msg.lParam as u16 as i16) as i32);
                let row = SendMessageW(list, LB_ITEMFROMPOINT, 0, msg.lParam);
                if (row >> 16) == 0 {
                    if let Some(&i) = (&(*APP).visible).get((row & 0xffff) as usize) {
                        if x < 38 {
                            if store_note(&mut *APP) {
                                let old = (&mut (*APP).notes)[i].done;
                                (&mut (*APP).notes)[i].done = !old;
                                if !save(&*APP) {
                                    (&mut (*APP).notes)[i].done = old;
                                }
                                rebuild(&mut *APP);
                                InvalidateRect(GetDlgItem(panel, 108), null(), 1);
                            }
                            continue;
                        }
                        if x >= 202 && !(&mut (*APP).notes)[i].images.is_empty() {
                            let menu = CreatePopupMenu();
                            for (j, _) in (&mut (*APP).notes)[i].images.iter().enumerate() {
                                AppendMenuW(
                                    menu,
                                    MF_STRING,
                                    j + 1,
                                    wide(&format!("查看图片 {}", j + 1)).as_ptr(),
                                );
                            }
                            let mut p: POINT = std::mem::zeroed();
                            GetCursorPos(&mut p);
                            let chosen = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_NONOTIFY,
                                p.x,
                                p.y,
                                0,
                                panel,
                                null(),
                            );
                            DestroyMenu(menu);
                            if chosen > 0 {
                                let path = (*APP)
                                    .dir
                                    .join("images")
                                    .join(&(&mut (*APP).notes)[i].images[chosen as usize - 1]);
                                ShellExecuteW(
                                    panel,
                                    wide("open").as_ptr(),
                                    wide(&path.to_string_lossy()).as_ptr(),
                                    null(),
                                    null(),
                                    SW_SHOWNORMAL,
                                );
                            }
                            continue;
                        }
                    }
                }
            }
            if msg.message == WM_KEYDOWN
                && msg.wParam == VK_ESCAPE as usize
                && (msg.hwnd == panel || IsChild(panel, msg.hwnd) != 0)
            {
                if store_note(&mut *APP) {
                    ShowWindow(panel, SW_HIDE);
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_bounds_and_dpi_combine_without_losing_saved_preference() {
        assert_eq!(bounded_zoom(-100), 75);
        assert_eq!(bounded_zoom(900), 175);
        assert_eq!(scaled_px(340, 96, 125), 425);
        assert_eq!(scaled_px(340, 144, 100), 510);
        let dir = std::env::temp_dir().join(format!("pet-zoom-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        save_settings(&dir, 150).unwrap();
        save_settings(&dir, 85).unwrap();
        let restored: Settings =
            serde_json::from_slice(&fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(restored.zoom_percent, 85);
        fs::remove_file(dir.join("settings.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn undo_only_removes_unchanged_capture_and_keeps_other_notes() {
        let captured = Note {
            text: "截图备注".into(),
            images: vec!["capture.png".into()],
            done: false,
        };
        let later = Note {
            text: "之后记录的事".into(),
            ..Note::default()
        };
        let mut notes = vec![captured.clone(), later.clone()];
        assert!(undo_unchanged(&mut notes, 0, &captured));
        assert_eq!(notes.len(), 1);
        assert!(notes[0] == later);
        notes.insert(0, captured.clone());
        notes[0].text.push_str(" 已补充");
        assert!(!undo_unchanged(&mut notes, 0, &captured));
        assert_eq!(notes.len(), 2);
        assert!(!undo_unchanged(&mut notes, 9, &captured));
    }
    #[test]
    fn scaled_layout_avoids_pet_and_badge() {
        for dpi in [96, 120, 144, 192] {
            let scale = |v: i32| (v * dpi * 5 + 192) / 384;
            let work = RECT {
                left: -2560,
                top: 0,
                right: 0,
                bottom: 1440,
            };
            for x in [-2560, -1500, -scale(100)] {
                for y in [0, 500, 1440 - scale(104)] {
                    let pet = RECT {
                        left: x,
                        top: y,
                        right: x + scale(100),
                        bottom: y + scale(104),
                    };
                    for height in [148, 216, 406] {
                        for bw in [108, 230] {
                            let (badge, panel) = surface_layout_sizes(
                                pet,
                                work,
                                scale(height),
                                scale(340),
                                scale(bw),
                                scale(38),
                                scale(4),
                            );
                            assert!(
                                panel.left >= work.left
                                    && panel.right <= work.right
                                    && panel.top >= work.top
                                    && panel.bottom <= work.bottom
                            );
                            for other in [pet, badge] {
                                assert!(
                                    panel.right <= other.left
                                        || panel.left >= other.right
                                        || panel.bottom <= other.top
                                        || panel.top >= other.bottom
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn floating_surfaces_fit_work_area_without_covering_pet() {
        for work in [
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
            RECT {
                left: -1920,
                top: -200,
                right: 0,
                bottom: 840,
            },
        ] {
            for x in [work.left, work.left + 800, work.right - 100] {
                for y in [work.top, work.top + 300, work.bottom - 104] {
                    let pet = RECT {
                        left: x,
                        top: y,
                        right: x + 100,
                        bottom: y + 104,
                    };
                    for height in [148, 216, 190, 406] {
                        let (capsule, panel) = surface_layout_sizes(
                            pet, work, height, BUBBLE_W, CAPSULE_W, CAPSULE_H, 4,
                        );
                        for r in [capsule, panel] {
                            assert!(r.left >= work.left && r.right <= work.right);
                            assert!(r.top >= work.top && r.bottom <= work.bottom);
                            assert!(
                                r.right <= pet.left
                                    || r.left >= pet.right
                                    || r.bottom <= pet.top
                                    || r.top >= pet.bottom
                            );
                        }
                        assert!(panel.bottom <= capsule.top || panel.top >= capsule.bottom);
                    }
                }
            }
        }
    }
    #[test]
    fn persistence_keeps_previous_version_and_unicode_attachments() {
        let dir = std::env::temp_dir().join(format!("pocket-pet-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut a = App {
            pet: null_mut(),
            capsule: null_mut(),
            panel: null_mut(),
            edit: null_mut(),
            list: null_mut(),
            status: null_mut(),
            notes: vec![Note {
                text: "稍后回复小王\n第二行".into(),
                images: vec!["test.png".into()],
                done: false,
            }],
            pending: vec![],
            dir: dir.clone(),
            selected: None,
            draft: Note::default(),
            visible: Vec::new(),
            show_done: false,
            loading: false,
            font: null_mut(),
            edit_font: null_mut(),
            notice: None,
            undo_capture: None,
            wheel_delta: 0,
            title_font: null_mut(),
            thumbs: HashMap::new(),
            view: Cell::new(View::Record),
            capsule_xy: Cell::new(None),
            layout_key: Cell::new(None),
            restore_after_drag: Cell::new(false),
            bubble_shape: Cell::new(None),
            bubble_xy: Cell::new(None),
            dragging: Cell::new(false),
            follow_pending: Cell::new(false),
        };
        persist(&a).unwrap();
        a.notes[0].done = true;
        persist(&a).unwrap();
        let current: Vec<Note> =
            serde_json::from_slice(&fs::read(dir.join("notes.json")).unwrap()).unwrap();
        let previous: Vec<Note> =
            serde_json::from_slice(&fs::read(dir.join("notes.backup.json")).unwrap()).unwrap();
        assert!(current[0].done);
        assert!(!previous[0].done);
        assert_eq!(current[0].text, "稍后回复小王\n第二行");
        assert_eq!(current[0].images, vec!["test.png"]);
        assert!(!dir.join("notes.tmp").exists());
        let draft = Note {
            text: "还没想好，不要创建任务".into(),
            images: vec!["draft.png".into()],
            done: false,
        };
        write_draft(&dir, &draft).unwrap();
        let restored: Note =
            serde_json::from_slice(&fs::read(dir.join("draft.json")).unwrap()).unwrap();
        assert_eq!(restored.text, draft.text);
        assert_eq!(restored.images, draft.images);
        let unchanged: Vec<Note> =
            serde_json::from_slice(&fs::read(dir.join("notes.json")).unwrap()).unwrap();
        assert_eq!(unchanged.len(), 1);
        assert_eq!(unchanged[0].text, current[0].text);
        fs::remove_file(dir.join("draft.json")).unwrap();
        fs::remove_file(dir.join("notes.json")).unwrap();
        fs::remove_file(dir.join("notes.backup.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    #[ignore = "offscreen visual review artifact"]
    fn render_surfaces_for_review() {
        let dir = PathBuf::from("target");
        let mut a = App {
            pet: null_mut(),
            capsule: null_mut(),
            panel: null_mut(),
            edit: null_mut(),
            list: null_mut(),
            status: null_mut(),
            notes: vec![Note {
                text: "稍后回复小王\n第二行".into(),
                images: vec!["test.png".into()],
                done: false,
            }],
            pending: vec![],
            dir: dir.clone(),
            selected: None,
            draft: Note::default(),
            visible: Vec::new(),
            show_done: false,
            loading: false,
            font: null_mut(),
            edit_font: null_mut(),
            notice: None,
            undo_capture: None,
            wheel_delta: 0,
            title_font: null_mut(),
            thumbs: HashMap::new(),
            view: Cell::new(View::Record),
            capsule_xy: Cell::new(None),
            layout_key: Cell::new(None),
            restore_after_drag: Cell::new(false),
            bubble_shape: Cell::new(None),
            bubble_xy: Cell::new(None),
            dragging: Cell::new(false),
            follow_pending: Cell::new(false),
        };
        a.notes = vec![
            Note {
                text: "回复小王的接口问题".into(),
                images: vec!["sample.png".into()],
                done: false,
            },
            Note {
                text: "看一下登录页 PR\ngithub.com/team/pull/42".into(),
                images: vec![],
                done: false,
            },
            Note {
                text: "整理测试结果，发给同事".into(),
                images: vec![],
                done: false,
            },
        ];
        a.visible = vec![0, 1, 2];
        a.pending = vec!["sample.png".into()];
        let mut pixels = vec![255u8; 144 * 100 * 4];
        for y in 0..100 {
            for x in 0..144 {
                let i = (y * 144 + x) * 4;
                let color = if y < 18 {
                    [228, 228, 228]
                } else if x > 9 && x < 124 && y % 19 < 7 {
                    [236, 221, 207]
                } else {
                    [250, 250, 250]
                };
                pixels[i..i + 3].copy_from_slice(&color);
            }
        }
        a.thumbs.insert("sample.png".into(), (144, 100, pixels));
        unsafe {
            a.font = ui::font(14, 400);
            a.title_font = ui::font(20, 600);
            ui::export_preview(&mut a, std::path::Path::new("target/capsule-preview.png"));
            DeleteObject(a.font as _);
            DeleteObject(a.title_font as _);
        }
    }
}
