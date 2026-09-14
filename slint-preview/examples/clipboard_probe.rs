#[path = "../src/clipboard.rs"]
mod clipboard;
fn main() {
    match clipboard::read_image() {
        Ok(Some(i)) => println!(
            "OK {}x{} alpha_nonzero={}",
            i.width(),
            i.height(),
            i.pixels().filter(|p| p[3] != 0).count()
        ),
        other => println!("{other:?}"),
    }
}
