use super::*;

pub(super) fn argument(name: &str) -> Result<Option<String>> {
    let args: Vec<_> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == name) {
        let value = args
            .get(at + 1)
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("{name} 缺少参数"))?;
        Ok(Some(value.clone()))
    } else {
        Ok(None)
    }
}

// run() validates this argument before any window/diagnostic callback starts.
pub(super) fn preview_dir() -> PathBuf {
    argument("--snapshot-output")
        .ok()
        .flatten()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview-output"))
}
