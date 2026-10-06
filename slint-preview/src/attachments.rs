//! Only remove individual app-owned copies, never source files or linked folders.
use std::{
    collections::HashSet,
    fs,
    os::windows::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub(super) fn resolve(dir: &Path, kind: &str, relative: &str) -> Option<PathBuf> {
    if !matches!(kind, "images" | "files")
        || relative.is_empty()
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let base = dir.canonicalize().ok()?;
    let root = dir.join(kind);
    let canonical_root = root.canonicalize().ok()?;
    if !canonical_root.starts_with(&base) || canonical_root == base {
        return None;
    }
    let mut path = root;
    // FILE_ATTRIBUTE_REPARSE_POINT covers symlinks and directory junctions.
    if path.symlink_metadata().ok()?.file_attributes() & 0x400 != 0 {
        return None;
    }
    for part in Path::new(relative).components() {
        path.push(part.as_os_str());
        if path.symlink_metadata().ok()?.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let actual = path.canonicalize().ok()?;
    (actual.starts_with(&canonical_root) && actual.is_file()).then_some(actual)
}

pub(super) fn remove(dir: &Path, kind: &str, relative: &str, protected: &HashSet<PathBuf>) {
    let Some(file) = resolve(dir, kind, relative) else {
        return;
    };
    if protected.contains(&file) {
        return;
    }
    let Ok(root) = dir.join(kind).canonicalize() else {
        return;
    };
    if fs::remove_file(&file).is_err() {
        return;
    }
    // Empty import batches may be removed. Never recursively delete, or remove
    // the images/files root; other attachments stop this walk immediately.
    let mut parent = file.parent();
    while let Some(folder) = parent {
        if folder == root || !folder.starts_with(&root) || fs::remove_dir(folder).is_err() {
            break;
        }
        parent = folder.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_copies_respect_references_traversal_and_junctions() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("pet-attachments-{}-{stamp}", std::process::id()));
        let root = dir.join("data");
        fs::create_dir_all(root.join("files/batch")).unwrap();
        let copy = root.join("files/batch/copy.txt");
        fs::write(&copy, b"copy").unwrap();
        let outside = dir.join("source.txt");
        fs::write(&outside, b"source").unwrap();
        let mut refs = HashSet::new();
        refs.insert(resolve(&root, "files", r"batch\copy.txt").unwrap());
        remove(&root, "files", "batch/copy.txt", &refs);
        assert!(copy.exists());
        remove(&root, "files", "../../source.txt", &HashSet::new());
        remove(&root, "files", &outside.to_string_lossy(), &HashSet::new());
        assert!(outside.exists());
        // Junction creation requires no elevation, unlike Windows symlinks.
        let link = root.join("files").join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "junction fixture could not be created: {} {}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
        remove(&root, "files", "linked/source.txt", &HashSet::new());
        assert!(outside.exists());
        fs::remove_dir(link).unwrap();
        remove(&root, "files", "batch/copy.txt", &HashSet::new());
        assert!(!copy.exists());
        assert!(!root.join("files/batch").exists());
        assert!(root.join("files").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
