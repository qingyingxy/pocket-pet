use std::{io, sync::mpsc, thread};
use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub struct Hotkey {
    thread_id: u32,
    worker: Option<thread::JoinHandle<()>>,
}

impl Hotkey {
    pub fn register(callback: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || unsafe {
            // Blocking message loop: no polling timer or clipboard monitoring.
            let mut message: MSG = std::mem::zeroed();
            PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            if RegisterHotKey(
                std::ptr::null_mut(),
                1,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                b'V' as u32,
            ) == 0
            {
                let _ = sender.send(Err(io::Error::last_os_error()));
                return;
            }
            let _ = sender.send(Ok(GetCurrentThreadId()));
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                if message.message == WM_HOTKEY && message.wParam == 1 {
                    callback();
                }
            }
            UnregisterHotKey(std::ptr::null_mut(), 1);
        });
        match receiver.recv().map_err(io::Error::other)? {
            Ok(thread_id) => Ok(Self {
                thread_id,
                worker: Some(worker),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn message_delivery_and_unregister() {
        let (sender, receiver) = mpsc::channel();
        let hotkey = Hotkey::register(move || {
            let _ = sender.send(());
        })
        .expect("Ctrl+Alt+V must be free for the integration check");
        unsafe {
            assert_ne!(PostThreadMessageW(hotkey.thread_id, WM_HOTKEY, 1, 0), 0);
        }
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(Hotkey::register(|| {}).is_err());
        drop(hotkey);
        let _registered_again = Hotkey::register(|| {}).unwrap();
    }
}
