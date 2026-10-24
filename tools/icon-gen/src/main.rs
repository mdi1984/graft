//! Renders the app icon SVGs into `graft.ico` (Windows exe, taskbar and
//! window icon), `graft-256.png` (fallback window icon) and `graft-title.png`
//! (Graft's own title bar). Run with `cargo run -p icon-gen`.
//!
//! `cargo run -p icon-gen -- --preview out.png` instead draws every size on a
//! dark and a light taskbar color, magnified so single pixels are visible.

use std::path::Path;

use resvg::{tiny_skia, usvg};

/// Sizes Windows asks for; up to `SMALL_MAX` use the simplified small SVG.
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const SMALL_MAX: u32 = 24;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/graft/assets/icon");
    let large = load(&dir.join("graft.svg"));
    let small = load(&dir.join("graft-small.svg"));
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, out] = args.as_slice()
        && flag == "--preview"
    {
        preview(&large, &small, Path::new(out));
        return;
    }

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
    // Painted at ~18 px in Graft's own title bar.
    let title_path = dir.join("graft-title.png");
    render(&small, 64)
        .save_png(&title_path)
        .expect("write graft-title.png");

    for path in [ico_path, png_path, title_path] {
        println!("wrote {}", path.display());
    }
}

/// Each size at 1x and magnified 4x, on Windows' dark and light taskbar.
fn preview(large: &usvg::Tree, small: &usvg::Tree, out: &Path) {
    const ZOOM: u32 = 4;
    const SIZES: [u32; 5] = [16, 20, 24, 32, 48];
    let backgrounds = [
        tiny_skia::Color::from_rgba8(0x1c, 0x1c, 0x1c, 255),
        tiny_skia::Color::from_rgba8(0xee, 0xee, 0xee, 255),
    ];
    let cell = 48 * ZOOM + 16;
    let width = cell * SIZES.len() as u32 + 48 + 16;
    let mut canvas = tiny_skia::Pixmap::new(width, cell * 2).expect("canvas");
    for (row, bg) in backgrounds.iter().enumerate() {
        let top = row as u32 * cell;
        let band =
            tiny_skia::Rect::from_xywh(0.0, top as f32, width as f32, cell as f32).expect("band");
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(*bg);
        canvas.fill_rect(band, &paint, tiny_skia::Transform::identity(), None);
        let mut x = 8;
        let mut y = top + 8;
        for size in SIZES {
            let tree = if size <= SMALL_MAX { small } else { large };
            let icon = render(tree, size);
            // Actual size stacked in the left strip, magnified copies to the right.
            let plain = tiny_skia::PixmapPaint::default();
            canvas.draw_pixmap(8, y as i32, icon.as_ref(), &plain, Default::default(), None);
            y += size + 6;
            let zoomed = tiny_skia::Transform::from_scale(ZOOM as f32, ZOOM as f32)
                .post_translate((x + 56) as f32, (top + 8) as f32);
            let nearest = tiny_skia::PixmapPaint {
                quality: tiny_skia::FilterQuality::Nearest,
                ..Default::default()
            };
            canvas.draw_pixmap(0, 0, icon.as_ref(), &nearest, zoomed, None);
            x += cell;
        }
    }
    canvas.save_png(out).expect("write preview");
    println!("wrote {}", out.display());
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
