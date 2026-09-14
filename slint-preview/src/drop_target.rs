use crate::drop_import::{self, Payload};
use std::{cell::Cell, marker::PhantomData, path::PathBuf, rc::Rc};
use windows::{
    core::{implement, w, Ref, Result},
    Win32::{
        Foundation::{HWND, POINTL},
        System::{
            Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL},
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
                ReleaseStgMedium, RevokeDragDrop, CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT,
                DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::Shell::{DragQueryFileW, HDROP},
    },
};

pub enum Event {
    Hover(bool),
    Dropped(std::result::Result<Payload, String>),
}
struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe {
            ReleaseStgMedium(&mut self.0);
        }
    }
}
fn format(id: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: id,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}
fn formats() -> Vec<u16> {
    unsafe {
        vec![
            CF_HDROP.0,
            RegisterClipboardFormatW(w!("PNG")) as u16,
            RegisterClipboardFormatW(w!("image/png")) as u16,
            CF_DIBV5.0,
            CF_DIB.0,
            CF_UNICODETEXT.0,
            RegisterClipboardFormatW(w!("UniformResourceLocatorW")) as u16,
        ]
    }
}
fn supports(data: &IDataObject) -> bool {
    formats()
        .into_iter()
        .any(|id| id != 0 && unsafe { data.QueryGetData(&format(id)).is_ok() })
}
fn read_global(medium: &Medium, limit: usize) -> std::result::Result<Vec<u8>, String> {
    unsafe {
        let handle = medium.0.u.hGlobal;
        let size = GlobalSize(handle);
        if size == 0 || size > limit {
            return Err("拖入内容为空或过大".into());
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            return Err("无法读取拖入内容".into());
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(handle);
        Ok(bytes)
    }
}
fn read_payload(data: &IDataObject) -> std::result::Result<Payload, String> {
    for id in formats() {
        if id == 0 || unsafe { data.QueryGetData(&format(id)).is_err() } {
            continue;
        }
        let medium = match unsafe { data.GetData(&format(id)) } {
            Ok(m) => Medium(m),
            Err(_) => continue,
        };
        if medium.0.tymed != TYMED_HGLOBAL.0 as u32 {
            continue;
        }
        if id == CF_HDROP.0 {
            unsafe {
                let handle = HDROP(medium.0.u.hGlobal.0);
                let count = DragQueryFileW(handle, u32::MAX, None);
                if count == 0 || count as usize > drop_import::MAX_FILES {
                    return Err("每次可收下 1–32 个文件".into());
                }
                let mut paths = Vec::new();
                for i in 0..count {
                    let length = DragQueryFileW(handle, i, None);
                    if length == 0 || length > 32767 {
                        return Err("文件路径无效".into());
                    }
                    let mut buffer = vec![0u16; length as usize + 1];
                    let copied = DragQueryFileW(handle, i, Some(&mut buffer));
                    if copied != length {
                        return Err("文件路径读取失败".into());
                    }
                    use std::os::windows::ffi::OsStringExt;
                    paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                        &buffer[..length as usize],
                    )));
                }
                return Ok(Payload::Files(paths));
            }
        }
        if id == CF_UNICODETEXT.0
            || id == unsafe { RegisterClipboardFormatW(w!("UniformResourceLocatorW")) as u16 }
        {
            return drop_import::decode_text(&read_global(&medium, drop_import::MAX_TEXT_BYTES)?)
                .map(Payload::Text);
        }
        return Ok(Payload::Image {
            bytes: read_global(&medium, 64 * 1024 * 1024)?,
            dib: id == CF_DIB.0 || id == CF_DIBV5.0,
        });
    }
    Err("这种拖入内容暂不支持".into())
}
#[implement(IDropTarget)]
struct Target {
    supported: Cell<bool>,
    callback: Box<dyn Fn(Event) -> bool>,
}
#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let supported = data.as_ref().is_some_and(supports);
        self.supported.set(supported);
        let allowed =
            supported && !effect.is_null() && unsafe { (*effect).0 & DROPEFFECT_COPY.0 != 0 };
        let allowed = (self.callback)(Event::Hover(allowed)) && allowed;
        self.supported.set(allowed);
        if !effect.is_null() {
            unsafe {
                *effect = if allowed {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
        }
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        if !effect.is_null() {
            unsafe {
                *effect = if self.supported.get() && (*effect).0 & DROPEFFECT_COPY.0 != 0 {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
        }
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        self.supported.set(false);
        (self.callback)(Event::Hover(false));
        Ok(())
    }
    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        (self.callback)(Event::Hover(false));
        let allowed = !effect.is_null() && unsafe { (*effect).0 & DROPEFFECT_COPY.0 != 0 };
        if !effect.is_null() {
            unsafe {
                *effect = DROPEFFECT_NONE;
            }
        }
        self.supported.set(false);
        if allowed {
            let payload = data
                .as_ref()
                .ok_or_else(|| "拖入内容不可用".to_string())
                .and_then(read_payload);
            let valid = payload.is_ok();
            if (self.callback)(Event::Dropped(payload)) && valid && !effect.is_null() {
                unsafe {
                    *effect = DROPEFFECT_COPY;
                }
            }
        }
        Ok(())
    }
}
pub struct Registration {
    hwnd: HWND,
    _target: IDropTarget,
    _apartment: PhantomData<Rc<()>>,
}
impl Registration {
    pub fn new(hwnd: HWND, callback: impl Fn(Event) -> bool + 'static) -> Result<Self> {
        unsafe {
            OleInitialize(None)?;
            let target: IDropTarget = Target {
                supported: Cell::new(false),
                callback: Box::new(callback),
            }
            .into();
            if let Err(e) = RegisterDragDrop(hwnd, &target) {
                OleUninitialize();
                return Err(e);
            }
            Ok(Self {
                hwnd,
                _target: target,
                _apartment: PhantomData,
            })
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            OleUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    use windows::core::{BOOL, HRESULT};
    use windows::Win32::{
        Foundation::{DV_E_FORMATETC, E_NOTIMPL},
        System::{
            Com::{IAdviseSink, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA},
            Memory::{GlobalAlloc, GMEM_MOVEABLE, GMEM_ZEROINIT},
        },
    };
    #[implement(IDataObject)]
    struct Sample {
        format: u16,
        bytes: Vec<u8>,
    }
    #[allow(non_snake_case)]
    impl IDataObject_Impl for Sample_Impl {
        fn GetData(&self, requested: *const FORMATETC) -> Result<STGMEDIUM> {
            if self.QueryGetData(requested).is_err() {
                return Err(DV_E_FORMATETC.into());
            }
            unsafe {
                let global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, self.bytes.len())?;
                let ptr = GlobalLock(global);
                assert!(!ptr.is_null());
                std::ptr::copy_nonoverlapping(self.bytes.as_ptr(), ptr.cast(), self.bytes.len());
                let _ = GlobalUnlock(global);
                let mut medium = STGMEDIUM::default();
                medium.tymed = TYMED_HGLOBAL.0 as u32;
                medium.u.hGlobal = global;
                Ok(medium)
            }
        }
        fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
            if !f.is_null()
                && unsafe { (*f).cfFormat == self.format && (*f).tymed == TYMED_HGLOBAL.0 as u32 }
            {
                HRESULT(0)
            } else {
                DV_E_FORMATETC
            }
        }
        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
            E_NOTIMPL
        }
        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumFormatEtc(&self, _: u32) -> Result<IEnumFORMATETC> {
            Err(E_NOTIMPL.into())
        }
        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> Result<u32> {
            Err(E_NOTIMPL.into())
        }
        fn DUnadvise(&self, _: u32) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
            Err(E_NOTIMPL.into())
        }
    }
    fn sample(format: u16, bytes: Vec<u8>) -> IDataObject {
        Sample { format, bytes }.into()
    }
    fn wide(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
    #[test]
    fn ole_text_copy_and_cancel() {
        let data = sample(CF_UNICODETEXT.0, wide("稍后回复同事\0"));
        let received = Rc::new(RefCell::new(Vec::new()));
        let events = received.clone();
        let target: IDropTarget = Target {
            supported: Cell::new(false),
            callback: Box::new(move |event| {
                events.borrow_mut().push(event);
                true
            }),
        }
        .into();
        unsafe {
            let mut effect = DROPEFFECT_COPY;
            target
                .DragEnter(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_COPY);
            target.DragLeave().unwrap();
            assert!(!received
                .borrow()
                .iter()
                .any(|e| matches!(e, Event::Dropped(_))));
            target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_COPY);
        }
        assert!(received
            .borrow()
            .iter()
            .any(|e| matches!(e,Event::Dropped(Ok(Payload::Text(text))) if text=="稍后回复同事")));
    }
    #[test]
    fn move_only_and_unsupported_are_rejected() {
        let data = sample(CF_UNICODETEXT.0, wide("保留原文\0"));
        let target: IDropTarget = Target {
            supported: Cell::new(false),
            callback: Box::new(|_| true),
        }
        .into();
        unsafe {
            let mut effect = windows::Win32::System::Ole::DROPEFFECT_MOVE;
            target
                .DragEnter(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
            let unknown = sample(0xffff, vec![1, 2]);
            effect = DROPEFFECT_COPY;
            target
                .DragEnter(
                    &unknown,
                    MODIFIERKEYS_FLAGS(0),
                    POINTL::default(),
                    &mut effect,
                )
                .unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
        }
    }
    #[test]
    fn ole_file_batch_and_bitmap_formats() {
        let mut hdrop = vec![0u8; 20];
        hdrop[0..4].copy_from_slice(&20u32.to_le_bytes());
        hdrop[16..20].copy_from_slice(&1u32.to_le_bytes());
        hdrop.extend(wide("C:\\资料\\一.txt\0C:\\资料\\二.png\0\0"));
        let data = sample(CF_HDROP.0, hdrop);
        assert!(
            matches!(read_payload(&data).unwrap(),Payload::Files(files) if files.len()==2 && files[0].ends_with("一.txt"))
        );
        let dib = sample(CF_DIB.0, vec![1, 2, 3, 4]);
        assert!(matches!(
            read_payload(&dib).unwrap(),
            Payload::Image { dib: true, .. }
        ));
    }
}
