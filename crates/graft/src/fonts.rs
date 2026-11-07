//! Font setup: Inter for UI text, JetBrains Mono for code. The fonts are
//! built into the executable; egui's own fonts cover what they lack.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Font family name for semibold UI text (egui has no font weights).
pub const SEMIBOLD: &str = "semibold";

const UI: (&str, &[u8]) = (
    "graft-ui",
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
);
const UI_SEMIBOLD: (&str, &[u8]) = (
    "graft-ui-semibold",
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
);
const CODE: (&str, &[u8]) = (
    "graft-code",
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
);

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    for (key, bytes) in [UI, UI_SEMIBOLD, CODE] {
        defs.font_data
            .insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let chain = |faces: &[&str], family: FontFamily| -> Vec<String> {
        faces
            .iter()
            .map(|face| (*face).to_owned())
            .chain(defs.families[&family].iter().cloned())
            .collect()
    };
    let proportional = chain(&[UI.0], FontFamily::Proportional);
    let semibold = chain(&[UI_SEMIBOLD.0, UI.0], FontFamily::Proportional);
    let monospace = chain(&[CODE.0], FontFamily::Monospace);
    defs.families.insert(FontFamily::Proportional, proportional);
    defs.families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    defs.families.insert(FontFamily::Monospace, monospace);
    ctx.set_fonts(defs);
}
