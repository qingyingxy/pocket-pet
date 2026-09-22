use std::{fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let svg = fs::read(root.join("ui/cat.svg"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(256, 256).ok_or("icon allocation failed")?;
    let scale = 256.0 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png()?;
    // ICO directory entry with a PNG-compressed 256 x 256 image.
    let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend(png);
    fs::write(root.join("ui/cat.ico"), ico)?;
    Ok(())
}
