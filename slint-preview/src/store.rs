use super::*;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Attachment {
    pub(super) name: String,
    pub(super) path: String,
}
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Note {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) created_at: Option<u64>,
    pub(super) text: String,
    pub(super) images: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) files: Vec<Attachment>,
    pub(super) done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) remind_at: Option<u64>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct Data {
    pub(super) notes: Vec<Note>,
    pub(super) draft: Note,
}
pub(super) struct Store {
    pub(super) recovery_notice: Option<String>,
    pub(super) data: Data,
    pub(super) buffer: Note,
    pub(super) selected: Option<usize>,
    pub(super) dir: PathBuf,
    pub(super) cache: HashMap<String, Image>,
    pub(super) thumbnail_decodes: usize,
    pub(super) undo: Option<CaptureUndo>,
    pub(super) completion: Option<CompletionUndo>,
    pub(super) deletion: Option<deletion::DeletionUndo>,
    pub(super) snap_rects: HashMap<usize, [f32; 6]>,
    pub(super) snaps: HashMap<usize, SnapVisual>,
    pub(super) finishing: HashMap<usize, (Note, bool, Instant)>,
}
pub(super) struct SnapVisual {
    pub(super) backdrop: Image,
    pub(super) layers: ModelRc<DustLayer>,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) running: bool,
    pub(super) bytes: usize,
    pub(super) prepare_ms: f64,
}
pub(super) struct CompletionUndo {
    pub(super) index: usize,
    pub(super) before: Note,
    pub(super) after: Note,
    pub(super) deadline: Instant,
}
pub(super) struct CaptureUndo {
    pub(super) index: usize,
    pub(super) note: Note,
    pub(super) deadline: Instant,
}
pub(super) const UNDO_DURATION: Duration = Duration::from_secs(6);
impl Store {
    pub(super) fn load(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("images"))?;
        let loaded = storage::load::<Data>(&dir, "state")?;
        let data = loaded.value;
        Ok(Self {
            recovery_notice: loaded.notice,
            buffer: data.draft.clone(),
            data,
            selected: None,
            dir,
            cache: HashMap::new(),
            thumbnail_decodes: 0,
            undo: None,
            completion: None,
            deletion: None,
            snap_rects: HashMap::new(),
            snaps: HashMap::new(),
            finishing: HashMap::new(),
        })
    }
    pub(super) fn flush(&mut self) -> Result<()> {
        let old = if let Some(i) = self.selected {
            let note = self.data.notes.get_mut(i).ok_or("记录不存在")?;
            std::mem::replace(note, self.buffer.clone())
        } else {
            std::mem::replace(&mut self.data.draft, self.buffer.clone())
        };
        if let Err(error) = save_data(&self.dir, &self.data) {
            if let Some(i) = self.selected {
                self.data.notes[i] = old;
            } else {
                self.data.draft = old;
            }
            return Err(error);
        }
        Ok(())
    }
    pub(super) fn capture_note(&mut self, mut note: Note) -> Result<()> {
        if note.text.trim().is_empty() && note.images.is_empty() && note.files.is_empty() {
            return Err("没有可收下的内容".into());
        }
        let before = self.data.clone();
        note.created_at = Some(reminders::now());
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
    pub(super) fn undo_capture(&mut self) -> Result<()> {
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
    pub(super) fn toggle_note(&mut self, index: usize) -> Result<()> {
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
    pub(super) fn set_reminder(&mut self, index: usize, at: Option<u64>) -> Result<()> {
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
    pub(super) fn update_record(
        &mut self,
        index: usize,
        update: impl FnOnce(&mut Note),
    ) -> Result<()> {
        let old = self.data.notes.get(index).ok_or("记录不存在")?.clone();
        let old_buffer = self.buffer.clone();
        if self.selected == Some(index) {
            self.data.notes[index] = self.buffer.clone();
        }
        update(&mut self.data.notes[index]);
        if self.selected == Some(index) {
            self.buffer = self.data.notes[index].clone();
        }
        if let Err(e) = self.flush() {
            self.data.notes[index] = old;
            self.buffer = old_buffer;
            return Err(e);
        }
        Ok(())
    }
    pub(super) fn complete(&mut self, index: usize) -> Result<()> {
        let saved = self.data.notes.get(index).ok_or("记录不存在")?;
        let before = if self.selected == Some(index) {
            self.buffer.clone()
        } else {
            saved.clone()
        };
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
    pub(super) fn undo_completion(&mut self) -> Result<()> {
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
    pub(super) fn thumbnail(&mut self, name: &str) -> Image {
        if let Some(image) = self.cache.get(name) {
            return image.clone();
        }
        self.thumbnail_decodes += 1;
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
    pub(super) fn clipboard_image(&self) -> Result<Option<Note>> {
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
                created_at: None,
            }));
        }
        Ok(None)
    }
    pub(super) fn clipboard(&self) -> Result<Note> {
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
            created_at: None,
        })
    }
}
pub(super) fn save_data(dir: &Path, data: &Data) -> Result<()> {
    storage::save(dir, "state", data)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undo_of_selected_completion_survives_subsequent_save() {
        let mut s = temporary_store();
        s.data.notes.push(Note {
            text: "正在编辑的记录".into(),
            remind_at: Some(123),
            ..Note::default()
        });
        s.selected = Some(0);
        s.buffer = s.data.notes[0].clone();
        s.flush().unwrap();
        s.buffer.text = "尚未自动保存的修改".into();
        s.complete(0).unwrap();
        s.undo_completion().unwrap();
        assert!(!s.buffer.done);
        assert_eq!(s.buffer.text, "尚未自动保存的修改");
        assert_eq!(s.buffer.remind_at, Some(123));
        s.flush().unwrap();
        assert!(!Store::load(s.dir.clone()).unwrap().data.notes[0].done);
        cleanup_store(&s.dir);
    }
    #[test]
    fn failed_draft_flush_keeps_committed_data_and_retryable_buffer() {
        let mut s = temporary_store();
        s.buffer.text = "已保存".into();
        s.flush().unwrap();
        s.buffer.text = "继续输入".into();
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.flush().is_err());
        assert_eq!(s.data.draft.text, "已保存");
        assert_eq!(s.buffer.text, "继续输入");
        fs::remove_dir(s.dir.join("state.tmp")).unwrap();
        s.flush().unwrap();
        assert_eq!(Store::load(s.dir.clone()).unwrap().buffer.text, "继续输入");
        cleanup_store(&s.dir);
    }
    fn cleanup_store(dir: &Path) {
        for name in ["state.json", "state.backup.json"] {
            if dir.join(name).exists() {
                fs::remove_file(dir.join(name)).unwrap();
            }
        }
        fs::remove_dir(dir.join("images")).unwrap();
        // Windows may retain delete-pending directory entries briefly (e.g.
        // antivirus file handles). Retry only non-recursive removal of this
        // fixture; unexpected remaining files still make the test fail.
        for attempt in 0..20 {
            match fs::remove_dir(dir) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty && attempt < 19 => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("temporary store cleanup failed: {e}"),
            }
        }
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
        Store::load(std::env::temp_dir().join(
            format!("pocket-capture-{}-{suffix}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()),
        ))
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
    fn added_time_survives_edit_completion_and_reload() {
        let mut s = temporary_store();
        s.capture_note(Note {
            text: "记录".into(),
            ..Note::default()
        })
        .unwrap();
        let created = s.data.notes[0].created_at;
        assert!(created.is_some());
        s.selected = Some(0);
        s.buffer = s.data.notes[0].clone();
        s.buffer.text = "修改".into();
        s.flush().unwrap();
        s.toggle_note(0).unwrap();
        s.toggle_note(0).unwrap();
        assert_eq!(
            Store::load(s.dir.clone()).unwrap().data.notes[0].created_at,
            created
        );
        cleanup_store(&s.dir);
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
                created_at: None,
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
