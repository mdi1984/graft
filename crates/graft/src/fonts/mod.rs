//! Fonts: Inter for UI text and JetBrains Mono for code are built into the
//! executable. The user can pick other fonts in Settings > Appearance; those
//! are read from their files, and the built-in fonts (then egui's own) fill
//! in the characters they lack.

mod scan;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use egui::epaint::text::VariationCoords;
use egui::{FontData, FontDefinitions, FontFamily, FontTweak};
use serde::{Deserialize, Serialize};

pub use scan::{Family, inspect, scan};

/// Font family name for semibold UI text (egui has no font weights).
pub const SEMIBOLD: &str = "semibold";

const UI: (&str, &[u8]) = (
    "graft-ui",
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
);
const UI_SEMIBOLD: (&str, &[u8]) = (
    "graft-ui-semibold",
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
);
const CODE: (&str, &[u8]) = (
    "graft-code",
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
);

/// Which of the two fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Everything but code.
    Ui,
    /// Diffs, commit hashes, the console.
    Code,
}

impl Kind {
    /// The name of the font built into Graft.
    pub fn built_in(self) -> &'static str {
        match self {
            Kind::Ui => "Inter",
            Kind::Code => "JetBrains Mono",
        }
    }
}

/// One face in a font file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Face {
    pub path: PathBuf,
    /// Which font of a collection (.ttc); 0 for a single font.
    #[serde(default)]
    pub index: u32,
    /// For a variable font: the weight to draw it at.
    #[serde(default)]
    pub weight: Option<u16>,
}

/// A font the user picked, or the built-in one. Saved with the files it was
/// found in, so starting Graft needs no search for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct FontChoice {
    /// The family's name; empty for the font built into Graft.
    pub family: String,
    /// `None` for the built-in font.
    pub regular: Option<Face>,
    /// Headings (interface font only); without one they use `regular`.
    pub semibold: Option<Face>,
}

impl FontChoice {
    pub fn is_built_in(&self) -> bool {
        self.regular.is_none()
    }

    /// The family's name as the user reads it.
    pub fn name(&self, kind: Kind) -> &str {
        if self.is_built_in() {
            kind.built_in()
        } else {
            &self.family
        }
    }

    /// Both are the same family: the same name, or both built in.
    pub fn same_family(&self, other: &FontChoice) -> bool {
        self.is_built_in() == other.is_built_in() && self.family.eq_ignore_ascii_case(&other.family)
    }
}

/// The interface font and the code font.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fonts {
    pub ui: FontChoice,
    pub code: FontChoice,
}

/// A pair of fonts picked with one click (Settings > Appearance).
pub struct Style {
    pub name: &'static str,
    /// Why someone would pick it.
    pub note: &'static str,
    /// Family names; empty for the built-in font.
    ui: &'static str,
    code: &'static str,
}

pub const STYLES: [Style; 3] = [
    Style {
        name: "Graft",
        note: "Inter and JetBrains Mono, built in",
        ui: "",
        code: "",
    },
    Style {
        name: "Windows",
        note: "Segoe UI and Consolas, the look before 1.0.10",
        ui: "Segoe UI",
        code: "Consolas",
    },
    Style {
        name: "Windows 11",
        note: "Segoe UI Variable and Cascadia Mono",
        ui: "Segoe UI Variable",
        code: "Cascadia Mono",
    },
];

impl Style {
    /// Whether `fonts` are this style's.
    pub fn is(&self, fonts: &Fonts) -> bool {
        let matches = |choice: &FontChoice, name: &str| {
            if name.is_empty() {
                choice.is_built_in()
            } else {
                !choice.is_built_in() && choice.family.eq_ignore_ascii_case(name)
            }
        };
        matches(&fonts.ui, self.ui) && matches(&fonts.code, self.code)
    }

    /// This style's fonts, found among `installed`; otherwise which one this
    /// PC lacks.
    pub fn fonts(&self, installed: &[Family]) -> Result<Fonts, String> {
        let find = |name: &str| -> Result<FontChoice, String> {
            if name.is_empty() {
                return Ok(FontChoice::default());
            }
            installed
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(name))
                .map(Family::choice)
                .ok_or_else(|| format!("{name} is not installed on this PC."))
        };
        Ok(Fonts {
            ui: find(self.ui)?,
            code: find(self.code)?,
        })
    }
}

/// The chosen fonts, read from their files. `None` where the built-in font
/// is used.
#[derive(Default)]
pub struct Loaded {
    ui: Option<FontData>,
    ui_semibold: Option<FontData>,
    code: Option<FontData>,
}

/// Read the chosen fonts' files. A font that can't be read is left out, so
/// the built-in one shows, and the errors say which and why.
pub fn load(fonts: &Fonts) -> (Loaded, Vec<String>) {
    let mut errors = Vec::new();
    let mut read = |choice: &FontChoice, face: Option<&Face>| {
        let face = face?;
        read_face(face)
            .inspect_err(|error| {
                errors.push(format!(
                    "Could not load the font {} ({error:#}). Graft shows its built-in font instead.",
                    choice.family
                ));
            })
            .ok()
    };
    let ui = read(&fonts.ui, fonts.ui.regular.as_ref());
    let ui_semibold = match ui {
        Some(_) => read(&fonts.ui, fonts.ui.semibold.as_ref()),
        None => None,
    };
    let code = read(&fonts.code, fonts.code.regular.as_ref());
    let loaded = Loaded {
        ui,
        ui_semibold,
        code,
    };
    (loaded, errors)
}

fn read_face(face: &Face) -> anyhow::Result<FontData> {
    let bytes =
        std::fs::read(&face.path).with_context(|| format!("reading {}", face.path.display()))?;
    scan::check_text_font(&bytes, face.index)
        .with_context(|| format!("{} is no font Graft can use", face.path.display()))?;
    let mut data = FontData::from_blob(Arc::new(bytes), face.index);
    if let Some(weight) = face.weight {
        data = data.tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", f32::from(weight))]),
            ..FontTweak::default()
        });
    }
    Ok(data)
}

/// Give egui the fonts: the chosen ones first, then the built-in ones, then
/// egui's own, for characters the earlier ones lack.
pub fn install(ctx: &egui::Context, loaded: &Loaded) {
    let mut defs = FontDefinitions::default();
    let builtin_proportional = defs.families[&FontFamily::Proportional].clone();
    let builtin_monospace = defs.families[&FontFamily::Monospace].clone();
    for (key, bytes) in [UI, UI_SEMIBOLD, CODE] {
        defs.font_data
            .insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let mut chosen = |key: &str, data: &Option<FontData>| {
        let data = data.clone()?;
        defs.font_data.insert(key.to_owned(), Arc::new(data));
        Some(key.to_owned())
    };
    let ui = chosen("chosen-ui", &loaded.ui);
    let ui_semibold = chosen("chosen-ui-semibold", &loaded.ui_semibold).or_else(|| ui.clone());
    let code = chosen("chosen-code", &loaded.code);

    let chain = |first: Option<String>, built_in: &[&str], egui: &[String]| -> Vec<String> {
        first
            .into_iter()
            .chain(built_in.iter().map(|face| (*face).to_owned()))
            .chain(egui.iter().cloned())
            .collect()
    };
    defs.families.insert(
        FontFamily::Proportional,
        chain(ui, &[UI.0], &builtin_proportional),
    );
    defs.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        chain(ui_semibold, &[UI_SEMIBOLD.0, UI.0], &builtin_proportional),
    );
    defs.families.insert(
        FontFamily::Monospace,
        chain(code, &[CODE.0], &builtin_monospace),
    );
    ctx.set_fonts(defs);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(path: &str) -> Option<Face> {
        Some(Face {
            path: PathBuf::from(path),
            index: 0,
            weight: None,
        })
    }

    #[test]
    fn built_in_fonts_load_without_errors() {
        let (loaded, errors) = load(&Fonts::default());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(loaded.ui.is_none() && loaded.code.is_none());
    }

    #[test]
    fn a_missing_font_falls_back_and_says_so() {
        let fonts = Fonts {
            ui: FontChoice {
                family: "Gone Sans".into(),
                regular: face(r"C:\nowhere\gone.ttf"),
                semibold: None,
            },
            code: FontChoice::default(),
        };
        let (loaded, errors) = load(&fonts);
        assert!(loaded.ui.is_none());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Gone Sans"), "{}", errors[0]);
    }

    #[test]
    fn a_file_that_is_no_font_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.ttf");
        std::fs::write(&path, b"not a font").unwrap();
        let fonts = Fonts {
            ui: FontChoice::default(),
            code: FontChoice {
                family: "Fake".into(),
                regular: face(path.to_str().unwrap()),
                semibold: None,
            },
        };
        let (loaded, errors) = load(&fonts);
        assert!(loaded.code.is_none());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn styles_match_by_family_name() {
        let graft = &STYLES[0];
        assert!(graft.is(&Fonts::default()));
        let windows = &STYLES[1];
        let fonts = Fonts {
            ui: FontChoice {
                family: "segoe ui".into(),
                regular: face("a.ttf"),
                semibold: None,
            },
            code: FontChoice {
                family: "Consolas".into(),
                regular: face("b.ttf"),
                semibold: None,
            },
        };
        assert!(windows.is(&fonts));
        assert!(!graft.is(&fonts));
        assert_eq!(
            windows.fonts(&[]).unwrap_err(),
            "Segoe UI is not installed on this PC."
        );
    }
}
