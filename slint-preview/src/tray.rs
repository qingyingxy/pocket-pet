use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    io,
    path::Path,
    ptr,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};

const CLASS: &str = "PocketPetSlint.Tray.v1";
const ACTIVATE: u32 = WM_APP + 31;
const CALLBACK: u32 = WM_APP + 32;
const STATE: u32 = WM_APP + 33;
const REQUEST_EXIT: u32 = WM_APP + 34;
const MUTEX_NAME: &str = "Local\\PocketPetSlint.Instance.v1";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Installation requests the normal save-and-exit path, never kills the process.
pub fn request_exit() -> io::Result<()> {
    unsafe {
        let hwnd = FindWindowW(wide(CLASS).as_ptr(), ptr::null());
        if hwnd.is_null() {
            return Ok(());
        }
        if PostMessageW(hwnd, REQUEST_EXIT, 0, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
pub struct Instance(HANDLE);
impl Instance {
    pub fn acquire() -> io::Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(ptr::null(), 0, wide(MUTEX_NAME).as_ptr());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                // A concurrent first launch may still be creating its tray window.
                for _ in 0..30 {
                    let hwnd = FindWindowW(wide(CLASS).as_ptr(), ptr::null());
                    if !hwnd.is_null() {
                        let mut pid = 0;
                        GetWindowThreadProcessId(hwnd, &mut pid);
                        AllowSetForegroundWindow(pid);
                        if PostMessageW(hwnd, ACTIVATE, 0, 0) != 0 {
                            return Ok(None);
                        }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                return Err(io::Error::other("程序已在运行，但暂时无法唤回，请稍后再试"));
            }
            Ok(Some(Self(handle)))
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub topmost: bool,
    pub show_done: bool,
    pub rail_left: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            topmost: true,
            show_done: false,
            rail_left: true,
        }
    }
}
impl Preferences {
    pub fn load(dir: &Path) -> io::Result<Self> {
        Self::load_with_recovery(dir).map(|loaded| loaded.value)
    }
    pub fn load_with_recovery(dir: &Path) -> io::Result<crate::storage::Loaded<Self>> {
        crate::storage::load_preferences(dir)
    }
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        crate::storage::save(dir, "preferences", self)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Show,
    ToggleVisibility,
    Record,
    ToggleTopmost,
    Exit,
}
type Handler = Arc<Mutex<Option<Box<dyn Fn(Action) + Send>>>>;
#[derive(Clone, Copy)]
pub struct Control(usize);
impl Control {
    pub fn state(self, visible: bool, topmost: bool) {
        unsafe {
            PostMessageW(
                self.0 as HWND,
                STATE,
                visible as usize | ((topmost as usize) << 1),
                0,
            );
        }
    }
    pub fn owner(self) -> isize {
        self.0 as isize
    }
}
pub struct Tray {
    control: Control,
    handler: Handler,
    pending: Arc<Mutex<bool>>,
    worker: Option<thread::JoinHandle<()>>,
}
struct Context {
    handler: Handler,
    pending: Arc<Mutex<bool>>,
    visible: Cell<bool>,
    topmost: Cell<bool>,
    icon: HICON,
    restart: u32,
}
impl Context {
    fn dispatch(&self, action: Action) {
        let handler = self.handler.lock().unwrap();
        if let Some(callback) = handler.as_ref() {
            callback(action);
        } else {
            *self.pending.lock().unwrap() = true;
        }
    }
}
impl Tray {
    pub fn new(topmost: bool) -> io::Result<Self> {
        let handler: Handler = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(false));
        let h = handler.clone();
        let p = pending.clone();
        let (send, recv) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let name = wide(CLASS);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                let _ = send.send(Err(io::Error::last_os_error()));
                return;
            }
            let icon = cat_icon();
            if icon.is_null() {
                let _ = send.send(Err(io::Error::last_os_error()));
                UnregisterClassW(name.as_ptr(), instance);
                return;
            }
            let mut context = Box::new(Context {
                handler: h,
                pending: p,
                visible: Cell::new(true),
                topmost: Cell::new(topmost),
                icon,
                restart: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            });
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                name.as_ptr(),
                wide("Pocket Pet Tray").as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                (&mut *context as *mut Context).cast(),
            );
            if hwnd.is_null() {
                let _ = send.send(Err(io::Error::last_os_error()));
                DestroyIcon(icon);
                UnregisterClassW(name.as_ptr(), instance);
                return;
            }
            if !add_icon(hwnd, icon) {
                let _ = send.send(Err(io::Error::other("无法添加托盘图标，未隐藏程序入口")));
                DestroyWindow(hwnd);
                DestroyIcon(icon);
                UnregisterClassW(name.as_ptr(), instance);
                return;
            }
            let _ = send.send(Ok(hwnd as usize));
            let mut msg = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let mut data = notification(hwnd, icon);
            Shell_NotifyIconW(NIM_DELETE, &mut data);
            DestroyIcon(icon);
            UnregisterClassW(name.as_ptr(), instance);
        });
        match recv.recv().map_err(io::Error::other)? {
            Ok(hwnd) => Ok(Self {
                control: Control(hwnd),
                handler,
                pending,
                worker: Some(worker),
            }),
            Err(e) => {
                let _ = worker.join();
                Err(e)
            }
        }
    }
    pub fn control(&self) -> Control {
        self.control
    }
    pub fn on_action(&self, callback: impl Fn(Action) + Send + 'static) {
        *self.handler.lock().unwrap() = Some(Box::new(callback));
        if std::mem::take(&mut *self.pending.lock().unwrap()) {
            unsafe {
                PostMessageW(self.control.0 as HWND, ACTIVATE, 0, 0);
            }
        }
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.control.0 as HWND, WM_CLOSE, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
unsafe fn notification(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = CALLBACK;
    data.hIcon = icon;
    let tip = wide("口袋宠物 · 点击找回，右键管理");
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}
unsafe fn add_icon(hwnd: HWND, icon: HICON) -> bool {
    Shell_NotifyIconW(NIM_ADD, &notification(hwnd, icon)) != 0
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
    let context = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Context;
    if context.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let context = &*context;
    if context.restart != 0 && msg == context.restart {
        add_icon(hwnd, context.icon);
        return 0;
    }
    match msg {
        REQUEST_EXIT => {
            context.dispatch(Action::Exit);
            0
        }
        ACTIVATE => {
            context.dispatch(Action::Show);
            0
        }
        STATE => {
            context.visible.set(w & 1 != 0);
            context.topmost.set(w & 2 != 0);
            0
        }
        CALLBACK => {
            match l as u32 {
                WM_LBUTTONUP => context.dispatch(Action::Show),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd, context),
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
unsafe fn show_menu(hwnd: HWND, context: &Context) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(
        menu,
        MF_STRING,
        1,
        wide(if context.visible.get() {
            "隐藏小猫（暂停桌面提示）"
        } else {
            "显示小猫"
        })
        .as_ptr(),
    );
    AppendMenuW(menu, MF_STRING, 2, wide("记一条").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    AppendMenuW(
        menu,
        MF_STRING
            | if context.topmost.get() {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            },
        3,
        wide("置顶显示").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING
            | if startup_enabled() {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            },
        4,
        wide("开机启动").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    AppendMenuW(menu, MF_STRING, 5, wide("退出").as_ptr());
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    let selected = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        ptr::null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);
    match selected {
        1 => context.dispatch(Action::ToggleVisibility),
        2 => context.dispatch(Action::Record),
        3 => context.dispatch(Action::ToggleTopmost),
        4 => {
            if let Err(error) = set_startup(!startup_enabled()) {
                MessageBoxW(
                    hwnd,
                    wide(&format!("无法修改开机启动：{error}")).as_ptr(),
                    wide("口袋宠物").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
        5 => context.dispatch(Action::Exit),
        _ => {}
    }
}
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "PocketPetSlint";
fn startup_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}
fn startup_enabled() -> bool {
    unsafe {
        let mut key = ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let mut bytes = 0;
        let result = RegQueryValueExW(
            key,
            wide(RUN_VALUE).as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        );
        RegCloseKey(key);
        result == ERROR_SUCCESS
    }
}
fn set_startup(enabled: bool) -> io::Result<()> {
    unsafe {
        let mut key = ptr::null_mut();
        let result = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            ptr::null(),
            0,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        );
        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let result = if enabled {
            let exe = std::env::current_exe();
            match exe {
                Ok(exe) => {
                    let command = wide(&startup_command(&exe));
                    RegSetValueExW(
                        key,
                        wide(RUN_VALUE).as_ptr(),
                        0,
                        REG_SZ,
                        command.as_ptr().cast(),
                        (command.len() * 2) as u32,
                    )
                }
                Err(e) => {
                    RegCloseKey(key);
                    return Err(e);
                }
            }
        } else {
            RegDeleteValueW(key, wide(RUN_VALUE).as_ptr())
        };
        RegCloseKey(key);
        if result == ERROR_SUCCESS || (!enabled && result == ERROR_FILE_NOT_FOUND) {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result as i32))
        }
    }
}
// Small original cat silhouette; no system-default application icon or image decoder.
unsafe fn cat_icon() -> HICON {
    let mut pixels = [0u8; 32 * 32 * 4];
    let mut mask = [255u8; 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let (dx, dy) = (x as i32 - 16, y as i32 - 18);
            let ears = (5..=13).contains(&x) && y >= 4 && y <= 15 && x <= y + 4
                || (19..=27).contains(&x) && y >= 4 && y <= 15 && 31 - x <= y + 4;
            if dx * dx * 9 + dy * dy * 13 <= 1300 || ears {
                let eye = y >= 16 && y <= 18 && (x == 11 || x == 21);
                let nose = (15..=17).contains(&x) && y == 21;
                let c = if eye || nose {
                    [55, 75, 112, 255]
                } else {
                    [148, 193, 242, 255]
                };
                let i = (y * 32 + x) * 4;
                pixels[i..i + 4].copy_from_slice(&c);
                mask[y * 4 + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    CreateIcon(
        GetModuleHandleW(ptr::null()),
        32,
        32,
        1,
        32,
        mask.as_ptr(),
        pixels.as_ptr(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn preferences_default_and_roundtrip_without_touching_notes() {
        let dir = std::env::temp_dir().join(format!("pocket-tray-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("state.json"), b"keep notes").unwrap();
        let defaults = Preferences::load(&dir).unwrap();
        assert!(defaults.topmost);
        assert!(!defaults.show_done);
        Preferences {
            topmost: false,
            show_done: true,
            rail_left: false,
        }
        .save(&dir)
        .unwrap();
        let restored = Preferences::load(&dir).unwrap();
        assert!(!restored.topmost);
        assert!(restored.show_done);
        assert!(!restored.rail_left);
        fs::write(dir.join("preferences.json"), br#"{"topmost":true}"#).unwrap();
        let legacy = Preferences::load(&dir).unwrap();
        assert!(legacy.topmost);
        assert!(!legacy.show_done);
        assert!(legacy.rail_left);
        assert_eq!(fs::read(dir.join("state.json")).unwrap(), b"keep notes");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn tray_native_activation_and_cleanup() {
        let tray = Tray::new(true).unwrap();
        let handle = tray.control().0 as HWND;
        let (tx, rx) = mpsc::channel();
        tray.on_action(move |action| {
            let _ = tx.send(action);
        });
        unsafe {
            assert_ne!(PostMessageW(handle, ACTIVATE, 0, 0), 0);
        }
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Action::Show
        ));
        drop(tray);
        unsafe {
            assert_eq!(IsWindow(handle), 0);
        }
    }
    #[test]
    fn startup_quotes_paths_with_spaces() {
        assert_eq!(
            startup_command(&std::path::PathBuf::from(r"C:\My Apps\pet.exe")),
            r#""C:\My Apps\pet.exe""#
        );
    }
}
