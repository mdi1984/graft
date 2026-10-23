//! Font setup: Inter for UI text, JetBrains Mono for code.
//!
//! Fonts are looked up in this order, first hit wins:
//! 1. `fonts/` next to the executable (release layout),
//! 2. `crates/graft/assets/fonts/` (running from the source tree),
//! 3. the per-user and system Windows font folders (an installed Inter),
//! 4. a Windows fallback (Segoe UI, Consolas),
//! 5. egui's built-in fonts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Font family name for semibold UI text (egui has no font weights).
pub const SEMIBOLD: &str = "semibold";

struct Face {
    key: &'static str,
    preferred: &'static str,
    fallback: &'static str,
}

const UI: Face = Face {
    key: "graft-ui",
    preferred: "Inter-Regular.ttf",
    fallback: "segoeui.ttf",
};
const UI_SEMIBOLD: Face = Face {
    key: "graft-ui-semibold",
    preferred: "Inter-SemiBold.ttf",
    fallback: "seguisb.ttf",
};
const CODE: Face = Face {
    key: "graft-code",
    preferred: "JetBrainsMono-Regular.ttf",
    fallback: "consola.ttf",
};

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let builtin_proportional = defs.families[&FontFamily::Proportional].clone();
    let builtin_monospace = defs.families[&FontFamily::Monospace].clone();

    let (preferred_dirs, system_dirs) = search_dirs();
    let mut load = |face: &Face| -> Option<String> {
        let path = find(face.preferred, preferred_dirs.iter().chain(&system_dirs))
            .or_else(|| find(face.fallback, &system_dirs))?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                tracing::info!(font = face.key, path = %path.display(), "loaded font");
                defs.font_data
                    .insert(face.key.to_owned(), Arc::new(FontData::from_owned(bytes)));
                Some(face.key.to_owned())
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "could not read font");
                None
            }
        }
    };

    let ui = load(&UI);
    let semibold = load(&UI_SEMIBOLD);
    let code = load(&CODE);

    let chain = |primary: &[&Option<String>], rest: &[String]| -> Vec<String> {
        primary
            .iter()
            .filter_map(|name| (*name).clone())
            .chain(rest.iter().cloned())
            .collect()
    };
    defs.families.insert(
        FontFamily::Proportional,
        chain(&[&ui], &builtin_proportional),
    );
    defs.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        chain(&[&semibold, &ui], &builtin_proportional),
    );
    defs.families
        .insert(FontFamily::Monospace, chain(&[&code], &builtin_monospace));

    ctx.set_fonts(defs);
}

fn find<'a>(file: &str, dirs: impl IntoIterator<Item = &'a PathBuf>) -> Option<PathBuf> {
    dirs.into_iter()
        .map(|dir| dir.join(file))
        .find(|path| path.is_file())
}

/// Returns (bundled font dirs, system font dirs).
fn search_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut bundled = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        bundled.push(exe_dir.join("fonts"));
    }
    bundled.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fonts"),
    );

    let mut system = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        system.push(Path::new(&local).join(r"Microsoft\Windows\Fonts"));
    }
    if let Some(windir) = std::env::var_os("WINDIR") {
        system.push(Path::new(&windir).join("Fonts"));
    }
    (bundled, system)
}
