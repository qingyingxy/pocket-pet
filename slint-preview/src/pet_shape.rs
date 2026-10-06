use super::*;
use image::{imageops::FilterType, RgbaImage};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

const PET_FRAME_SUBCLASS: usize = 0x70657466;

// Winit keeps caption style bits for native window management. With a custom
// region, DefWindowProc can paint that caption over our client pixels on focus
// changes. Preserve winit's activation handling, but suppress non-client paint.
unsafe extern "system" fn frameless_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    match message {
        WM_NCPAINT => 0,
        // The documented -1 parameter prevents DefWindowProc from redrawing
        // the non-client area while winit still receives the activation event.
        WM_NCACTIVATE => DefSubclassProc(hwnd, message, wparam, -1),
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(frameless_proc), id);
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
    count: i32,
    status: bool,
    drop: bool,
    window_width: u32,
    window_height: u32,
    offset: (i32, i32),
}

impl Geometry {
    fn read(pet: &PetWindow, hwnd: HWND) -> Option<Self> {
        let mut origin = POINT { x: 0, y: 0 };
        let mut window: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            if ClientToScreen(hwnd, &mut origin) == 0 || GetWindowRect(hwnd, &mut window) == 0 {
                return None;
            }
        }
        Some(Self {
            x: pet.get_sprite_x(),
            y: pet.get_sprite_y(),
            width: pet.get_sprite_width(),
            height: pet.get_sprite_height(),
            scale: pet.window().scale_factor(),
            count: pet.get_count(),
            status: pet.get_status_visible(),
            drop: pet.get_drop_hover(),
            window_width: pet.window().size().width,
            window_height: pet.window().size().height,
            offset: (origin.x - window.left, origin.y - window.top),
        })
    }

    // Match Image's `contain` fit, including centering during the celebration pose.
    fn sprite_rect(self, source: &RgbaImage) -> (f32, f32, f32, f32) {
        let fit = (self.width / source.width() as f32).min(self.height / source.height() as f32);
        let w = source.width() as f32 * fit;
        let h = source.height() as f32 * fit;
        (
            self.x + (self.width - w) / 2.,
            self.y + (self.height - h) / 2.,
            w,
            h,
        )
    }
}

struct Mask {
    width: u32,
    height: u32,
    pixels: Vec<bool>,
}

impl Mask {
    fn new(source: &RgbaImage, g: Geometry) -> Self {
        let mut mask = Self {
            width: g.window_width,
            height: g.window_height,
            pixels: vec![false; (g.window_width * g.window_height) as usize],
        };
        let (x, y, w, h) = g.sprite_rect(source);
        let image = image::imageops::resize(
            source,
            (w * g.scale).round().max(1.) as u32,
            (h * g.scale).round().max(1.) as u32,
            FilterType::Triangle,
        );
        let left = (x * g.scale).round() as i32;
        let top = (y * g.scale).round() as i32;
        for (x, y, pixel) in image.enumerate_pixels() {
            // Keep antialiased outline pixels; completely transparent pixels cannot click.
            if pixel[3] != 0 {
                mask.set(left + x as i32, top + y as i32);
            }
        }
        if g.count > 0 {
            let w = if g.count > 99 {
                36.
            } else if g.count > 9 {
                30.
            } else {
                24.
            };
            mask.rounded_rect(g.scale, [105., 6., w, 24., 12.]);
        }
        if g.status {
            mask.rounded_rect(g.scale, [3., 104., 154., 30., 15.]);
        }
        if g.drop {
            mask.rounded_rect(g.scale, [6., 1.5, 112.5, 96., 22.5]);
        }
        mask
    }

    fn set(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
            self.pixels[(y as u32 * self.width + x as u32) as usize] = true;
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= 0.
            && y >= 0.
            && x < self.width as f32
            && y < self.height as f32
            && self.pixels[(y as u32 * self.width + x as u32) as usize]
    }

    fn rounded_rect(&mut self, scale: f32, rect: [f32; 5]) {
        let [x, y, w, h, r] = rect.map(|v| v * scale);
        for py in y.floor() as i32..(y + h).ceil() as i32 {
            for px in x.floor() as i32..(x + w).ceil() as i32 {
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                let dx = cx - cx.clamp(x + r, x + w - r);
                let dy = cy - cy.clamp(y + r, y + h - r);
                if cx >= x && cy >= y && cx < x + w && cy < y + h && dx * dx + dy * dy <= r * r {
                    self.set(px, py);
                }
            }
        }
    }

    fn region(&self, offset: (i32, i32)) -> Option<Region> {
        let region = Region::new(unsafe { CreateRectRgn(0, 0, 0, 0) })?;
        for y in 0..self.height {
            let row = &self.pixels[(y * self.width) as usize..((y + 1) * self.width) as usize];
            let mut x = 0;
            while x < self.width {
                if !row[x as usize] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.width && row[x as usize] {
                    x += 1;
                }
                let part = Region::new(unsafe {
                    CreateRectRgn(
                        start as i32 + offset.0,
                        y as i32 + offset.1,
                        x as i32 + offset.0,
                        y as i32 + 1 + offset.1,
                    )
                })?;
                if unsafe { CombineRgn(region.0, region.0, part.0, RGN_OR) } == 0 {
                    return None;
                }
            }
        }
        Some(region)
    }
}

struct Region(HRGN);
impl Region {
    fn new(handle: HRGN) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

fn with_hwnd(pet: &PetWindow, f: impl FnOnce(HWND)) {
    pet.window().with_winit_window(|window| {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                f(h.hwnd.get() as HWND);
            }
        }
    });
}

struct Shape {
    source: RgbaImage,
    last: RefCell<Option<(Geometry, Mask)>>,
    animation: Timer,
    native_window: Cell<HWND>,
}
impl Shape {
    fn update(&self, pet: &PetWindow) {
        with_hwnd(pet, |hwnd| {
            if self.native_window.get() != hwnd {
                if unsafe { SetWindowSubclass(hwnd, Some(frameless_proc), PET_FRAME_SUBCLASS, 0) }
                    == 0
                {
                    return;
                }
                self.native_window.set(hwnd);
                self.last.borrow_mut().take();
            }
            let Some(g) = Geometry::read(pet, hwnd) else {
                return;
            };
            if self
                .last
                .borrow()
                .as_ref()
                .is_some_and(|(old, _)| *old == g)
            {
                return;
            }
            let mask = Mask::new(&self.source, g);
            let Some(region) = mask.region(g.offset) else {
                return;
            };
            if unsafe { SetWindowRgn(hwnd, region.0, 1) } == 0 {
                return;
            }
            // Windows owns the successful region; our failed/temporary regions remain RAII-owned.
            std::mem::forget(region);
            *self.last.borrow_mut() = Some((g, mask));
            redraw_full_window(pet.window());
        });
    }
}

pub fn install(pet: &PetWindow) -> Result<()> {
    let pixels = pet.get_pet_picture().to_rgba8().ok_or("无法读取小猫轮廓")?;
    let source = RgbaImage::from_raw(pixels.width(), pixels.height(), pixels.as_bytes().to_vec())
        .ok_or("小猫轮廓尺寸无效")?;
    let shape = Rc::new(Shape {
        source,
        last: RefCell::new(None),
        animation: Timer::default(),
        native_window: Cell::new(std::ptr::null_mut()),
    });
    let state = shape.clone();
    let animal = pet.as_weak();
    pet.on_shape_changed(move || {
        if let Some(pet) = animal.upgrade() {
            state.update(&pet);
        }
    });
    let state = shape.clone();
    pet.on_hit_test(move |x, y| {
        state
            .last
            .borrow()
            .as_ref()
            .is_some_and(|(g, mask)| mask.contains(x * g.scale, y * g.scale))
    });
    let animal = pet.as_weak();
    pet.on_pose_changed(move || {
        let deadline = Instant::now() + Duration::from_millis(280);
        let state = Rc::downgrade(&shape);
        let animal = animal.clone();
        // No permanent polling: follow only the 170/240 ms pose animations.
        shape
            .animation
            .start(TimerMode::Repeated, Duration::from_millis(16), move || {
                if let Some(state) = state.upgrade() {
                    if let Some(pet) = animal.upgrade() {
                        state.update(&pet);
                    }
                    if Instant::now() >= deadline {
                        state.animation.stop();
                    }
                }
            });
    });
    Ok(())
}

pub fn window_event(pet: &PetWindow, event: &winit::event::WindowEvent) {
    match event {
        winit::event::WindowEvent::RedrawRequested => {
            pet.invoke_shape_changed();
            // The shaped HWND can lose pixels on move/show/frame changes while
            // softbuffer still reports a reused buffer. Repaint the small pet
            // surface on an actual redraw event; do not enqueue another frame.
            invalidate_full_window(pet.window());
        }
        winit::event::WindowEvent::Resized(_)
        | winit::event::WindowEvent::ScaleFactorChanged { .. } => {
            let animal = pet.as_weak();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(pet) = animal.upgrade() {
                    pet.invoke_shape_changed();
                }
            });
        }
        _ => {}
    }
}

// Read Windows' applied region and actual point routing, without moving the user's mouse.
fn check_native(pet: &PetWindow, native: &RgbaImage) -> Result<()> {
    let undecorated = pet.window().with_winit_window(|w| !w.is_decorated()) == Some(true);
    let mut result = Err("小猫原生窗口不可用".into());
    with_hwnd(pet, |hwnd| {
        result = (|| -> Result<()> {
            let g = Geometry::read(pet, hwnd).ok_or("无法读取窗口几何信息")?;
            let region =
                Region::new(unsafe { CreateRectRgn(0, 0, 0, 0) }).ok_or("无法创建测试区域")?;
            if unsafe { GetWindowRgn(hwnd, region.0) } <= 0 {
                return Err("Windows 未应用小猫轮廓区域".into());
            }
            let side = g.width.min(g.height);
            let left = g.x + (g.width - side) / 2.;
            let top = g.y + (g.height - side) / 2.;
            let sprite_point = |x: f32, y: f32| (left + x / 144. * side, top + y / 144. * side);
            let (body_x, body_y) = sprite_point(72., 65.);
            let (ear_gap_x, ear_gap_y) = sprite_point(72., 20.);
            let checks = [
                ("body", body_x, body_y, true),
                ("ear-gap", ear_gap_x, ear_gap_y, g.drop),
                ("left-empty", 1., 55., false),
                ("right-empty", 150., 55., false),
                ("count", 116., 18., g.count > 0 || g.drop),
                ("wide-count", 137., 18., g.count > 99),
                ("count-corner", 105., 6., g.drop),
                ("status", 80., 119., g.status),
                ("status-corner", 3., 104., false),
                ("drop-background", 16., 52., g.drop),
            ];
            for (name, x, y, expected) in checks {
                let px = (x * g.scale) as i32;
                let py = (y * g.scale) as i32;
                let in_region =
                    unsafe { PtInRegion(region.0, px + g.offset.0, py + g.offset.1) != 0 };
                let mut point = POINT { x: px, y: py };
                if unsafe { ClientToScreen(hwnd, &mut point) } == 0 {
                    return Err("测试点无法转换为桌面坐标".into());
                }
                let target = unsafe { WindowFromPoint(point) };
                let routes_to_pet = target == hwnd;
                let hover = pet.invoke_hit_test(x, y);
                if expected && in_region && hover && !routes_to_pet {
                    let mut class = [0u16; 128];
                    let length =
                        unsafe { GetClassNameW(target, class.as_mut_ptr(), class.len() as i32) };
                    return Err(format!(
                        "INCONCLUSIVE: {name} occluded by {}",
                        String::from_utf16_lossy(&class[..length.max(0) as usize])
                    )
                    .into());
                }
                if in_region != expected || routes_to_pet != expected || hover != expected {
                    return Err(format!("{name}: expected={expected} region={in_region} routing={routes_to_pet} hover={hover}").into());
                }
            }
            // Winit deliberately retains WS_CAPTION for Aero functionality and removes
            // the drawn non-client area in WM_NCCALCSIZE; the style bits alone lie.
            if !undecorated || g.offset.0 != 0 || g.offset.1.abs() > 1 {
                return Err(format!(
                    "小猫窗口边框异常：undecorated={undecorated} offset={:?}",
                    g.offset
                )
                .into());
            }
            let source = pet.get_pet_picture().to_rgba8().ok_or("测试图片无法读取")?;
            let colors: Vec<_> = [(34., 24.), (107., 24.), (72., 50.), (72., 108.)]
                .into_iter()
                .map(|(sx, sy)| {
                    let (x, y) = sprite_point(sx, sy);
                    let color = native.get_pixel((x * g.scale) as u32, (y * g.scale) as u32);
                    let expected =
                        source.as_slice()[(sy as u32 * source.width() + sx as u32) as usize];
                    (
                        [color[0], color[1], color[2]],
                        [expected.r, expected.g, expected.b],
                    )
                })
                .collect();
            for (actual, expected) in colors {
                if actual
                    .into_iter()
                    .zip(expected)
                    .any(|(a, e)| a.abs_diff(e) > 24)
                {
                    return Err(format!(
                        "小猫实际像素未正确重绘：actual={actual:?} expected={expected:?}"
                    )
                    .into());
                }
            }
            if g.count > 0 {
                // Check the digits themselves: sampling the almost-white badge
                // background would miss a white system caption covering its text.
                let expected = pet.window().take_snapshot()?;
                let mut checked = 0;
                let mut mismatched = 0;
                for y in (6. * g.scale).ceil() as u32..(30. * g.scale).floor() as u32 {
                    for x in (105. * g.scale).ceil() as u32
                        ..((105. + pet.get_count_width()) * g.scale).floor() as u32
                    {
                        let pixel = expected.as_slice()[(y * expected.width() + x) as usize];
                        if pixel.a == 255 && pixel.r < 180 && pixel.g < 170 && pixel.b < 160 {
                            checked += 1;
                            let color = native.get_pixel(x, y);
                            let actual = [color[0], color[1], color[2]];
                            if actual
                                .into_iter()
                                .zip([pixel.r, pixel.g, pixel.b])
                                .any(|(a, e)| a.abs_diff(e) > 24)
                            {
                                mismatched += 1;
                            }
                        }
                    }
                }
                if checked == 0 || mismatched > 2 {
                    return Err(format!(
                        "数字徽标被覆盖：checked={checked} mismatched={mismatched}"
                    )
                    .into());
                }
            }
            Ok(())
        })();
    });
    result
}

fn check_drag(pet: &PetWindow) -> Result<()> {
    use slint::platform::{PointerEventButton, WindowEvent};
    let before = pet.window().position();
    let start = slint::LogicalPosition::new(60., 50.);
    let end = slint::LogicalPosition::new(70., 58.);
    pet.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    pet.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    pet.window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
    pet.window().dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });
    pet.window().dispatch_event(WindowEvent::PointerExited);
    let after = pet.window().position();
    let scale = pet.window().scale_factor();
    if after.x - before.x != (10. * scale) as i32 || after.y - before.y != (8. * scale) as i32 {
        return Err(format!("缩小后的拖动位置异常：before={before:?} after={after:?}").into());
    }
    Ok(())
}

fn native_snapshot(pet: &PetWindow, path: &Path) -> Result<RgbaImage> {
    let mut result = Err("原生窗口不可用".into());
    with_hwnd(pet, |hwnd| {
        result = (|| -> Result<RgbaImage> {
            let size = pet.window().size();
            let mut origin = POINT { x: 0, y: 0 };
            let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = size.width as i32;
            info.bmiHeader.biHeight = -(size.height as i32);
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut bits = std::ptr::null_mut();
            let screen = unsafe { GetDC(std::ptr::null_mut()) };
            let memory = unsafe { CreateCompatibleDC(screen) };
            let bitmap = unsafe {
                CreateDIBSection(
                    screen,
                    &info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    std::ptr::null_mut(),
                    0,
                )
            };
            let mut bytes = None;
            unsafe {
                if !screen.is_null()
                    && !memory.is_null()
                    && !bitmap.is_null()
                    && !bits.is_null()
                    && ClientToScreen(hwnd, &mut origin) != 0
                {
                    let old = SelectObject(memory, bitmap);
                    if BitBlt(
                        memory,
                        0,
                        0,
                        size.width as i32,
                        size.height as i32,
                        screen,
                        origin.x,
                        origin.y,
                        SRCCOPY,
                    ) != 0
                    {
                        GdiFlush();
                        bytes = Some(
                            std::slice::from_raw_parts(
                                bits as *const u8,
                                (size.width * size.height * 4) as usize,
                            )
                            .to_vec(),
                        );
                    }
                    SelectObject(memory, old);
                }
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !memory.is_null() {
                    DeleteDC(memory);
                }
                if !screen.is_null() {
                    ReleaseDC(std::ptr::null_mut(), screen);
                }
            }
            let mut bytes = bytes.ok_or("无法截取原生窗口像素")?;
            let reference = pet.window().take_snapshot()?;
            for (pixel, mask) in bytes.chunks_exact_mut(4).zip(reference.as_slice()) {
                pixel.swap(0, 2);
                pixel[3] = if mask.a == 0 { 0 } else { 255 };
                if mask.a == 0 {
                    pixel.fill(0);
                }
            }
            let native =
                RgbaImage::from_raw(size.width, size.height, bytes).ok_or("原生快照尺寸无效")?;
            native.save(path)?;
            Ok(native)
        })();
    });
    result
}

pub fn check(ui: &PocketWindow, pet: &PetWindow, out: PathBuf) {
    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    // Leave room above the pet so the panel and rail cannot cover the routing points.
    pet.set_count(0);
    pet.set_due_count(0);
    pet.set_toast("".into());
    pet.set_nudge_text("".into());
    pet.set_can_undo(false);
    for stage in 0..15u64 {
        let animal = pet.as_weak();
        let view = ui.as_weak();
        let errors = failures.clone();
        Timer::single_shot(Duration::from_millis(100 + stage * 500), move || {
            if let Some(pet) = animal.upgrade() {
                if stage == 0 {
                    // Keep this disposable test window away from bottom-right
                    // system/app notification overlays. Never move the installed pet.
                    let p = pet.window().position();
                    let scale = pet.window().scale_factor();
                    let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
                    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                    if unsafe {
                        GetMonitorInfoW(
                            MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST),
                            &mut monitor,
                        )
                    } != 0
                    {
                        pet.window().set_position(PhysicalPosition::new(
                            monitor.rcWork.left + (48. * scale) as i32,
                            monitor.rcWork.top + (64. * scale) as i32,
                        ));
                    }
                }
                if stage == 3 {
                    if let Err(e) = check_drag(&pet) {
                        errors.borrow_mut().push(e.to_string());
                    }
                }
                pet.set_count(match stage {
                    1 => 1,
                    2 => 12,
                    3 | 8..=14 => 123,
                    _ => 0,
                });
                pet.set_toast(if stage == 4 {
                    "边界检查".into()
                } else {
                    "".into()
                });
                pet.set_drop_hover(stage == 5);
                pet.set_happy(stage == 6);
                pet.set_celebrating(stage == 7);
                pet.invoke_shape_changed();
                match stage {
                    8 | 9 => with_hwnd(&pet, |hwnd| unsafe {
                        SendMessageW(hwnd, WM_NCACTIVATE, usize::from(stage == 8), 0);
                    }),
                    10 => with_hwnd(&pet, |hwnd| unsafe {
                        SendMessageW(hwnd, WM_NCPAINT, 1, 0);
                    }),
                    11 => with_hwnd(&pet, |hwnd| unsafe {
                        RedrawWindow(
                            hwnd,
                            std::ptr::null(),
                            std::ptr::null_mut(),
                            RDW_INVALIDATE | RDW_FRAME | RDW_UPDATENOW,
                        );
                    }),
                    12 => {
                        let _ = pet.hide();
                        let _ = pet.show();
                    }
                    13 => {
                        use slint::platform::{PointerEventButton, WindowEvent};
                        pet.window().with_winit_window(|w| w.focus_window());
                        let position = slint::LogicalPosition::new(60., 50.);
                        pet.window()
                            .dispatch_event(WindowEvent::PointerMoved { position });
                        pet.window().dispatch_event(WindowEvent::PointerPressed {
                            position,
                            button: PointerEventButton::Left,
                        });
                        pet.window().dispatch_event(WindowEvent::PointerReleased {
                            position,
                            button: PointerEventButton::Left,
                        });
                        pet.window().dispatch_event(WindowEvent::PointerExited);
                        if !view.upgrade().is_some_and(|ui| ui.get_revealed()) {
                            errors.borrow_mut().push("点击小猫未打开面板".into());
                        }
                    }
                    14 => {
                        if let Some(ui) = view.upgrade() {
                            ui.invoke_dismiss();
                            if ui.get_revealed() {
                                errors.borrow_mut().push("面板未正常收起".into());
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
        let animal = pet.as_weak();
        let output = out.clone();
        let errors = failures.clone();
        Timer::single_shot(Duration::from_millis(450 + stage * 500), move || {
            if let Some(pet) = animal.upgrade() {
                if let Err(e) =
                    native_snapshot(&pet, &output.join(format!("pet-native-{stage}.png")))
                        .and_then(|native| check_native(&pet, &native))
                        .and_then(|_| {
                            snapshot_window(pet.window(), &output.join(format!("pet-{stage}.png")))
                        })
                {
                    errors.borrow_mut().push(format!("stage={stage}: {e}"));
                }
            } else {
                errors
                    .borrow_mut()
                    .push(format!("stage={stage}: 小猫窗口丢失"));
            }
            if stage == 14 {
                let errors = errors.borrow();
                let report = if errors.is_empty() {
                    "PASS: 15 poses/control/frame states, native silhouette region, transparent point routing, synchronized hover, frameless window, native ear/body/badge pixels, pointer drag/click, activation/frame repaint, hide/show, panel open/close".into()
                } else {
                    let status = if errors.iter().all(|e| e.contains("INCONCLUSIVE:")) {
                        "INCONCLUSIVE"
                    } else {
                        "FAIL"
                    };
                    format!("{status}: {}", errors.join("\n"))
                };
                let _ = fs::write(output.join("pet-boundary-check.txt"), report);
                let _ = slint::quit_event_loop();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> RgbaImage {
        let tree = resvg::usvg::Tree::from_data(
            include_bytes!("../ui/cat.svg"),
            &resvg::usvg::Options::default(),
        )
        .unwrap();
        let mut pixels = resvg::tiny_skia::Pixmap::new(144, 144).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixels.as_mut(),
        );
        RgbaImage::from_raw(144, 144, pixels.data().to_vec()).unwrap()
    }

    fn geometry(scale: f32) -> Geometry {
        Geometry {
            x: 14.25,
            y: 5.25,
            width: 96.,
            height: 96.,
            scale,
            count: 0,
            status: false,
            drop: false,
            window_width: (160. * scale).round() as u32,
            window_height: (138. * scale).round() as u32,
            offset: (0, 0),
        }
    }

    #[test]
    fn actual_cat_contour_excludes_transparent_gaps_at_five_dpi_scales() {
        let source = source();
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for (width, height, y) in [(96., 96., 5.25), (96., 96., 0.), (105., 90., 5.25)] {
                let g = Geometry {
                    width,
                    height,
                    y,
                    ..geometry(scale)
                };
                let mask = Mask::new(&source, g);
                let (x, y, w, h) = g.sprite_rect(&source);
                for (sx, sy, expected) in [
                    (36., 26., true),
                    (72., 65., true),
                    (72., 108., true),
                    (72., 20., false),
                    (15., 50., false),
                    (130., 60., false),
                ] {
                    assert_eq!(
                        mask.contains((x + sx / 144. * w) * scale, (y + sy / 144. * h) * scale),
                        expected,
                        "scale={scale} pose={width}x{height} source=({sx},{sy})"
                    );
                }
            }
        }
    }

    #[test]
    fn hidden_controls_and_rounded_corners_do_not_capture_clicks() {
        let source = source();
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for count in [0, 1, 12, 123] {
                for status in [false, true] {
                    let mask = Mask::new(
                        &source,
                        Geometry {
                            count,
                            status,
                            ..geometry(scale)
                        },
                    );
                    for (x, y, expected) in [
                        (116., 18., count > 0),
                        (137., 18., count > 99),
                        (80., 119., status),
                        (105., 6., false),
                        (3., 104., false),
                        (1., 55., false),
                        (150., 55., false),
                    ] {
                        assert_eq!(
                            mask.contains(x * scale, y * scale),
                            expected,
                            "scale={scale} count={count} status={status} point=({x},{y})"
                        );
                    }
                }
            }
        }
    }
}
