//! Renders the app icon SVGs into `graft.ico` (Windows exe/taskbar) and
//! `graft-256.png` (window icon). Run with `cargo run -p icon-gen`.

use std::path::Path;

use resvg::{tiny_skia, usvg};

/// Sizes Windows asks for; up to `SMALL_MAX` use the simplified small SVG.
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const SMALL_MAX: u32 = 24;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/graft/assets/icon");
    let large = load(&dir.join("graft.svg"));
    let small = load(&dir.join("graft-small.svg"));

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let tree = if size <= SMALL_MAX { &small } else { &large };
        let pixmap = render(tree, size);
        let image = ico::IconImage::from_rgba_data(size, size, unpremultiply(&pixmap));
        icon_dir.add_entry(ico::IconDirEntry::encode(&image).expect("encode ico entry"));
    }
    let ico_path = dir.join("graft.ico");
    let file = std::fs::File::create(&ico_path).expect("create graft.ico");
    icon_dir.write(file).expect("write graft.ico");

    let png_path = dir.join("graft-256.png");
    render(&large, 256)
        .save_png(&png_path)
        .expect("write graft-256.png");

    for path in [ico_path, png_path] {
        println!("wrote {}", path.display());
    }
}

fn load(path: &Path) -> usvg::Tree {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    usvg::Tree::from_data(&data, &usvg::Options::default()).expect("parse svg")
}

fn render(tree: &usvg::Tree, size: u32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// tiny-skia stores premultiplied alpha; ICO wants straight RGBA.
fn unpremultiply(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
