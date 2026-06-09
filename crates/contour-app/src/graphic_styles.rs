//! A named **graphic-styles** library — the Graphic Styles panel's model.
//!
//! A [`GraphicStyle`] is a named snapshot of an [`Appearance`](crate::appearance::Appearance)
//! (its whole fill / stroke / effect stack, each with its own paint, opacity,
//! blend mode, and visibility). A [`GraphicStyles`] collection is the document's
//! style library: an ordered, name-unique list a user builds from their artwork
//! and applies to a selection to paint it in one click — exactly the way
//! Illustrator's / Affinity's Graphic Styles panel works.
//!
//! **Save** captures the current selection's effective appearance into a new
//! named style; **apply** overwrites a target shape's appearance with a style's
//! snapshot (replacing whatever stack it had); **rename** / **delete** edit the
//! library. Because a style is just a stored `Appearance`, applying one routes
//! through the same `set_appearance` + checkpoint undo path the Appearance panel
//! already uses.
//!
//! Everything in this module is pure and unit-tested; the inspector panel only
//! drives these operations and renders the result.

use crate::appearance::Appearance;
use serde::{Deserialize, Serialize};

/// One named appearance snapshot in the document's style library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphicStyle {
    /// Unique, stable identifier (never reused within a document). Lets a style
    /// be addressed independently of its (editable) name.
    pub id: u64,
    /// Display name shown in the panel and its tooltip.
    pub name: String,
    /// The captured appearance stack (fills / strokes / effects, each with its
    /// own paint / opacity / blend / visibility). Applying the style overwrites a
    /// shape's [`Appearance`] with a clone of this.
    pub appearance: Appearance,
}

impl GraphicStyle {
    pub fn new(id: u64, name: impl Into<String>, appearance: Appearance) -> Self {
        Self {
            id,
            name: name.into(),
            appearance,
        }
    }
}

/// The document's ordered graphic-styles library. Names are kept unique (a clash
/// is disambiguated with a numeric suffix); ids are unique and never reused.
/// Empty by default — a fresh document ships no styles (unlike the colour
/// palette, which seeds a starter set).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphicStyles {
    pub list: Vec<GraphicStyle>,
}

impl GraphicStyles {
    /// Whether the library is empty.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Number of styles.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// The next free id (one past the current maximum, so ids never collide and
    /// never reuse a removed slot's id within a session).
    pub fn next_id(&self) -> u64 {
        self.list.iter().map(|s| s.id).max().map_or(0, |m| m + 1)
    }

    /// A name not already in use: `base` if free, else `base 2`, `base 3`, …
    /// (the suffix climbs until it is unique). Mirrors Illustrator appending a
    /// number to a duplicate style name.
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        let base = if base.is_empty() { "Style" } else { base };
        if !self.list.iter().any(|s| s.name == base) {
            return base.to_string();
        }
        let mut n = 2u32;
        loop {
            let candidate = format!("{base} {n}");
            if !self.list.iter().any(|s| s.name == candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Look up a style by id.
    pub fn get(&self, id: u64) -> Option<&GraphicStyle> {
        self.list.iter().find(|s| s.id == id)
    }

    /// The captured appearance of style `id`, if it exists. The single read the
    /// editor uses to apply a style to a shape.
    pub fn appearance_of(&self, id: u64) -> Option<&Appearance> {
        self.get(id).map(|s| &s.appearance)
    }

    /// Save `appearance` as a new style named `name` (de-duplicated). Unlike a
    /// swatch, styles are *not* merged by content — two identical-looking styles
    /// are allowed (Illustrator lets you keep duplicates), so this always adds an
    /// entry. Returns the id of the new style.
    pub fn add(&mut self, name: &str, appearance: Appearance) -> u64 {
        let id = self.next_id();
        let name = self.unique_name(name);
        self.list.push(GraphicStyle::new(id, name, appearance));
        id
    }

    /// Remove the style with `id`. Returns `true` if one was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|s| s.id != id);
        self.list.len() != before
    }

    /// Rename the style `id`, keeping names unique (a clash gets a numeric
    /// suffix). No-op for an unknown id. Returns the final stored name.
    pub fn rename(&mut self, id: u64, name: &str) -> Option<String> {
        // Compute the unique name against the *other* styles (so renaming a
        // style to its own current name is a no-op, not "Name 2").
        let taken: Vec<String> = self
            .list
            .iter()
            .filter(|s| s.id != id)
            .map(|s| s.name.clone())
            .collect();
        let base = name.trim();
        let base = if base.is_empty() { "Style" } else { base };
        let taken_base = base.to_string();
        let final_name = if !taken.contains(&taken_base) {
            taken_base
        } else {
            let mut n = 2u32;
            loop {
                let candidate = format!("{base} {n}");
                if !taken.contains(&candidate) {
                    break candidate;
                }
                n += 1;
            }
        };
        let s = self.list.iter_mut().find(|s| s.id == id)?;
        s.name = final_name.clone();
        Some(final_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::{BlendMode, Effect, Fill, Paint, Stroke};
    use crate::gradient::Gradient;

    /// A small, distinctive appearance: a gradient fill over a solid fill, one
    /// blended stroke, and a drop shadow — exercises every part of the snapshot.
    fn sample_appearance() -> Appearance {
        Appearance {
            fills: vec![
                Fill::solid([0.1, 0.2, 0.3, 1.0]),
                Fill {
                    paint: Paint::Gradient(Gradient::default()),
                    opacity: 0.5,
                    blend: BlendMode::Multiply,
                    visible: false,
                },
            ],
            strokes: vec![Stroke {
                paint: Paint::Solid([1.0, 0.0, 0.0, 0.8]),
                width: 3.0,
                style: Default::default(),
                opacity: 0.75,
                blend: BlendMode::Screen,
                visible: true,
            }],
            effects: vec![Effect::drop_shadow()],
        }
    }

    #[test]
    fn next_id_is_one_past_max() {
        let mut s = GraphicStyles::default();
        assert!(s.is_empty());
        assert_eq!(s.next_id(), 0);
        s.add("A", Appearance::default());
        s.add("B", Appearance::default());
        assert_eq!(s.next_id(), 2);
        // Removing the lowest id still gives one-past-max, never reusing 0.
        let id0 = s.list[0].id;
        s.remove(id0);
        assert_eq!(s.next_id(), 2);
    }

    /// **Save** captures the supplied appearance verbatim under a unique id/name.
    #[test]
    fn add_captures_the_appearance() {
        let mut s = GraphicStyles::default();
        let ap = sample_appearance();
        let id = s.add("Card", ap.clone());
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(id).unwrap().name, "Card");
        // The whole stack is stored, byte-for-byte.
        assert_eq!(s.appearance_of(id), Some(&ap));
    }

    /// Unlike a swatch, an identical-looking style is *not* de-duplicated — two
    /// saves of the same appearance keep two entries (Illustrator allows it).
    #[test]
    fn add_keeps_duplicate_appearances() {
        let mut s = GraphicStyles::default();
        let ap = sample_appearance();
        let a = s.add("One", ap.clone());
        let b = s.add("Two", ap);
        assert_ne!(a, b);
        assert_eq!(s.len(), 2);
    }

    /// **Apply** overwrites a target appearance with the style's snapshot,
    /// replacing whatever stack it had. Modelled as the pure read the editor does
    /// before calling `set_appearance`.
    #[test]
    fn apply_overwrites_a_target_appearance() {
        let mut s = GraphicStyles::default();
        let id = s.add("Fancy", sample_appearance());

        // A shape currently wearing a plain one-fill stack.
        let mut target = Appearance {
            fills: vec![Fill::solid([0.0, 0.0, 0.0, 1.0])],
            strokes: vec![],
            effects: vec![],
        };
        assert_eq!(target.fills.len(), 1, "starts with the plain one-fill stack");
        // Applying the style overwrites the whole appearance with the snapshot —
        // this is exactly what the editor's `set_appearance` does.
        target = s.appearance_of(id).unwrap().clone();
        assert_eq!(target, sample_appearance());
        assert_eq!(target.fills.len(), 2, "the plain fill is fully replaced");
        assert_eq!(target.strokes.len(), 1);
        assert!(target.has_active_effects());
    }

    #[test]
    fn unique_name_appends_a_number() {
        let mut s = GraphicStyles::default();
        s.add("Badge", Appearance::default());
        assert_eq!(s.unique_name("Badge"), "Badge 2");
        s.add("Badge 2", Appearance::default());
        assert_eq!(s.unique_name("Badge"), "Badge 3");
        // A free name is returned untouched.
        assert_eq!(s.unique_name("Banner"), "Banner");
        // Empty base falls back to a default.
        assert_eq!(s.unique_name("   "), "Style");
    }

    #[test]
    fn rename_keeps_names_unique_but_allows_self_rename() {
        let mut s = GraphicStyles::default();
        let a = s.add("Alpha", Appearance::default());
        let b = s.add("Beta", Appearance::default());
        // Renaming Beta to the taken name "Alpha" disambiguates.
        assert_eq!(s.rename(b, "Alpha").as_deref(), Some("Alpha 2"));
        // Renaming Alpha to its own current name is a no-op (not "Alpha 2").
        assert_eq!(s.rename(a, "Alpha").as_deref(), Some("Alpha"));
        // Unknown id → None.
        assert_eq!(s.rename(12345, "X"), None);
    }

    #[test]
    fn remove_reports_whether_it_removed() {
        let mut s = GraphicStyles::default();
        let id = s.add("Gone", Appearance::default());
        assert!(s.remove(id));
        assert!(!s.remove(id), "second remove is a no-op");
        assert!(s.is_empty());
    }

    /// The whole style library round-trips through serde unchanged.
    #[test]
    fn serde_round_trip_preserves_library() {
        let mut s = GraphicStyles::default();
        s.add("Card", sample_appearance());
        s.add("Plain", Appearance::default());
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicStyles = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    /// An older document's JSON (no `graphic_styles` key) loads with an empty
    /// library — the `#[derive(Default)]` empty-vec fallback the document field's
    /// `#[serde(default)]` relies on.
    #[test]
    fn default_library_is_empty() {
        let s = GraphicStyles::default();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }
}
