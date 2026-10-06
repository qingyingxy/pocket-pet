use super::*;

const DELETE_UNDO_DURATION: Duration = Duration::from_secs(10);
#[derive(Clone)]
pub(super) struct DeletedRecord {
    index: usize,
    note: Note,
}
pub(super) struct DeletionUndo {
    records: Vec<DeletedRecord>,
    pub deadline: Instant,
}
impl Store {
    // Array indices are UI IDs. Invalidate all index-keyed transient operations
    // after a successful structural change, so old animation timers cannot act
    // on a different note now occupying the same index.
    fn clear_index_state(&mut self) {
        self.undo = None;
        self.completion = None;
        self.finishing.clear();
        self.snaps.clear();
        self.snap_rects.clear();
    }
    pub fn deletion_count(&self) -> usize {
        self.deletion
            .as_ref()
            .filter(|u| Instant::now() < u.deadline)
            .map_or(0, |u| u.records.len())
    }
    pub fn delete_note(&mut self, index: usize) -> Result<()> {
        if index >= self.data.notes.len() {
            return Err("记录不存在".into());
        }
        let mut next = self.data.clone();
        if let Some(selected) = self.selected {
            next.notes[selected] = self.buffer.clone();
        } else {
            next.draft = self.buffer.clone();
        }
        let note = next.notes.remove(index);
        save_data(&self.dir, &next)?;
        self.data = next;
        if self.selected == Some(index) {
            self.selected = None;
            self.buffer = self.data.draft.clone();
        } else if let Some(selected) = self.selected {
            if selected > index {
                self.selected = Some(selected - 1);
            }
        }
        self.clear_index_state();
        let now = Instant::now();
        if self.deletion.as_ref().is_none_or(|u| now >= u.deadline) {
            if let Some(old) = self.deletion.take() {
                self.cleanup_deleted(&old.records);
            }
            self.deletion = Some(DeletionUndo {
                records: Vec::new(),
                deadline: now + DELETE_UNDO_DURATION,
            });
        }
        let undo = self.deletion.as_mut().unwrap();
        undo.records.push(DeletedRecord { index, note });
        undo.deadline = now + DELETE_UNDO_DURATION;
        Ok(())
    }
    pub fn undo_deletion(&mut self) -> Result<()> {
        let undo = self.deletion.as_ref().ok_or("没有可撤销的删除")?;
        if Instant::now() >= undo.deadline {
            return Err("删除撤销时间已过".into());
        }
        let mut next = self.data.clone();
        let mut selected = self.selected;
        if let Some(i) = selected {
            next.notes[i] = self.buffer.clone();
        } else {
            next.draft = self.buffer.clone();
        }
        for record in undo.records.iter().rev() {
            let at = record.index.min(next.notes.len());
            next.notes.insert(at, record.note.clone());
            if let Some(i) = selected {
                if i >= at {
                    selected = Some(i + 1);
                }
            }
        }
        save_data(&self.dir, &next)?;
        self.data = next;
        self.selected = selected;
        self.clear_index_state();
        self.deletion = None;
        Ok(())
    }
    pub fn expire_deletion(&mut self, deadline: Instant) {
        if self
            .deletion
            .as_ref()
            .is_some_and(|u| u.deadline == deadline && Instant::now() >= deadline)
        {
            if let Some(undo) = self.deletion.take() {
                self.cleanup_deleted(&undo.records);
            }
        }
    }
    fn cleanup_deleted(&self, records: &[DeletedRecord]) {
        // Keep the backup usable. Unreadable backup => retain attachments.
        let backup = match fs::read(self.dir.join("state.backup.json")) {
            Ok(bytes) => match serde_json::from_slice::<Data>(&bytes) {
                Ok(data) => Some(data),
                Err(_) => return,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return,
        };
        let mut refs = std::collections::HashSet::new();
        let mut protect = |n: &Note| {
            for name in &n.images {
                if let Some(path) = attachments::resolve(&self.dir, "images", name) {
                    refs.insert(path);
                }
            }
            for file in &n.files {
                if let Some(path) = attachments::resolve(&self.dir, "files", &file.path) {
                    refs.insert(path);
                }
            }
        };
        for n in &self.data.notes {
            protect(n);
        }
        protect(&self.data.draft);
        protect(&self.buffer);
        if let Some(data) = &backup {
            for n in &data.notes {
                protect(n);
            }
            protect(&data.draft);
        }
        if let Some(undo) = &self.deletion {
            for r in &undo.records {
                protect(&r.note);
            }
        }
        for r in records {
            let paths = r
                .note
                .images
                .iter()
                .map(|n| ("images", n.as_str()))
                .chain(r.note.files.iter().map(|f| ("files", f.path.as_str())));
            for (kind, relative) in paths {
                attachments::remove(&self.dir, kind, relative, &refs);
            }
        }
    }
}

fn reset_views(ui: &PocketWindow, pet: &PetWindow) {
    ui.set_record_menu(-1);
    ui.set_expanded_id(-1);
    ui.set_editing_id(-1);
    ui.set_reminder_id(-1);
    ui.set_completion_request(-1);
    ui.set_fresh_id(-1);
    ui.invoke_close_image();
    pet.set_can_undo(false);
    pet.invoke_rail_reset();
}

fn schedule_expiry(
    state: Rc<RefCell<Store>>,
    weak: slint::Weak<PocketWindow>,
    animal: slint::Weak<PetWindow>,
    deadline: Instant,
) {
    let delay = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1);
    Timer::single_shot(delay, move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let mut s = state.borrow_mut();
        if !s.deletion.as_ref().is_some_and(|u| u.deadline == deadline) {
            return;
        }
        // Slint's cached/rounded timer clock can wake just before Instant's deadline.
        // Keep the true undo interval, then retry rather than leave a stale button forever.
        if Instant::now() < deadline {
            drop(s);
            schedule_expiry(state, weak, animal, deadline);
            return;
        }
        s.expire_deletion(deadline);
        sync(&ui, &pet, &mut s);
    });
}

pub fn install(ui: &PocketWindow, pet: &PetWindow, store: Rc<RefCell<Store>>) {
    let state = store.clone();
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    ui.on_delete_record(move |id| {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let mut s = state.borrow_mut();
        if let Err(e) = s.delete_note(id as usize) {
            report(&ui, e);
            return;
        }
        let deadline = s.deletion.as_ref().unwrap().deadline;
        let count = s.deletion_count();
        reset_views(&ui, &pet);
        sync(&ui, &pet, &mut s);
        ui.set_notice(format!("已删除 {count} 条 · 10 秒内可撤销").into());
        schedule_expiry(state.clone(), ui.as_weak(), pet.as_weak(), deadline);
    });
    let weak = ui.as_weak();
    let animal = pet.as_weak();
    ui.on_undo_delete(move || {
        let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
            return;
        };
        let mut s = store.borrow_mut();
        match s.undo_deletion() {
            Ok(()) => {
                reset_views(&ui, &pet);
                sync(&ui, &pet, &mut s);
                ui.set_notice("已恢复删除的记录".into());
            }
            Err(e) => {
                report(&ui, e);
                sync(&ui, &pet, &mut s);
            }
        }
    });
}

pub fn check(ui: &PocketWindow, pet: &PetWindow, state: Rc<RefCell<Store>>) {
    let checks = Rc::new(RefCell::new(Vec::new()));
    for (ms, stage) in [
        (500, 0),
        (800, 1),
        (1050, 2),
        (1300, 3),
        (1600, 4),
        (12100, 5),
    ] {
        let weak = ui.as_weak();
        let animal = pet.as_weak();
        let state = state.clone();
        let checks = checks.clone();
        Timer::single_shot(Duration::from_millis(ms), move || {
            let (Some(ui), Some(pet)) = (weak.upgrade(), animal.upgrade()) else {
                return;
            };
            let out = state.borrow().dir.clone();
            match stage {
                0 => {
                    let mut s = state.borrow_mut();
                    s.data.notes[1].done = true;
                    s.flush().unwrap();
                    ui.set_show_done(true);
                    sync(&ui, &pet, &mut s);
                    drop(s);
                    ui.invoke_edit(1);
                    ui.set_record_menu(1);
                    ui.set_record_menu_done(true);
                    ui.set_record_menu_y(82.);
                }
                1 => {
                    let _ = snapshot_window(ui.window(), &out.join("completed-actions.png"));
                    ui.invoke_toggle(1);
                    ui.set_record_menu(-1);
                    checks.borrow_mut().push(format!(
                        "restore-done={}",
                        !state.borrow().data.notes[1].done
                    ));
                    ui.invoke_delete_record(0);
                }
                2 => {
                    checks.borrow_mut().push(format!(
                        "delete-visible={}",
                        state.borrow().data.notes.len() == 2 && ui.get_delete_undo()
                    ));
                    ui.invoke_delete_record(1);
                }
                3 => {
                    let _ = snapshot_window(ui.window(), &out.join("deletion-toast.png"));
                    checks.borrow_mut().push(format!(
                        "batch-undo-offered={}",
                        state.borrow().deletion_count() == 2
                    ));
                    ui.invoke_undo_delete();
                }
                4 => {
                    checks.borrow_mut().push(format!(
                        "batch-restored={}",
                        state.borrow().data.notes.len() == 3 && !ui.get_delete_undo()
                    ));
                    ui.invoke_delete_record(0);
                }
                _ => {
                    let s = state.borrow();
                    checks.borrow_mut().push(format!(
                        "expired-and-persisted={}",
                        s.deletion_count() == 0
                            && !ui.get_delete_undo()
                            && Store::load(s.dir.clone()).unwrap().data.notes.len() == 2
                    ));
                    let all = checks.borrow().iter().all(|s| s.ends_with("=true"));
                    let _ = fs::write(
                        out.join("deletion-check.txt"),
                        format!(
                            "{}\n{}\n",
                            if all { "PASS" } else { "FAIL" },
                            checks.borrow().join("\n")
                        ),
                    );
                    drop(s);
                    ui.invoke_quit();
                }
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_drop_copies_are_cleaned_but_shared_alias_and_draft_are_kept() {
        let mut s = store();
        fs::create_dir_all(s.dir.join("files/batch")).unwrap();
        for name in ["removed.txt", "shared.txt", "draft.txt"] {
            fs::write(s.dir.join("files/batch").join(name), b"copy").unwrap();
            s.data.notes[0].files.push(Attachment {
                name: name.into(),
                path: format!("batch/{name}"),
            });
        }
        s.data.notes[2].files.push(Attachment {
            name: "shared".into(),
            path: r"batch\shared.txt".into(),
        });
        s.buffer.files.push(Attachment {
            name: "draft".into(),
            path: "batch/draft.txt".into(),
        });
        s.flush().unwrap();
        s.delete_note(0).unwrap();
        s.flush().unwrap();
        let deadline = Instant::now() - Duration::from_secs(1);
        s.deletion.as_mut().unwrap().deadline = deadline;
        s.expire_deletion(deadline);
        assert!(!s.dir.join("files/batch/removed.txt").exists());
        assert!(s.dir.join("files/batch/shared.txt").exists());
        assert!(s.dir.join("files/batch/draft.txt").exists());
        fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn cleanup_only_removes_unreferenced_app_copies_after_backup_rotates() {
        let mut s = store();
        fs::create_dir_all(s.dir.join("files")).unwrap();
        fs::write(s.dir.join("files/copy.txt"), b"copied").unwrap();
        fs::write(s.dir.join("images/shared.png"), b"shared").unwrap();
        s.data.notes[0].files.push(Attachment {
            name: "copy".into(),
            path: "copy.txt".into(),
        });
        s.data.notes[0].images.push("shared.png".into());
        s.data.notes[2].images.push("shared.png".into());
        s.flush().unwrap();
        s.delete_note(0).unwrap();
        s.flush().unwrap(); // The backup now references the surviving records.
        let deadline = Instant::now() - Duration::from_secs(1);
        s.deletion.as_mut().unwrap().deadline = deadline;
        s.expire_deletion(deadline);
        assert!(!s.dir.join("files/copy.txt").exists());
        assert!(s.dir.join("images/shared.png").exists());
        fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn deleting_selected_note_preserves_draft_and_invalidates_old_animation_ids() {
        let mut s = store();
        s.selected = Some(1);
        s.buffer = s.data.notes[1].clone();
        s.buffer.text = "edited completed".into();
        s.finishing
            .insert(2, (s.data.notes[2].clone(), false, Instant::now()));
        s.delete_note(1).unwrap();
        assert!(s.selected.is_none());
        assert_eq!(s.buffer.text, "keep draft");
        assert!(s.finishing.is_empty());
        s.undo_deletion().unwrap();
        assert_eq!(s.data.notes[1].text, "edited completed");
        fs::remove_dir_all(s.dir).unwrap();
    }
    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!(
            "pet-delete-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut s = Store::load(dir).unwrap();
        s.data.notes = (0..4)
            .map(|i| Note {
                text: format!("note{i}"),
                done: i == 1,
                remind_at: Some(123),
                ..Note::default()
            })
            .collect();
        s.buffer.text = "keep draft".into();
        s.flush().unwrap();
        s
    }
    #[test]
    fn batch_delete_undo_restores_order_flags_and_keeps_new_capture() {
        let mut s = store();
        s.delete_note(1).unwrap();
        s.delete_note(2).unwrap();
        s.capture_note(Note {
            text: "new".into(),
            ..Note::default()
        })
        .unwrap();
        s.undo_deletion().unwrap();
        assert_eq!(
            s.data
                .notes
                .iter()
                .map(|n| n.text.as_str())
                .collect::<Vec<_>>(),
            vec!["note0", "note1", "note2", "note3", "new"]
        );
        assert!(s.data.notes[1].done);
        assert_eq!(s.data.notes[1].remind_at, Some(123));
        assert_eq!(s.buffer.text, "keep draft");
        assert_eq!(Store::load(s.dir.clone()).unwrap().data.notes.len(), 5);
        fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn failed_delete_and_failed_restore_do_not_change_memory() {
        let mut s = store();
        let original = s.data.notes.clone();
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.delete_note(0).is_err());
        assert!(s.deletion.is_none());
        assert!(s.data.notes == original);
        fs::remove_dir(s.dir.join("state.tmp")).unwrap();
        s.delete_note(0).unwrap();
        let deleted = s.data.notes.clone();
        fs::create_dir(s.dir.join("state.tmp")).unwrap();
        assert!(s.undo_deletion().is_err());
        assert!(s.data.notes == deleted);
        assert_eq!(s.deletion_count(), 1);
        fs::remove_dir_all(s.dir).unwrap();
    }
    #[test]
    fn expiry_is_not_undoable_and_keeps_backup_and_source_files() {
        let mut s = store();
        let outside = s.dir.with_extension("source.txt");
        fs::write(&outside, b"source").unwrap();
        fs::write(s.dir.join("images/shared.png"), b"image").unwrap();
        s.data.notes[0].images.push("shared.png".into());
        s.data.notes[0].files.push(Attachment {
            name: "source".into(),
            path: outside.to_string_lossy().into(),
        });
        s.flush().unwrap();
        s.delete_note(0).unwrap();
        let deadline = Instant::now() - Duration::from_secs(1);
        s.deletion.as_mut().unwrap().deadline = deadline;
        assert!(s.undo_deletion().is_err());
        s.expire_deletion(deadline);
        assert!(s.deletion.is_none());
        assert!(s.dir.join("images/shared.png").exists());
        assert!(outside.exists());
        assert_eq!(Store::load(s.dir.clone()).unwrap().data.notes.len(), 3);
        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(s.dir).unwrap();
    }
}
