use super::*;
pub const BG: u32 = 0x00ffffff;
const INK: u32 = 0x003b3532;
const MUTED: u32 = 0x008c8179;
const LINE: u32 = 0x00e7ebee;
const ACCENT: u32 = 0x00b9d7f7;
pub unsafe fn font(size: i32, weight: u32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Microsoft YaHei UI").as_ptr(),
    )
}
pub unsafe fn controls(panel: HWND) -> (HWND, HWND, HWND) {
    let edit = control(
        "EDIT",
        "",
        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32 | WS_TABSTOP,
        18,
        38,
        304,
        50,
        panel,
        111,
    );
    let list = control(
        "LISTBOX",
        "",
        LBS_NOTIFY as u32
            | LBS_OWNERDRAWFIXED as u32
            | LBS_NOINTEGRALHEIGHT as u32
            | WS_VSCROLL
            | WS_TABSTOP,
        10,
        42,
        320,
        108,
        panel,
        110,
    );
    SendMessageW(list, LB_SETITEMHEIGHT, 0, px(108) as isize);
    for (id, title, x, y, w, h) in [
        (102, "添加", 258, 104, 64, 30),
        (103, "＋ 图片", 12, 104, 74, 30),
        (107, "移除", 90, 104, 58, 30),
        (105, "×", 298, 8, 30, 24),
        (108, "已完成", 12, 154, 316, 26),
    ] {
        control(
            "BUTTON",
            title,
            BS_OWNERDRAW as u32 | WS_TABSTOP,
            x,
            y,
            w,
            h,
            panel,
            id,
        );
    }
    let status = control("STATIC", "", 0, 0, 0, 0, 0, panel, 0);
    ShowWindow(status, SW_HIDE);
    (edit, list, status)
}
pub unsafe fn capsule_controls(capsule: HWND) {
    control(
        "BUTTON",
        "待办",
        BS_OWNERDRAW as u32 | WS_TABSTOP,
        4,
        4,
        CAPSULE_W - 8,
        30,
        capsule,
        202,
    );
}
pub unsafe fn layout(a: &App) {
    let mode = a.view.get();
    let h = panel_height(a);
    let record = mode == View::Record;
    let key = (mode, h);
    if a.layout_key.get() != Some(key) {
        a.layout_key.set(Some(key));
        move_control(a.list, 10, 42, 320, h - 82, 1);
        for (id, x, w) in [(102, 258, 64), (103, 12, 74), (107, 90, 58)] {
            move_control(GetDlgItem(a.panel, id), x, h - 44, w, 30, 1);
        }
        move_control(GetDlgItem(a.panel, 108), 12, h - 34, 316, 26, 1);
    }
    for (handle, visible) in [
        (a.edit, record),
        (a.list, !record && !a.visible.is_empty()),
        (GetDlgItem(a.panel, 103), record),
        (GetDlgItem(a.panel, 107), record && !a.pending.is_empty()),
        (GetDlgItem(a.panel, 108), !record),
        (
            GetDlgItem(a.panel, 102),
            record
                && (a.selected.is_some()
                    || !a.pending.is_empty()
                    || !text(a.edit).trim().is_empty()),
        ),
    ] {
        ShowWindow(handle, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
pub unsafe fn paint_capsule(dc: HDC, a: &App) {
    round(
        dc,
        r(
            0,
            0,
            if a.notice.is_some() { 230 } else { CAPSULE_W },
            CAPSULE_H,
        ),
        BG,
        LINE,
        38,
    );

    let _ = a;
}
pub unsafe fn round(dc: HDC, r: RECT, color: u32, border: u32, radius: i32) {
    let b = CreateSolidBrush(color);
    let p = CreatePen(PS_SOLID, 1, border);
    let ob = SelectObject(dc, b as _);
    let op = SelectObject(dc, p as _);
    RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as _);
    DeleteObject(p as _);
}
pub unsafe fn words(dc: HDC, s: &str, mut r: RECT, color: u32, font: HFONT, flags: u32) {
    let old = SelectObject(dc, font as _);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, color);
    let v = wide(s);
    DrawTextW(
        dc,
        v.as_ptr(),
        v.len() as i32 - 1,
        &mut r,
        flags | DT_NOPREFIX,
    );
    SelectObject(dc, old);
}
fn r(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}
pub unsafe fn thumb(dc: HDC, a: &mut App, name: &str, bounds: RECT) {
    if !a.thumbs.contains_key(name) {
        // Bounded cache: never retain full-resolution images or an unlimited gallery.
        if a.thumbs.len() >= 48 {
            a.thumbs.clear();
        }
        if let Ok(im) = image::open(a.dir.join("images").join(name)) {
            let im = im.thumbnail(144, 100).to_rgba8();
            let (w, h) = im.dimensions();
            let mut pixels = im.into_raw();
            for p in pixels.chunks_exact_mut(4) {
                let alpha = p[3] as u32;
                for c in &mut p[..3] {
                    *c = (((*c as u32) * alpha + 255 * (255 - alpha)) / 255) as u8;
                }
                p.swap(0, 2);
            }
            a.thumbs.insert(name.to_owned(), (w, h, pixels));
        }
    }
    if let Some((w, h, pixels)) = a.thumbs.get(name) {
        let scale = ((bounds.right - bounds.left) as f64 / *w as f64)
            .min((bounds.bottom - bounds.top) as f64 / *h as f64);
        let dw = (*w as f64 * scale) as i32;
        let dh = (*h as f64 * scale) as i32;
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = *w as i32;
        info.bmiHeader.biHeight = -(*h as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        SetStretchBltMode(dc, HALFTONE);
        StretchDIBits(
            dc,
            bounds.left,
            bounds.top,
            dw,
            dh,
            0,
            0,
            *w as i32,
            *h as i32,
            pixels.as_ptr() as _,
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}
pub unsafe fn paint(dc: HDC, a: &mut App) {
    let h = panel_height(a);
    round(dc, r(0, 0, BUBBLE_W, h), BG, LINE, 24);
    match a.view.get() {
        View::Record => {
            words(
                dc,
                if a.selected.is_some() {
                    "编辑备忘"
                } else {
                    "记点什么…"
                },
                r(18, 12, 250, 20),
                MUTED,
                a.font,
                DT_SINGLELINE,
            );
            if let Some(name) = a.pending.first().cloned() {
                thumb(dc, a, &name, r(18, 98, 84, 58));
                words(
                    dc,
                    &format!("{} 张图片", a.pending.len()),
                    r(116, 114, 170, 22),
                    MUTED,
                    a.font,
                    DT_SINGLELINE,
                );
            }
        }
        View::Tasks => {
            let count = a.notes.iter().filter(|n| !n.done).count();
            words(
                dc,
                &format!("待办  {count}"),
                r(18, 12, 250, 22),
                INK,
                a.font,
                DT_SINGLELINE,
            );
            if a.visible.is_empty() {
                words(
                    dc,
                    "暂时没有待办\n想起什么，随手记下来",
                    r(24, 66, 292, 62),
                    MUTED,
                    a.font,
                    DT_CENTER | DT_WORDBREAK,
                );
            }
        }
    }
}
pub unsafe fn owner_draw(a: &mut App, d: &DRAWITEMSTRUCT) {
    if d.CtlID == 110 {
        rect(
            d.hDC,
            d.rcItem.left,
            d.rcItem.top,
            d.rcItem.right - d.rcItem.left,
            108,
            BG,
        );
        if d.itemID == u32::MAX {
            return;
        }
        let Some(&i) = a.visible.get(d.itemID as usize) else {
            return;
        };
        let n = a.notes[i].clone();
        let y = d.rcItem.top;
        round(
            d.hDC,
            r(0, y + 3, d.rcItem.right - 3, 99),
            0x00ffffff,
            if a.selected == Some(i) { ACCENT } else { LINE },
            16,
        );
        round(
            d.hDC,
            r(12, y + 18, 20, 20),
            if n.done { ACCENT } else { 0x00ffffff },
            if n.done { ACCENT } else { 0x00c8c4c0 },
            20,
        );
        if n.done {
            words(d.hDC, "✓", r(13, y + 17, 20, 22), INK, a.font, DT_CENTER);
        }
        let width = if n.images.is_empty() { 244 } else { 154 };
        words(
            d.hDC,
            if n.text.trim().is_empty() {
                "图片备忘"
            } else {
                &n.text
            },
            r(43, y + 16, width, 51),
            if n.done { MUTED } else { INK },
            a.font,
            DT_WORDBREAK | DT_END_ELLIPSIS,
        );
        words(
            d.hDC,
            if n.done {
                "已完成 · 点击圆圈恢复"
            } else {
                "点击编辑"
            },
            r(43, y + 75, 156, 19),
            MUTED,
            a.font,
            DT_SINGLELINE,
        );
        if let Some(name) = n.images.first() {
            thumb(d.hDC, a, name, r(207, y + 15, 80, 60));
            words(
                d.hDC,
                &format!("{} 张 · 查看", n.images.len()),
                r(207, y + 75, 95, 20),
                MUTED,
                a.font,
                DT_SINGLELINE,
            );
        }
    } else {
        let id = d.CtlID;
        let title = match id {
            102 => if a.selected.is_some() {
                "返回"
            } else {
                "添加"
            }
            .to_owned(),
            201 => "＋ 记录".to_owned(),
            202 => {
                if let Some(notice) = &a.notice {
                    if a.undo_capture.is_some() {
                        format!(
                            "{} · 待办 {} · 撤销",
                            notice,
                            a.notes.iter().filter(|n| !n.done).count()
                        )
                    } else {
                        notice.clone()
                    }
                } else {
                    format!("待办 {}", a.notes.iter().filter(|n| !n.done).count())
                }
            }
            108 => format!(
                "{}  已完成 · {}",
                if a.show_done { "⌄" } else { "›" },
                a.notes.iter().filter(|n| n.done).count()
            ),
            _ => text(d.hwndItem),
        };
        let primary = id == 102;
        let active =
            !text(a.edit).trim().is_empty() || !a.pending.is_empty() || a.selected.is_some();
        rect(
            d.hDC,
            0,
            0,
            d.rcItem.right,
            d.rcItem.bottom,
            if id == 105 || id == 108 {
                BG
            } else {
                0x00ffffff
            },
        );
        let color = if primary && active {
            ACCENT
        } else if id == 105 || id == 108 {
            BG
        } else {
            0x00f5f7f9
        };
        round(d.hDC, d.rcItem, color, color, 12);
        words(
            d.hDC,
            &title,
            d.rcItem,
            if primary && active { INK } else { MUTED },
            a.font,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        if d.itemState & ODS_FOCUS != 0 {
            let mut bounds = d.rcItem;
            InflateRect(&mut bounds, -3, -3);
            DrawFocusRect(d.hDC, &bounds);
        }
    }
}

// Offscreen review of the actual GDI drawing routines, without operating the desktop.
#[cfg(test)]
pub unsafe fn export_preview(a: &mut App, path: &std::path::Path) {
    let (width, height) = (px(760), px(610));
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let dc = CreateCompatibleDC(null_mut());
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    assert!(!bitmap.is_null() && !bits.is_null());
    let old = SelectObject(dc, bitmap as _);
    drawing_scale(dc);
    rect(dc, 0, 0, width, height, 0x00ececec);
    words(
        dc,
        "口袋宠物 / 快速记录与放大显示",
        r(24, 18, 700, 30),
        INK,
        a.title_font,
        DT_SINGLELINE,
    );
    for (view, x) in [(View::Record, 24), (View::Tasks, 396)] {
        a.view.set(view);
        preview_origin(dc, x + 116, 64, null_mut());
        paint_capsule(dc, a);
        for (id, bx) in [(202, 4)] {
            preview_origin(dc, x + 116 + bx, 68, null_mut());
            let d = DRAWITEMSTRUCT {
                CtlID: id,
                hDC: dc,
                rcItem: r(0, 0, 100, 30),
                ..std::mem::zeroed()
            };
            owner_draw(a, &d);
        }
        preview_origin(dc, x, 120, null_mut());
        paint(dc, a);
        if view == View::Record {
            words(
                dc,
                "稍后回复小王的接口问题",
                r(18, 38, 304, 50),
                INK,
                a.font,
                DT_WORDBREAK,
            );
            for (id, bx, w) in [(103, 12, 74), (107, 90, 58), (102, 258, 64)] {
                preview_origin(dc, x + bx, 120 + panel_height(a) - 44, null_mut());
                let d = DRAWITEMSTRUCT {
                    CtlID: id,
                    hDC: dc,
                    rcItem: r(0, 0, w, 30),
                    ..std::mem::zeroed()
                };
                if id == 103 || id == 107 {
                    round(dc, d.rcItem, 0x00f5f7f9, 0x00f5f7f9, 12);
                    words(
                        dc,
                        if id == 103 { "＋ 图片" } else { "移除" },
                        d.rcItem,
                        MUTED,
                        a.font,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                } else {
                    owner_draw(a, &d);
                }
            }
        } else {
            preview_origin(dc, x + 10, 162, null_mut());
            for i in 0..a.visible.len().min(3) {
                let d = DRAWITEMSTRUCT {
                    CtlID: 110,
                    itemID: i as u32,
                    hDC: dc,
                    rcItem: r(0, i as i32 * 108, 303, 108),
                    ..std::mem::zeroed()
                };
                owner_draw(a, &d);
            }
            preview_origin(dc, x + 12, 120 + panel_height(a) - 34, null_mut());
            owner_draw(
                a,
                &DRAWITEMSTRUCT {
                    CtlID: 108,
                    hDC: dc,
                    rcItem: r(0, 0, 316, 26),
                    ..std::mem::zeroed()
                },
            );
        }
        preview_origin(dc, x + 298, 128, null_mut());
        words(
            dc,
            "×",
            r(0, 0, 30, 24),
            MUTED,
            a.font,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    preview_origin(dc, 0, 0, null_mut());
    words(
        dc,
        "绘制预览，非桌面运行截图；原生输入框和滚动条需另行实测。",
        r(24, 568, 720, 24),
        MUTED,
        a.font,
        DT_SINGLELINE,
    );
    GdiFlush();
    let mut pixels =
        std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec();
    for p in pixels.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
    image::save_buffer(
        path,
        &pixels,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
    )
    .unwrap();
    SelectObject(dc, old);
    DeleteObject(bitmap as _);
    DeleteDC(dc);
}

#[cfg(test)]
unsafe fn preview_origin(dc: HDC, x: i32, y: i32, old: *mut POINT) {
    SetViewportOrgEx(dc, px(x), px(y), old);
}
