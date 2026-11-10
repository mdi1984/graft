//! The fonts on this PC, for the font lists in Settings: every family that
//! can show text, with its regular and semibold faces.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use skrifa::raw::{FileRef, TableProvider as _};
use skrifa::{FontRef, MetadataProvider as _, Tag, instance::LocationRef, instance::Size};

use super::{Face, FontChoice};

/// A font family, as the font lists show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    /// Every character is equally wide: the code font list shows only these.
    pub monospace: bool,
    regular: Face,
    semibold: Option<Face>,
}

impl Family {
    pub fn choice(&self) -> FontChoice {
        FontChoice {
            family: self.name.clone(),
            regular: Some(self.regular.clone()),
            semibold: self.semibold.clone(),
        }
    }
}

/// Every family in Windows' font folders (for all users and the current
/// user), sorted by name. Files that can't be read are skipped. Reads many
/// files: run it as a job.
pub fn scan() -> Vec<Family> {
    let mut dirs = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        dirs.push(PathBuf::from(windows).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
    }
    scan_dirs(&dirs)
}

fn scan_dirs(dirs: &[PathBuf]) -> Vec<Family> {
    let faces = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_font_file(path))
        .flat_map(|path| faces_in(&path).unwrap_or_default());
    group(faces)
}

/// The families in a font file the user picked.
pub fn inspect(path: &Path) -> anyhow::Result<Vec<Family>> {
    let families = group(faces_in(path)?.into_iter());
    if families.is_empty() {
        bail!(
            "{} has no font Graft can show text with.",
            path.file_name().unwrap_or_default().display()
        );
    }
    Ok(families)
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["ttf", "otf", "ttc", "otc"].contains(&e.to_ascii_lowercase().as_str()))
}

/// Can face `index` of `bytes` show text? Fails for symbol fonts and files
/// that are no font.
pub(super) fn check_text_font(bytes: &[u8], index: u32) -> anyhow::Result<()> {
    let font = FontRef::from_index(bytes, index).context("not a font file")?;
    if !shows_text(&font) {
        bail!("it has no letters");
    }
    Ok(())
}

/// One face found in a file.
struct Found {
    family: String,
    face: Face,
    weight: f32,
    italic: bool,
    /// Distance from normal width (condensed or expanded).
    stretch: f32,
    monospace: bool,
    /// A variable font's weight range.
    weights: Option<(f32, f32)>,
}

fn faces_in(path: &Path) -> anyhow::Result<Vec<Found>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: the map is only read while this function runs. Font files in
    // the font folders are not changed while in use; if one is, the worst
    // outcome is a font that fails to parse.
    let map = unsafe { memmap2::Mmap::map(&file) }
        .with_context(|| format!("reading {}", path.display()))?;
    let file = FileRef::new(&map).with_context(|| format!("{} is no font file", path.display()))?;
    let found = file
        .fonts()
        .enumerate()
        .filter_map(|(index, font)| {
            let font = font.ok()?;
            let index = u32::try_from(index).ok()?;
            found_face(path, index, &font)
        })
        .collect();
    Ok(found)
}

fn found_face(path: &Path, index: u32, font: &FontRef) -> Option<Found> {
    if !shows_text(font) {
        return None;
    }
    let name = |id| {
        font.localized_strings(id)
            .english_or_first()
            .map(|s| s.chars().collect::<String>())
            .filter(|s| !s.trim().is_empty())
    };
    let family = name(skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME)
        .or_else(|| name(skrifa::string::StringId::FAMILY_NAME))?;
    // Fonts named with a dot are hidden system fonts.
    if family.starts_with('.') {
        return None;
    }
    let attributes = font.attributes();
    let weights = font
        .axes()
        .iter()
        .find(|axis| axis.tag() == Tag::new(b"wght"))
        .map(|axis| (axis.min_value(), axis.max_value()));
    Some(Found {
        family: family.trim().to_owned(),
        face: Face {
            path: path.to_path_buf(),
            index,
            weight: None,
        },
        weight: attributes.weight.value(),
        italic: attributes.style != skrifa::attribute::Style::Normal,
        stretch: (attributes.stretch.ratio() - 1.0).abs(),
        monospace: is_monospace(font),
        weights,
    })
}

/// Letters and digits: not a symbol or icon font.
fn shows_text(font: &FontRef) -> bool {
    let charmap = font.charmap();
    ['a', 'A', '0']
        .into_iter()
        .all(|c| charmap.map(c).is_some())
}

/// The narrow and the wide letters are equally wide. Checked by measuring,
/// since some fonts don't set the flag that says so.
fn is_monospace(font: &FontRef) -> bool {
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let widths: Option<Vec<f32>> = ['i', 'l', '.', 'm', 'W', '0']
        .into_iter()
        .map(|c| metrics.advance_width(charmap.map(c)?))
        .collect();
    match widths {
        Some(w) => w[0] > 0.0 && w.iter().all(|x| (x - w[0]).abs() < 0.5),
        // A letter is missing: trust the flag fonts set for this.
        None => font.post().is_ok_and(|post| post.is_fixed_pitch() != 0),
    }
}

/// Group faces into families: for each, the upright face nearest to normal
/// width and regular weight, and the semibold face (weight 600, or the
/// variable font drawn at 600).
fn group(faces: impl Iterator<Item = Found>) -> Vec<Family> {
    let mut by_name: BTreeMap<String, Vec<Found>> = BTreeMap::new();
    for face in faces {
        by_name
            .entry(face.family.to_lowercase())
            .or_default()
            .push(face);
    }
    by_name
        .into_values()
        .filter_map(|faces| family(&faces))
        .collect()
}

fn family(faces: &[Found]) -> Option<Family> {
    let upright = || faces.iter().filter(|f| !f.italic);
    let regular = upright().min_by(|a, b| {
        let key = |f: &Found| (f.stretch, (f.weight - 400.0).abs());
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let (regular_face, semibold) = match regular.weights {
        Some((min, max)) => {
            let at = |weight: f32| Face {
                weight: Some(weight.clamp(min, max).round() as u16),
                ..regular.face.clone()
            };
            let semibold = (max >= 550.0).then(|| at(600.0));
            (at(400.0), semibold)
        }
        None => {
            let semibold = upright()
                .filter(|f| f.stretch == regular.stretch && (550.0..=700.0).contains(&f.weight))
                .min_by(|a, b| {
                    let key = |f: &Found| (f.weight - 600.0).abs();
                    key(a)
                        .partial_cmp(&key(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|f| f.face.clone());
            (regular.face.clone(), semibold)
        }
    };
    Some(Family {
        name: regular.family.clone(),
        monospace: regular.monospace,
        regular: regular_face,
        semibold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts")
    }

    #[test]
    fn finds_the_families_in_a_folder_with_their_faces() {
        let families = scan_dirs(&[assets(), PathBuf::from("does-not-exist")]);
        let names: Vec<&str> = families.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Inter", "JetBrains Mono"]);

        let inter = &families[0];
        assert!(!inter.monospace);
        assert!(inter.regular.path.ends_with("Inter-Regular.ttf"));
        let semibold = inter.semibold.as_ref().expect("Inter has a semibold face");
        assert!(semibold.path.ends_with("Inter-SemiBold.ttf"));

        let mono = &families[1];
        assert!(mono.monospace);
        assert!(mono.semibold.is_none());
    }

    #[test]
    fn a_file_that_is_no_font_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.ttf");
        std::fs::write(&path, b"hello").unwrap();
        assert!(inspect(&path).is_err());
        assert!(scan_dirs(&[dir.path().to_path_buf()]).is_empty());
    }

    #[test]
    fn a_picked_file_gives_its_family() {
        let families = inspect(&assets().join("JetBrainsMono-Regular.ttf")).unwrap();
        assert_eq!(families.len(), 1);
        let choice = families[0].choice();
        assert_eq!(choice.family, "JetBrains Mono");
        assert!(!choice.is_built_in());
    }
}
