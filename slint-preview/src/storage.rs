//! JSON persistence shared by notes and preferences. Never reset unreadable notes.
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Loaded<T> {
    pub value: T,
    pub notice: Option<String>,
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// Sync the complete temporary file before replacing the old one. Replacement is
// atomic on Windows; if writing/renaming fails, the previous file is still usable.
fn replace(dir: &Path, stem: &str, bytes: &[u8]) -> io::Result<()> {
    let temp = dir.join(format!("{stem}.tmp"));
    let mut file = fs::File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, dir.join(format!("{stem}.json")))
}

fn archive(dir: &Path, stem: &str, bytes: &[u8]) -> io::Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..16 {
        let path = dir.join(format!(
            "{stem}.corrupt.{stamp}.{}.{attempt}.json",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                return file.sync_all();
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("无法保留损坏文件，请备份数据目录后重试"))
}

pub(super) fn load<T: Serialize + DeserializeOwned + Default>(
    dir: &Path,
    stem: &str,
) -> io::Result<Loaded<T>> {
    let primary = dir.join(format!("{stem}.json"));
    let failure = match read(&primary) {
        Ok(value) => {
            return Ok(Loaded {
                value,
                notice: None,
            })
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidData
            ) =>
        {
            e
        }
        Err(e) => return Err(e),
    };
    let value = match read::<T>(&dir.join(format!("{stem}.backup.json"))) {
        Ok(value) => value,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound && failure.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(Loaded {
                value: T::default(),
                notice: None,
            });
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidData
            ) =>
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("{stem}.json 无法读取，且备份不可用。原文件未改动，请保留数据目录后恢复备份：{failure}; {e}")));
        }
        Err(e) => return Err(e),
    };
    if failure.kind() == io::ErrorKind::InvalidData {
        archive(dir, stem, &fs::read(&primary)?)?;
    }
    // Do not rotate the invalid primary over the known-good backup.
    replace(
        dir,
        stem,
        &serde_json::to_vec_pretty(&value).map_err(io::Error::other)?,
    )?;
    Ok(Loaded {
        value,
        notice: Some(format!(
            "已从备份恢复 {stem}.json，可能缺少最后一次修改；原文件和备份已保留"
        )),
    })
}

pub(super) fn save<T: Serialize + DeserializeOwned>(
    dir: &Path,
    stem: &str,
    value: &T,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let primary = dir.join(format!("{stem}.json"));
    match fs::read(&primary) {
        Ok(previous) => {
            // An external edit or damaged file must not destroy the good backup.
            serde_json::from_slice::<T>(&previous)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            // Write the new data first, so a failed write does not rotate backups.
            let temp = dir.join(format!("{stem}.tmp"));
            let mut file = fs::File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            replace(dir, &format!("{stem}.backup"), &previous)?;
            fs::rename(temp, primary)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => replace(dir, stem, &bytes),
        Err(e) => Err(e),
    }
}

// Display settings may be reset, while note data uses the strict loader above.
pub(super) fn load_preferences<T: Serialize + DeserializeOwned + Default>(
    dir: &Path,
) -> io::Result<Loaded<T>> {
    match load(dir, "preferences") {
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            for stem in ["preferences", "preferences.backup"] {
                match fs::read(dir.join(format!("{stem}.json"))) {
                    Ok(bytes) => archive(dir, stem, &bytes)?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
            }
            let value = T::default();
            replace(
                dir,
                "preferences",
                &serde_json::to_vec_pretty(&value).map_err(io::Error::other)?,
            )?;
            Ok(Loaded {
                value,
                notice: Some("显示偏好文件损坏，已恢复默认设置并保留原文件；记录未改动".into()),
            })
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Data, Note};
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("pet-storage-{}-{stamp}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn data() -> Data {
        Data {
            notes: vec![Note {
                text: "需要恢复".into(),
                images: vec!["batch/pic.png".into()],
                remind_at: Some(123),
                ..Note::default()
            }],
            draft: Note {
                text: "草稿".into(),
                ..Note::default()
            },
        }
    }
    #[test]
    fn corrupt_primary_restores_backup_and_preserves_original() {
        let f = Fixture::new();
        save(&f.0, "state", &data()).unwrap();
        save(&f.0, "state", &Data::default()).unwrap();
        fs::write(f.0.join("state.json"), b"{broken").unwrap();
        let recovered = load::<Data>(&f.0, "state").unwrap();
        assert!(recovered.notice.is_some());
        assert_eq!(recovered.value.notes, data().notes);
        assert_eq!(recovered.value.draft, data().draft);
        assert_eq!(
            read::<Data>(&f.0.join("state.backup.json")).unwrap().notes,
            data().notes
        );
        let archive = fs::read_dir(&f.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("state.corrupt.")
            })
            .unwrap();
        assert_eq!(fs::read(archive).unwrap(), b"{broken");
        assert!(load::<Data>(&f.0, "state").unwrap().notice.is_none());
    }
    #[test]
    fn missing_primary_recovers_but_missing_both_starts_empty() {
        let f = Fixture::new();
        assert!(load::<Data>(&f.0, "state").unwrap().value.notes.is_empty());
        save(&f.0, "state.backup", &data()).unwrap();
        assert_eq!(
            load::<Data>(&f.0, "state").unwrap().value.notes,
            data().notes
        );
    }
    #[test]
    fn unusable_notes_are_never_reset_or_overwritten() {
        let f = Fixture::new();
        fs::write(f.0.join("state.json"), b"broken primary").unwrap();
        fs::write(f.0.join("state.backup.json"), b"broken backup").unwrap();
        assert!(load::<Data>(&f.0, "state").is_err());
        assert!(save(&f.0, "state", &data()).is_err());
        assert_eq!(fs::read(f.0.join("state.json")).unwrap(), b"broken primary");
        assert_eq!(
            fs::read(f.0.join("state.backup.json")).unwrap(),
            b"broken backup"
        );
        fs::remove_file(f.0.join("state.json")).unwrap();
        assert!(load::<Data>(&f.0, "state").is_err());
        assert!(!f.0.join("state.json").exists());
    }
    #[test]
    fn failed_write_keeps_both_versions_and_valid_primary_ignores_bad_backup() {
        let f = Fixture::new();
        save(&f.0, "state", &data()).unwrap();
        save(&f.0, "state", &Data::default()).unwrap();
        fs::create_dir(f.0.join("state.tmp")).unwrap();
        assert!(save(&f.0, "state", &data()).is_err());
        assert!(read::<Data>(&f.0.join("state.json"))
            .unwrap()
            .notes
            .is_empty());
        assert_eq!(
            read::<Data>(&f.0.join("state.backup.json")).unwrap().notes,
            data().notes
        );
        fs::write(f.0.join("state.backup.json"), b"broken").unwrap();
        assert!(load::<Data>(&f.0, "state").unwrap().notice.is_none());
    }
    #[test]
    fn corrupt_settings_reset_without_touching_notes() {
        let f = Fixture::new();
        fs::write(f.0.join("state.json"), b"private notes").unwrap();
        fs::write(f.0.join("preferences.json"), b"broken").unwrap();
        let restored = load_preferences::<crate::tray::Preferences>(&f.0).unwrap();
        assert!(restored.value.topmost);
        assert!(restored.notice.is_some());
        assert_eq!(fs::read(f.0.join("state.json")).unwrap(), b"private notes");
    }
}
