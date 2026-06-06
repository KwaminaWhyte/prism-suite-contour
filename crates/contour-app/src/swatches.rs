//! A named colour library — the **Swatches** panel's model.
//!
//! A [`Swatch`] is a named straight-sRGB RGBA colour. A [`Swatches`] collection
//! is the document's palette: an ordered, name-unique list a user builds from
//! their artwork and clicks to paint fills and strokes, exactly the way
//! Illustrator's / Affinity's Swatches panel works.
//!
//! A swatch can be **global** (Illustrator's filled-corner swatch): when a
//! global swatch is recoloured, every fill or stroke in the document that used
//! its *previous* colour follows the edit. That recolour is expressed here as a
//! pure [`Swatches::recolor`] that hands back the `(old, new)` colour pair, so
//! the document layer can walk its shapes and remap — keeping all the colour
//! bookkeeping out of any egui / canvas state and unit-testable on its own.
//!
//! Everything in this module is pure and unit-tested; the inspector panel only
//! drives these operations and renders the result.

use serde::{Deserialize, Serialize};

/// Approximate equality for a straight-sRGB RGBA colour. Colours round-trip
/// through `u8` egui pickers, so an exact `==` is brittle; a half-a-channel
/// tolerance treats picker-equal colours as the same swatch colour.
pub fn colors_eq(a: [f32; 4], b: [f32; 4]) -> bool {
    const EPS: f32 = 1.0 / 512.0;
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() <= EPS)
}

/// One named colour in the document palette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    /// Unique, stable identifier (never reused within a document). Lets a global
    /// swatch be addressed independently of its (editable) name or colour.
    pub id: u64,
    /// Display name shown in the panel and its tooltip.
    pub name: String,
    /// Straight-sRGB RGBA colour (matching egui's channel order and the rest of
    /// the document model).
    pub color: [f32; 4],
    /// A **global** swatch propagates a recolour to every fill / stroke in the
    /// document that currently matches its colour (Illustrator's global swatch).
    /// A non-global swatch is a plain shortcut: editing it changes only the
    /// swatch, not the artwork. Additive (`#[serde(default)]`).
    #[serde(default)]
    pub global: bool,
}

impl Swatch {
    pub fn new(id: u64, name: impl Into<String>, color: [f32; 4]) -> Self {
        Self {
            id,
            name: name.into(),
            color,
            global: false,
        }
    }
}

/// The document's ordered colour palette. Names are kept unique (a clash is
/// disambiguated with a numeric suffix); ids are unique and never reused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Swatches {
    pub list: Vec<Swatch>,
}

impl Default for Swatches {
    fn default() -> Self {
        Self::starter()
    }
}

impl Swatches {
    /// The default starter palette a fresh document opens with — a small,
    /// Illustrator-like set of primaries plus black / white / a mid grey.
    pub fn starter() -> Self {
        let entries: [(&str, [f32; 4]); 8] = [
            ("White", [1.0, 1.0, 1.0, 1.0]),
            ("Black", [0.0, 0.0, 0.0, 1.0]),
            ("Grey", [0.5, 0.5, 0.5, 1.0]),
            ("Red", [0.90, 0.20, 0.20, 1.0]),
            ("Orange", [0.95, 0.60, 0.15, 1.0]),
            ("Yellow", [0.97, 0.85, 0.20, 1.0]),
            ("Green", [0.25, 0.70, 0.35, 1.0]),
            ("Blue", [0.27, 0.55, 0.85, 1.0]),
        ];
        let list = entries
            .iter()
            .enumerate()
            .map(|(i, (name, color))| Swatch::new(i as u64, *name, *color))
            .collect();
        Self { list }
    }

    /// Whether the palette is empty.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Number of swatches.
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
    /// number to a duplicate swatch name.
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        let base = if base.is_empty() { "Swatch" } else { base };
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

    /// Look up a swatch by id.
    pub fn get(&self, id: u64) -> Option<&Swatch> {
        self.list.iter().find(|s| s.id == id)
    }

    /// The id of the first swatch whose colour matches `color`, if any. Used to
    /// highlight the active swatch when the current paint already names one.
    pub fn id_for_color(&self, color: [f32; 4]) -> Option<u64> {
        self.list
            .iter()
            .find(|s| colors_eq(s.color, color))
            .map(|s| s.id)
    }

    /// Add a swatch for `color` with `name` (de-duplicated). When a swatch with
    /// an equal colour already exists its id is returned and nothing is added, so
    /// "add the current fill" twice doesn't pile up duplicates. Returns the id of
    /// the added-or-existing swatch.
    pub fn add(&mut self, name: &str, color: [f32; 4]) -> u64 {
        if let Some(id) = self.id_for_color(color) {
            return id;
        }
        let id = self.next_id();
        let name = self.unique_name(name);
        self.list.push(Swatch::new(id, name, color));
        id
    }

    /// Remove the swatch with `id`. Returns `true` if one was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|s| s.id != id);
        self.list.len() != before
    }

    /// Rename the swatch `id`, keeping names unique (a clash gets a numeric
    /// suffix). No-op for an unknown id. Returns the final stored name.
    pub fn rename(&mut self, id: u64, name: &str) -> Option<String> {
        // Compute the unique name against the *other* swatches (so renaming a
        // swatch to its own current name is a no-op, not "Name 2").
        let taken: Vec<String> = self
            .list
            .iter()
            .filter(|s| s.id != id)
            .map(|s| s.name.clone())
            .collect();
        let base = name.trim();
        let base = if base.is_empty() { "Swatch" } else { base };
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

    /// Toggle the swatch's `global` flag. No-op for an unknown id; returns the
    /// new state.
    pub fn set_global(&mut self, id: u64, global: bool) -> Option<bool> {
        let s = self.list.iter_mut().find(|s| s.id == id)?;
        s.global = global;
        Some(global)
    }

    /// Recolour the swatch `id` to `color`, returning the `(old, new)` colour
    /// pair **only when the swatch is global** (so the caller knows to remap the
    /// artwork) — and `None` for a non-global swatch (whose edit touches nothing
    /// but the swatch) or an unknown id / unchanged colour. Mutates the swatch in
    /// every case where the id is valid.
    pub fn recolor(&mut self, id: u64, color: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
        let s = self.list.iter_mut().find(|s| s.id == id)?;
        let old = s.color;
        s.color = color;
        if s.global && !colors_eq(old, color) {
            Some((old, color))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Swatches {
        /// An empty palette (test convenience).
        fn empty() -> Self {
            Self { list: Vec::new() }
        }
    }

    #[test]
    fn starter_palette_has_unique_ids_and_names() {
        let s = Swatches::starter();
        assert_eq!(s.len(), 8);
        let mut ids: Vec<u64> = s.list.iter().map(|x| x.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8, "ids unique");
        let mut names: Vec<&str> = s.list.iter().map(|x| x.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8, "names unique");
    }

    #[test]
    fn next_id_is_one_past_max() {
        let mut s = Swatches::empty();
        assert_eq!(s.next_id(), 0);
        s.add("A", [1.0, 0.0, 0.0, 1.0]);
        s.add("B", [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(s.next_id(), 2);
        // Removing the lowest id still gives one-past-max, never reusing 0.
        let id0 = s.list[0].id;
        s.remove(id0);
        assert_eq!(s.next_id(), 2);
    }

    #[test]
    fn add_dedups_by_color() {
        let mut s = Swatches::empty();
        let a = s.add("Red", [1.0, 0.0, 0.0, 1.0]);
        let b = s.add("Crimson", [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(a, b, "equal colour reuses the swatch");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn add_dedups_with_picker_rounding_tolerance() {
        let mut s = Swatches::empty();
        let a = s.add("Blue", [0.2, 0.4, 0.6, 1.0]);
        // A colour off by less than half a u8 channel is the same swatch.
        let b = s.add("Blue2", [0.2 + 1.0 / 1024.0, 0.4, 0.6, 1.0]);
        assert_eq!(a, b);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn unique_name_appends_a_number() {
        let mut s = Swatches::empty();
        s.add("Sky", [0.0, 0.0, 1.0, 1.0]);
        // Different colour, same base name → suffixed.
        assert_eq!(s.unique_name("Sky"), "Sky 2");
        s.list.push(Swatch::new(99, "Sky 2", [0.1; 4]));
        assert_eq!(s.unique_name("Sky"), "Sky 3");
        // A free name is returned untouched.
        assert_eq!(s.unique_name("Sea"), "Sea");
        // Empty base falls back to a default.
        assert_eq!(s.unique_name("   "), "Swatch");
    }

    #[test]
    fn rename_keeps_names_unique_but_allows_self_rename() {
        let mut s = Swatches::empty();
        let red = s.add("Red", [1.0, 0.0, 0.0, 1.0]);
        let green = s.add("Green", [0.0, 1.0, 0.0, 1.0]);
        // Renaming Green to the taken name "Red" disambiguates.
        assert_eq!(s.rename(green, "Red").as_deref(), Some("Red 2"));
        // Renaming Red to its own current name is a no-op (not "Red 2").
        assert_eq!(s.rename(red, "Red").as_deref(), Some("Red"));
        // Unknown id → None.
        assert_eq!(s.rename(12345, "X"), None);
    }

    #[test]
    fn remove_reports_whether_it_removed() {
        let mut s = Swatches::empty();
        let id = s.add("X", [0.5; 4]);
        assert!(s.remove(id));
        assert!(!s.remove(id), "second remove is a no-op");
        assert!(s.is_empty());
    }

    #[test]
    fn id_for_color_finds_a_named_swatch() {
        let s = Swatches::starter();
        let black = s.id_for_color([0.0, 0.0, 0.0, 1.0]).unwrap();
        assert_eq!(s.get(black).unwrap().name, "Black");
        assert!(s.id_for_color([0.123, 0.456, 0.789, 1.0]).is_none());
    }

    #[test]
    fn recolor_global_reports_old_new_pair() {
        let mut s = Swatches::empty();
        let id = s.add("Brand", [0.2, 0.2, 0.2, 1.0]);
        s.set_global(id, true);
        let pair = s.recolor(id, [0.8, 0.1, 0.1, 1.0]);
        assert_eq!(pair, Some(([0.2, 0.2, 0.2, 1.0], [0.8, 0.1, 0.1, 1.0])));
        // The swatch itself is updated.
        assert_eq!(s.get(id).unwrap().color, [0.8, 0.1, 0.1, 1.0]);
    }

    #[test]
    fn recolor_non_global_returns_none_but_still_edits() {
        let mut s = Swatches::empty();
        let id = s.add("Plain", [0.2, 0.2, 0.2, 1.0]);
        // Non-global: no remap pair, but the swatch colour changes.
        assert_eq!(s.recolor(id, [0.9, 0.9, 0.9, 1.0]), None);
        assert_eq!(s.get(id).unwrap().color, [0.9, 0.9, 0.9, 1.0]);
    }

    #[test]
    fn recolor_global_to_same_color_is_noop_pair() {
        let mut s = Swatches::empty();
        let id = s.add("Brand", [0.2, 0.2, 0.2, 1.0]);
        s.set_global(id, true);
        assert_eq!(s.recolor(id, [0.2, 0.2, 0.2, 1.0]), None);
    }
}
