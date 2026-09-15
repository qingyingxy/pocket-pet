use super::*;
use slint::Model;

// Existing entries keep their order; new records follow the focused entry.
fn reconcile(old: &[i32], incoming: &[i32], focus: Option<i32>) -> (Vec<i32>, usize) {
    let previous = focus
        .and_then(|id| old.iter().position(|x| *x == id))
        .unwrap_or(0);
    let mut order: Vec<_> = old
        .iter()
        .copied()
        .filter(|id| incoming.contains(id))
        .collect();
    let additions: Vec<_> = incoming
        .iter()
        .copied()
        .filter(|id| !order.contains(id))
        .collect();
    let at = focus
        .and_then(|id| order.iter().position(|x| *x == id))
        .map(|i| i + 1)
        .unwrap_or(order.len());
    order.splice(at..at, additions);
    let selected = focus
        .and_then(|id| order.iter().position(|x| *x == id))
        .unwrap_or(previous.min(order.len().saturating_sub(1)));
    (order, selected)
}

fn position(rail: &RailWindow, pet: &PetWindow, prefer_left: bool) {
    let p = pet.window().position();
    let scale = pet.window().scale_factor();
    let width = (272. * scale).round() as i32;
    let height = (314. * scale).round() as i32;
    let gap = (6. * scale).round() as i32;
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
        let left = p.x - width - gap;
        let right = p.x + pet.window().size().width as i32 + gap;
        let use_left = if prefer_left {
            left >= r.left || right + width > r.right
        } else {
            right + width > r.right && left >= r.left
        };
        rail.set_left_side(use_left);
        let x = if use_left { left } else { right };
        let y = p.y + pet.window().size().height as i32 / 2 - height / 2;
        rail.window().set_position(PhysicalPosition::new(
            x.clamp(r.left, (r.right - width).max(r.left)),
            y.clamp(r.top, (r.bottom - height).max(r.top)),
        ));
    }
}

pub fn install(ui: &PocketWindow, pet: &PetWindow) -> Result<RailWindow> {
    let rail = RailWindow::new()?;
    // Native entry is independent of movement: winit deduplicates identical positions.
    pet.set_native_hover_events(
        !std::env::args().any(|a| a == "--snapshot")
            || std::env::args().any(|a| a == "--native-hover"),
    );
    {
        let weak = rail.as_weak();
        rail.on_shape_changed(move || {
            if let Some(rail) = weak.upgrade() {
                update_shape(&rail);
            }
        });
        let weak = rail.as_weak();
        rail.window().on_winit_window_event(move |_, event| {
            if let Some(rail) = weak.upgrade() {
                match event {
                    winit::event::WindowEvent::CursorEntered { .. } => {
                        rail.invoke_preview_hover(true)
                    }
                    winit::event::WindowEvent::CursorLeft { .. } => {
                        rail.invoke_preview_hover(false)
                    }
                    _ => {}
                }
            }
            if matches!(
                event,
                winit::event::WindowEvent::Resized(_)
                    | winit::event::WindowEvent::ScaleFactorChanged { .. }
            ) {
                if let Some(rail) = weak.upgrade() {
                    update_shape(&rail);
                }
            }
            EventResult::Propagate
        });
    }
    let preview = Rc::new(Preview {
        ui: ui.as_weak(),
        rail: rail.as_weak(),
        pet: pet.as_weak(),
        pet_over: Cell::new(false),
        rail_over: Cell::new(false),
        blocked: Cell::new(false),
        detail_open: Cell::new(false),
        dragging: Cell::new(false),
        visible: Cell::new(false),
        delay: Timer::default(),
        clock: Timer::default(),
    });
    {
        let state = preview.clone();
        pet.on_preview_hover(move |over| {
            if state.pet_over.replace(over) != over {
                state.trace(if over { "pet-in" } else { "pet-out" });
                state.pointer_changed();
            }
        });
        let state = preview.clone();
        rail.on_preview_hover(move |over| {
            if state.rail_over.replace(over) != over {
                state.trace(if over { "rail-in" } else { "rail-out" });
                state.pointer_changed();
            }
        });
        let state = preview.clone();
        pet.on_preview_drag(move |dragging| {
            state.dragging.set(dragging);
            state.update();
        });
    }
    {
        let animal = pet.as_weak();
        let diagnostic = preview.clone();
        pet.window().on_winit_window_event(move |window, event| {
            if let Some(pet) = animal.upgrade() {
                if !pet.get_native_hover_events() {
                    return EventResult::Propagate;
                }
                match event {
                    winit::event::WindowEvent::CursorEntered { .. } => {
                        diagnostic.trace("native-enter");
                        if let Some(over) = pet_cursor_inside(&pet) {
                            pet.invoke_preview_hover(over);
                        }
                    }
                    winit::event::WindowEvent::CursorMoved { position, .. } => {
                        let p = position.to_logical::<f32>(f64::from(window.scale_factor()));
                        pet.invoke_preview_hover(pet_hover_area(&pet, p.x, p.y));
                    }
                    winit::event::WindowEvent::CursorLeft { .. } => {
                        diagnostic.trace("native-leave");
                        pet.invoke_preview_hover(false)
                    }
                    winit::event::WindowEvent::MouseInput {
                        state: winit::event::ElementState::Released,
                        ..
                    } => pet.invoke_preview_drag(false),
                    _ => {}
                }
            }
            EventResult::Propagate
        });
    }
    let preference = Rc::new(Cell::new(true));
    let order = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let weak = rail.as_weak();
        let animal = pet.as_weak();
        let preference = preference.clone();
        pet.on_rail_place(move || {
            if let (Some(rail), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                position(&rail, &pet, preference.get());
            }
        });
    }
    {
        let weak = rail.as_weak();
        let animal = pet.as_weak();
        let order = order.clone();
        pet.on_rail_updated(move || {
            let (Some(rail), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let incoming: Vec<_> = pet.get_rail_notes().iter().collect();
            let ids: Vec<_> = incoming.iter().map(|n| n.id).collect();
            let old = order.borrow().clone();
            let focus = old.get(rail.get_current() as usize).copied();
            let (next, selected) = reconcile(&old, &ids, focus);
            if rail.get_busy_id() >= 0
                && !incoming
                    .iter()
                    .any(|n| n.id == rail.get_busy_id() && n.completing)
            {
                clear_dust(&rail);
            }
            let entries: Vec<_> = next
                .iter()
                .filter_map(|id| incoming.iter().find(|n| n.id == *id).cloned())
                .collect();
            if !old.is_empty() && next.is_empty() {
                rail.set_empty_message(true);
                let weak = rail.as_weak();
                Timer::single_shot(Duration::from_secs(3), move || {
                    if let Some(rail) = weak.upgrade() {
                        rail.set_empty_message(false);
                    }
                });
            } else if !next.is_empty() {
                rail.set_empty_message(false);
            }
            *order.borrow_mut() = next;
            rail.set_entries(ModelRc::new(VecModel::from(entries)));
            rail.set_current(selected as i32);
            rail.set_can_undo(pet.get_rail_undo());
        });
    }
    {
        let weak = rail.as_weak();
        let last = Rc::new(Cell::new(Instant::now() - Duration::from_secs(1)));
        rail.on_step(move |delta| {
            let Some(rail) = weak.upgrade() else {
                return;
            };
            if rail.get_busy_id() >= 0 || last.get().elapsed() < Duration::from_millis(160) {
                return;
            }
            rail.set_menu_open(false);
            last.set(Instant::now());
            let max = rail.get_entries().row_count().saturating_sub(1) as i32;
            rail.set_current((rail.get_current() + delta).clamp(0, max));
        });
    }
    {
        let weak = rail.as_weak();
        let animal = pet.as_weak();
        let preference = preference.clone();
        rail.on_flip(move || {
            if let (Some(rail), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                preference.set(!rail.get_left_side());
                position(&rail, &pet, preference.get());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        rail.on_detail(move |id| {
            if let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) {
                ui.set_expanded_id(-1);
                ui.invoke_edit(id);
                reveal(&ui, &pet);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let view = rail.as_weak();
        rail.on_undo(move || {
            if let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) {
                ui.invoke_undo_complete();
                clear_dust(&rail);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let view = rail.as_weak();
        rail.on_complete(move |id| {
            let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) else {
                return;
            };
            if rail.get_busy_id() >= 0 {
                return;
            }
            let visual = rail
                .window()
                .take_snapshot()
                .map_err(Into::into)
                .and_then(|frame| {
                    capture_frame(
                        frame,
                        rail.window().scale_factor(),
                        [16., 119., 240., 76., 46., 268.],
                        id as u32,
                    )
                });
            ui.invoke_toggle(id);
            // The durable completion must succeed before any visual is removed.
            if !rail
                .get_entries()
                .iter()
                .any(|n| n.id == id && n.completing)
            {
                return;
            }
            if let Ok(v) = visual {
                rail.set_backdrop(v.backdrop);
                rail.set_layers(v.layers);
                rail.set_busy_id(id);
                rail.set_dust_running(false);
                let weak = rail.as_weak();
                Timer::single_shot(Duration::from_millis(16), move || {
                    if let Some(rail) = weak.upgrade() {
                        if rail.get_busy_id() == id {
                            rail.set_dust_running(true);
                        }
                    }
                });
                let weak = rail.as_weak();
                Timer::single_shot(Duration::from_millis(1450), move || {
                    if let Some(rail) = weak.upgrade() {
                        if rail.get_busy_id() == id {
                            clear_dust(&rail);
                        }
                    }
                });
            }
        });
    }
    {
        let state = preview.clone();
        ui.on_rail_status(move || {
            state.status();
        });
    }
    {
        let weak = ui.as_weak();
        let view = rail.as_weak();
        let animal = pet.as_weak();
        pet.on_rail_check(move || {
            if let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) {
                if let Some(pet) = animal.upgrade() {
                    check_hover(&ui, &rail, &pet);
                }
            }
        });
    }
    rail.show()?;
    // Preview never activates the application or steals the editor's keyboard focus.
    with_hwnd(&rail, |hwnd| unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            style | WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize,
        );
    });
    rail.hide()?;
    pet.invoke_rail_updated();
    pet.invoke_rail_place();
    update_shape(&rail);
    if std::env::args().any(|a| a == "--snapshot") {
        let weak = rail.as_weak();
        Timer::single_shot(Duration::from_millis(650), move || {
            if let Some(rail) = weak.upgrade() {
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
                let _ = snapshot_window(rail.window(), &out.join("side-rail.png"));
            }
        });
    }
    Ok(rail)
}
fn clear_dust(rail: &RailWindow) {
    rail.set_busy_id(-1);
    rail.set_dust_running(false);
    rail.set_layers(ModelRc::default());
    rail.set_backdrop(Image::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insert_without_displacing_focus_and_advance_on_completion() {
        assert_eq!(
            reconcile(&[1, 2, 3], &[1, 2, 3, 4, 5], Some(2)),
            (vec![1, 2, 4, 5, 3], 1)
        );
        assert_eq!(
            reconcile(&[1, 2, 4, 5, 3], &[1, 4, 5, 3], Some(2)),
            (vec![1, 4, 5, 3], 1)
        );
        assert_eq!(reconcile(&[1], &[], Some(1)), (vec![], 0));
        assert_eq!(reconcile(&[], &[7], None), (vec![7], 0));
    }
}

fn refresh_times(rail: &RailWindow) {
    let entries = rail.get_entries();
    if let Some(model) = entries.as_any().downcast_ref::<VecModel<RailCard>>() {
        for i in 0..model.row_count() {
            if let Some(mut row) = model.row_data(i) {
                let added = added_time::labels(row.created.parse().ok(), reminders::now()).0;
                if row.added != added {
                    row.added = added.into();
                    model.set_row_data(i, row);
                }
            }
        }
    }
}

fn check(ui: &PocketWindow, rail: &RailWindow) {
    hide(ui);
    rail.invoke_preview_hover(true);
    let weak = ui.as_weak();
    let view = rail.as_weak();
    Timer::single_shot(Duration::from_millis(250), move || {
        let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) else {
            return;
        };
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
        rail.set_current(0);
        rail.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(100., 145.),
                delta_x: 0.,
                delta_y: -120.,
            });
        let scroll_ok = rail.get_current() == 1;
        let view = rail.as_weak();
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_millis(220), move || {
            let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) else {
                return;
            };
            let _ = snapshot_window(rail.window(), &out.join("side-rail.png"));
            let Some(row) = rail.get_entries().row_data(rail.get_current() as usize) else {
                return;
            };
            let id = row.id;
            update_shape(&rail);
            let region_ok = check_shape(&rail);
            rail.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(235., 155.),
                });
            rail.window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                    position: slint::LogicalPosition::new(235., 155.),
                    button: slint::platform::PointerEventButton::Left,
                });
            rail.window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(235., 155.),
                    button: slint::platform::PointerEventButton::Left,
                });
            let pixels_ok =
                rail.get_busy_id() == id && rail.get_layers().row_count() == snap::LAYERS;
            let view = rail.as_weak();
            let path = out.clone();
            Timer::single_shot(Duration::from_millis(500), move || {
                if let Some(rail) = view.upgrade() {
                    let _ = snapshot_window(rail.window(), &path.join("side-rail-dust.png"));
                }
            });
            let view = rail.as_weak();
            let weak = ui.as_weak();
            Timer::single_shot(Duration::from_millis(1600), move || {
                let (Some(ui), Some(rail)) = (weak.upgrade(), view.upgrade()) else {
                    return;
                };
                let removed = !rail.get_entries().iter().any(|n| n.id == id)
                    && rail.get_busy_id() < 0
                    && rail.get_layers().row_count() == 0;
                rail.invoke_undo();
                let restored = rail.get_entries().iter().any(|n| n.id == id);
                let draft = ui.get_draft();
                rail.invoke_detail(id);
                let detail_ok =
                    ui.get_revealed() && ui.get_expanded_id() == id && ui.get_draft() == draft;
                let result = if region_ok
                    && scroll_ok
                    && pixels_ok
                    && removed
                    && restored
                    && detail_ok
                {
                    "PASS: native gap exclusion, wheel navigation, pixel dissolve, advance, buffer release, undo and detail preserve draft".into()
                } else {
                    format!("FAIL: region={region_ok} scroll={scroll_ok} pixels={pixels_ok} removed={removed} restored={restored} detail={detail_ok}")
                };
                let _ = fs::write(out.join("side-rail-check.txt"), result);
                let _ = slint::quit_event_loop();
            });
        });
    });
}

// An OS window region excludes transparent gaps from both drawing and hit testing.
// Windows owns the region after SetWindowRgn succeeds; temporary regions stay ours.
fn with_hwnd(rail: &RailWindow, f: impl FnOnce(HWND)) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    rail.window().with_winit_window(|window| {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                f(h.hwnd.get() as HWND);
            }
        }
    });
}
fn update_shape(rail: &RailWindow) {
    let scale = rail.window().scale_factor();
    with_hwnd(rail, |hwnd| unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        if region.is_null() {
            return;
        }
        let add = |x: f32, y: f32, w: f32, h: f32, r: f32| {
            let part = CreateRoundRectRgn(
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                ((x + w) * scale).round() as i32 + 1,
                ((y + h) * scale).round() as i32 + 1,
                (2. * r * scale).round() as i32,
                (2. * r * scale).round() as i32,
            );
            if !part.is_null() {
                CombineRgn(region, region, part, RGN_OR);
                DeleteObject(part);
            }
        };
        let count = rail.get_entries().row_count() as i32;
        for offset in -1..=1 {
            let index = rail.get_current() + offset;
            if index >= 0 && index < count {
                add(
                    if offset == 0 { 16. } else { 24. },
                    119. + offset as f32 * 82.,
                    if offset == 0 { 240. } else { 224. },
                    76.,
                    14.,
                );
            }
        }
        if count == 0 && rail.get_empty_message() {
            add(16., 119., 240., 76., 14.);
        }
        if rail.get_can_undo() {
            add(84., 284., 104., 26., 13.);
        }
        if rail.get_menu_open() {
            add(136., 201., 120., 72., 12.);
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
    });
}
fn check_shape(rail: &RailWindow) -> bool {
    let result = Cell::new(false);
    let scale = rail.window().scale_factor();
    with_hwnd(rail, |hwnd| unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        if region.is_null() {
            return;
        }
        if GetWindowRgn(hwnd, region) != 0 {
            let inside = |x: f32, y: f32| {
                PtInRegion(
                    region,
                    (x * scale).round() as i32,
                    (y * scale).round() as i32,
                ) != 0
            };
            result.set(inside(100., 155.) && !inside(100., 116.) && !inside(5., 20.));
        }
        DeleteObject(region);
    });
    result.get()
}

struct Preview {
    ui: slint::Weak<PocketWindow>,
    rail: slint::Weak<RailWindow>,
    pet: slint::Weak<PetWindow>,
    pet_over: Cell<bool>,
    rail_over: Cell<bool>,
    blocked: Cell<bool>,
    detail_open: Cell<bool>,
    dragging: Cell<bool>,
    visible: Cell<bool>,
    delay: Timer,
    clock: Timer,
}
// Windows can discard hidden-window pixels while Slint retains its partial
// rendering cache. Invalidate the entire surface only on reveal; a plain
// request_redraw would restore only changed controls.
fn redraw_revealed_rail(rail: &RailWindow) {
    use i_slint_core::lengths::{LogicalPoint, LogicalRect, LogicalSize};
    let window = rail.window();
    let size = window.size();
    let scale = window.scale_factor();
    let bounds = LogicalRect::new(
        LogicalPoint::new(0., 0.),
        LogicalSize::new(size.width as f32 / scale, size.height as f32 / scale),
    );
    i_slint_core::window::WindowInner::from_pub(window)
        .window_adapter()
        .renderer()
        .mark_dirty_region(bounds.into());
    window.request_redraw();
}
impl Preview {
    fn trace(&self, event: &str) {
        if !std::env::args().any(|a| a == "--trace-hover" || a == "--snapshot") {
            return;
        }
        let Some(rail) = self.rail.upgrade() else {
            return;
        };
        let detail = self.ui.upgrade().is_some_and(|ui| ui.get_revealed());
        let snapshot = std::env::args().any(|a| a == "--snapshot");
        let dir = if snapshot {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output")
        } else {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("PocketPetSlintPreview")
        };
        let path = dir.join("hover-diagnostic.log");
        if fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
            let _ = fs::write(&path, "");
        }
        use std::io::Write;
        if let Ok(mut log) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _=writeln!(log,"{} {event} pet={} rail={} blocked={} dragging={} detail={} visible={} native={} timer={}",reminders::now(),self.pet_over.get(),self.rail_over.get(),self.blocked.get(),self.dragging.get(),detail,self.visible.get(),native_visible(&rail),self.delay.running());
        }
    }
    fn hide(&self) {
        self.trace("hide-request");
        self.delay.stop();
        self.clock.stop();
        self.visible.set(false);
        self.rail_over.set(false);
        if let Some(rail) = self.rail.upgrade() {
            rail.set_preview_visible(false);
            rail.set_menu_open(false);
            let _ = rail.hide();
        }
    }
    fn pointer_changed(self: &Rc<Self>) {
        if !self.pet_over.get() && !self.rail_over.get() {
            self.blocked.set(false);
        }
        self.update();
    }
    fn status(self: &Rc<Self>) {
        self.trace("detail-status");
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if let Some(rail) = self.rail.upgrade() {
            rail.set_can_undo(ui.get_completion_undo());
        }
        let opened = ui.get_revealed();
        let previous = self.detail_open.replace(opened);
        if opened {
            self.blocked.set(true);
            self.hide();
            return;
        }
        if previous {
            self.blocked.set(self.pet_over.get());
            self.hide();
            return;
        }
        self.update();
    }
    fn update(self: &Rc<Self>) {
        self.trace("update");
        self.delay.stop();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_revealed()
            || self.dragging.get()
            || self
                .pet
                .upgrade()
                .is_some_and(|pet| pet.get_desktop_hidden())
        {
            self.hide();
            return;
        }
        if self.blocked.get() {
            return;
        }
        if self.visible.get() {
            if !self.pet_over.get() && !self.rail_over.get() {
                let state = self.clone();
                self.delay.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(400),
                    move || {
                        state.trace("close-timer-fired");
                        if !state.pet_over.get() && !state.rail_over.get() {
                            state.hide();
                        }
                    },
                );
            }
        } else if self.pet_over.get() {
            let state = self.clone();
            self.delay.start(
                TimerMode::SingleShot,
                Duration::from_millis(200),
                move || {
                    state.trace("open-timer-fired");
                    if !state.pet_over.get() || state.blocked.get() || state.dragging.get() {
                        return;
                    }
                    let (Some(ui), Some(rail), Some(pet)) = (
                        state.ui.upgrade(),
                        state.rail.upgrade(),
                        state.pet.upgrade(),
                    ) else {
                        return;
                    };
                    if ui.get_revealed()
                        || (rail.get_entries().row_count() == 0
                            && !rail.get_can_undo()
                            && !rail.get_empty_message())
                    {
                        return;
                    }
                    pet.invoke_rail_place();
                    refresh_times(&rail);
                    update_shape(&rail);
                    state.visible.set(true);
                    rail.set_preview_visible(true);
                    if rail.show().is_err() {
                        state.visible.set(false);
                        rail.set_preview_visible(false);
                        return;
                    }
                    update_shape(&rail);
                    redraw_revealed_rail(&rail);
                    let weak = rail.as_weak();
                    state
                        .clock
                        .start(TimerMode::Repeated, Duration::from_secs(60), move || {
                            if let Some(rail) = weak.upgrade() {
                                refresh_times(&rail);
                            }
                        });
                },
            );
        }
    }
}

fn check_hover(ui: &PocketWindow, rail: &RailWindow, pet: &PetWindow) {
    hide(ui);
    post_pet_pointer(&pet, false);
    rail.invoke_preview_hover(false);
    let passed = Rc::new(Cell::new(true));
    // Run the same callbacks as pointer entry/exit, with real Slint timers.
    for (ms, stage) in [
        (20, 0),
        (100, 1),
        (150, 11),
        (190, 11),
        (270, 2),
        (290, 3),
        (480, 4),
        (510, 5),
        (960, 6),
        (980, 0),
        (1230, 7),
        (1300, 8),
        (1560, 9),
        (1590, 0),
        (1840, 10),
    ] {
        let weak = ui.as_weak();
        let view = rail.as_weak();
        let animal = pet.as_weak();
        let passed = passed.clone();
        Timer::single_shot(Duration::from_millis(ms), move || {
            let (Some(ui), Some(rail), Some(pet)) =
                (weak.upgrade(), view.upgrade(), animal.upgrade())
            else {
                return;
            };
            let ok = match stage {
                0 => {
                    post_pet_pointer(&pet, true);
                    true
                }
                11 => {
                    pet.invoke_preview_hover(true);
                    true
                }
                1 => !(rail.get_preview_visible() && native_visible(&rail)),
                2 => rail.get_preview_visible() && native_visible(&rail),
                3 => {
                    post_pet_pointer(&pet, false);
                    true
                }
                4 => {
                    let keep = rail.get_preview_visible() && native_visible(&rail);
                    rail.invoke_preview_hover(true);
                    keep
                }
                5 => {
                    rail.invoke_preview_hover(false);
                    true
                }
                6 => !(rail.get_preview_visible() && native_visible(&rail)),
                7 => {
                    let shown = rail.get_preview_visible() && native_visible(&rail);
                    pet.invoke_open_record();
                    shown
                }
                8 => {
                    let hidden =
                        !(rail.get_preview_visible() && native_visible(&rail)) && ui.get_revealed();
                    ui.invoke_dismiss();
                    hidden
                }
                9 => {
                    let hidden = !(rail.get_preview_visible() && native_visible(&rail));
                    post_pet_pointer(&pet, false);
                    hidden
                }
                _ => rail.get_preview_visible() && native_visible(&rail),
            };
            let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
            use std::io::Write;
            if let Ok(mut log) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(out.join("hover-event-trace.txt"))
            {
                let _ = writeln!(
                    log,
                    "stage={stage} ok={ok} pet={} preview={} native={} detail={}",
                    pet.get_preview_hovered(),
                    rail.get_preview_visible(),
                    native_visible(&rail),
                    ui.get_revealed()
                );
            }
            passed.set(passed.get() && ok);
            if stage == 10 {
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
                let result = if passed.get() {
                    "PASS: 200ms hover open, gap grace, 400ms leave close, click detail, close suppression and re-entry"
                } else {
                    "FAIL: hover preview timing or detail suppression"
                };
                let _ = fs::write(out.join("hover-preview-check.txt"), result);
                check_cycles(&ui, &rail, &pet);
            }
        });
    }
}

fn native_visible(rail: &RailWindow) -> bool {
    let visible = Cell::new(false);
    with_hwnd(rail, |hwnd| unsafe {
        visible.set(IsWindowVisible(hwnd) != 0);
    });
    visible.get()
}

fn pet_hover_area(pet: &PetWindow, x: f32, y: f32) -> bool {
    (12.0..=154.0).contains(&x) && (0.0..=133.0).contains(&y)
        || pet.get_count() > 0 && (140.0..=176.0).contains(&x) && (8.0..=32.0).contains(&y)
        || pet.get_status_visible() && (6.0..=160.0).contains(&x) && (137.0..=167.0).contains(&y)
}

fn post_pet_pointer(pet: &PetWindow, inside: bool) {
    if !std::env::args().any(|a| a == "--native-hover") {
        pet.window().dispatch_event(if inside {
            slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(80., 60.),
            }
        } else {
            slint::platform::WindowEvent::PointerExited
        });
        return;
    }
    let p = pet.window().position();
    let scale = pet.window().scale_factor();
    unsafe {
        if inside {
            SetCursorPos(p.x + (80. * scale) as i32, p.y + (60. * scale) as i32);
        } else {
            let mut monitor: MONITORINFO = std::mem::zeroed();
            monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(
                MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) != 0
            {
                SetCursorPos(monitor.rcWork.left + 10, monitor.rcWork.top + 10);
            }
        }
    }
}
pub struct CursorRestore(Option<POINT>);
impl CursorRestore {
    pub fn new() -> Self {
        if !std::env::args().any(|a| a == "--native-hover") {
            return Self(None);
        }
        let mut point = POINT { x: 0, y: 0 };
        Self(unsafe { (GetCursorPos(&mut point) != 0).then_some(point) })
    }
}
impl Drop for CursorRestore {
    fn drop(&mut self) {
        if let Some(point) = self.0 {
            unsafe {
                SetCursorPos(point.x, point.y);
            }
        }
    }
}

fn pet_cursor_inside(pet: &PetWindow) -> Option<bool> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let result = Cell::new(None);
    pet.window().with_winit_window(|window| {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                unsafe {
                    let mut point = POINT { x: 0, y: 0 };
                    if GetCursorPos(&mut point) != 0
                        && ScreenToClient(h.hwnd.get() as HWND, &mut point) != 0
                    {
                        let scale = window.scale_factor() as f32;
                        result.set(Some(pet_hover_area(
                            pet,
                            point.x as f32 / scale,
                            point.y as f32 / scale,
                        )));
                    }
                }
            }
        }
    });
    result.get()
}
fn check_cycles(ui: &PocketWindow, rail: &RailWindow, pet: &PetWindow) {
    let passed = Rc::new(Cell::new(true));
    post_pet_pointer(pet, false);
    rail.invoke_preview_hover(false);
    for cycle in 0..10u64 {
        for (offset, action) in [(500, 0), (1000, 1), (1050, 2), (1800, 3)] {
            let weak = ui.as_weak();
            let view = rail.as_weak();
            let animal = pet.as_weak();
            let passed = passed.clone();
            Timer::single_shot(Duration::from_millis(cycle * 1500 + offset), move || {
                let (Some(ui), Some(rail), Some(pet)) =
                    (weak.upgrade(), view.upgrade(), animal.upgrade())
                else {
                    return;
                };
                let ok = match action {
                    0 => {
                        post_pet_pointer(&pet, true);
                        true
                    }
                    1 => rail.get_preview_visible() && native_visible(&rail),
                    2 => {
                        post_pet_pointer(&pet, false);
                        true
                    }
                    _ => !rail.get_preview_visible() && !native_visible(&rail),
                };
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
                use std::io::Write;
                if let Ok(mut log) = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(out.join("hover-cycles-trace.txt"))
                {
                    let _=writeln!(log,"cycle={cycle} action={action} ok={ok} actual_pointer_inside={:?} preview={} native={}",pet_cursor_inside(&pet),rail.get_preview_visible(),native_visible(&rail));
                }
                passed.set(passed.get() && ok);
                if cycle == 9 && action == 3 {
                    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
                    let _ = fs::write(
                        out.join("hover-cycles-check.txt"),
                        if passed.get() {
                            "PASS: ten consecutive same-position enter/leave cycles"
                        } else {
                            "FAIL: repeated native hover cycle"
                        },
                    );
                    check_quick(&ui, &rail, &pet);
                }
            });
        }
    }
}

fn check_quick(ui: &PocketWindow, rail: &RailWindow, pet: &PetWindow) {
    let passed = Rc::new(Cell::new(true));
    for (ms, action) in [
        (20, 0),
        (100, 1),
        (450, 2),
        (500, 0),
        (800, 3),
        (850, 4),
        (1150, 5),
    ] {
        let weak = ui.as_weak();
        let view = rail.as_weak();
        let animal = pet.as_weak();
        let passed = passed.clone();
        Timer::single_shot(Duration::from_millis(ms), move || {
            let (Some(ui), Some(rail), Some(pet)) =
                (weak.upgrade(), view.upgrade(), animal.upgrade())
            else {
                return;
            };
            let ok = match action {
                0 => {
                    post_pet_pointer(&pet, true);
                    true
                }
                1 => {
                    post_pet_pointer(&pet, false);
                    true
                }
                2 => !rail.get_preview_visible() && !native_visible(&rail),
                3 => {
                    let visible = rail.get_preview_visible();
                    pet.invoke_preview_drag(true);
                    visible
                }
                4 => {
                    let hidden = !rail.get_preview_visible() && !native_visible(&rail);
                    pet.invoke_preview_drag(false);
                    hidden
                }
                _ => rail.get_preview_visible() && native_visible(&rail),
            };
            passed.set(passed.get() && ok);
            if action == 5 {
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output");
                let _ = fs::write(
                    out.join("hover-quick-check.txt"),
                    if passed.get() {
                        "PASS: quick pass cancels opening, dragging hides preview and release restores hover"
                    } else {
                        "FAIL: quick pass or drag suppression"
                    },
                );
                check(&ui, &rail);
            }
        });
    }
}
