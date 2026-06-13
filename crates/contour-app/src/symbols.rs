//! A named **symbols** library plus the document's placed instances — the
//! Symbols panel's model.
//!
//! A [`Symbol`] is a reusable *master*: a name plus the set of [`Shape`]s that
//! make it up (Illustrator builds one from the current selection). The document
//! places [`SymbolInstance`]s of it — each a lightweight reference to a symbol id
//! carrying its own placement [`Affine`] (position / scale / rotation). An
//! instance is never stored as its own geometry; it is **resolved** on demand by
//! cloning the master shapes and pushing them through the instance transform
//! ([`Symbols::resolve`]). That single indirection is what makes
//! **edit-master propagation** fall out for free: editing a symbol's master
//! shapes changes what every instance resolves to, with no per-instance fix-up.
//!
//! Master shapes are stored verbatim in *document space as captured*; the
//! instance transform is applied on top, so the first instance placed with the
//! identity transform sits exactly where the artwork was when the symbol was
//! defined. Subsequent instances carry a translate / scale / rotate matrix.
//!
//! Everything in this module is pure and unit-tested; the inspector panel and
//! the canvas only drive these operations and render the resolved shapes.

use crate::document::Shape;
use crate::transform::Affine;
use serde::{Deserialize, Serialize};

/// One named master in the document's symbol library: an id, a display name, and
/// the master shape set. Resolving an instance clones `shapes` and transforms
/// the clones, so the master itself is never mutated by rendering.
///
/// No `PartialEq`: [`Shape`] is not `PartialEq`. Tests compare the serialized
/// form (which is what determinism / round-trip actually need).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Symbol {
    /// Unique, stable identifier (never reused within a document). Lets a symbol
    /// be addressed independently of its (editable) name, and lets an instance
    /// reference it.
    pub id: u64,
    /// Display name shown in the panel and its tooltip.
    pub name: String,
    /// The master shape set. Editing these shapes re-defines the symbol; every
    /// instance resolves through the new geometry.
    pub shapes: Vec<Shape>,
}

impl Symbol {
    pub fn new(id: u64, name: impl Into<String>, shapes: Vec<Shape>) -> Self {
        Self {
            id,
            name: name.into(),
            shapes,
        }
    }
}

/// One placed instance of a symbol: a reference to a [`Symbol::id`] plus the
/// instance's own placement matrix. The instance carries **no geometry** — it is
/// resolved against the live master via [`Symbols::resolve`], so a master edit
/// shows in every instance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolInstance {
    /// Unique, stable identifier for this placed instance.
    pub id: u64,
    /// The master this instance references.
    pub symbol: u64,
    /// Placement transform applied to the master's shapes at resolve time
    /// (position / scale / rotation / shear). Identity places the master exactly
    /// where it was captured.
    pub transform: Affine,
}

impl SymbolInstance {
    pub fn new(id: u64, symbol: u64, transform: Affine) -> Self {
        Self {
            id,
            symbol,
            transform,
        }
    }
}

/// The document's symbol library and its placed instances. Symbol names are kept
/// unique (a clash gets a numeric suffix); ids (for both symbols and instances)
/// are unique and never reused. Empty by default — a fresh document ships no
/// symbols.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Symbols {
    /// The ordered master library.
    pub list: Vec<Symbol>,
    /// The placed instances, in paint order (drawn over the plain shapes).
    pub instances: Vec<SymbolInstance>,
}

impl Symbols {
    /// Whether the library has no masters.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Number of masters in the library.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// The next free **symbol** id (one past the current maximum, so ids never
    /// collide and never reuse a removed slot's id within a session).
    pub fn next_symbol_id(&self) -> u64 {
        self.list.iter().map(|s| s.id).max().map_or(0, |m| m + 1)
    }

    /// The next free **instance** id (one past the current maximum).
    pub fn next_instance_id(&self) -> u64 {
        self.instances
            .iter()
            .map(|i| i.id)
            .max()
            .map_or(0, |m| m + 1)
    }

    /// A name not already in use: `base` if free, else `base 2`, `base 3`, …
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        let base = if base.is_empty() { "Symbol" } else { base };
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

    /// Look up a master by id.
    pub fn get(&self, id: u64) -> Option<&Symbol> {
        self.list.iter().find(|s| s.id == id)
    }

    /// Mutable lookup of a master by id — the edit-master path mutates the
    /// shapes here, and every instance follows on the next resolve.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Symbol> {
        self.list.iter_mut().find(|s| s.id == id)
    }

    /// Define a new symbol named `name` from `shapes` (de-duplicated name).
    /// Returns the id of the new symbol. The shapes are stored verbatim; the
    /// caller normally strips group/clip tags it does not want carried.
    pub fn add(&mut self, name: &str, shapes: Vec<Shape>) -> u64 {
        let id = self.next_symbol_id();
        let name = self.unique_name(name);
        self.list.push(Symbol::new(id, name, shapes));
        id
    }

    /// Remove the master `id` **and every instance of it**. Returns `true` if a
    /// master was removed. (Deleting a symbol can't leave dangling instances.)
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|s| s.id != id);
        let removed = self.list.len() != before;
        if removed {
            self.instances.retain(|i| i.symbol != id);
        }
        removed
    }

    /// Rename master `id`, keeping names unique. No-op for an unknown id.
    /// Returns the final stored name.
    pub fn rename(&mut self, id: u64, name: &str) -> Option<String> {
        let taken: Vec<String> = self
            .list
            .iter()
            .filter(|s| s.id != id)
            .map(|s| s.name.clone())
            .collect();
        let base = name.trim();
        let base = if base.is_empty() { "Symbol" } else { base };
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

    /// Replace the master shapes of symbol `id` (the **edit-master** path). The
    /// new geometry takes effect for every instance on the next resolve. No-op
    /// for an unknown id; returns whether it updated.
    pub fn set_master_shapes(&mut self, id: u64, shapes: Vec<Shape>) -> bool {
        match self.get_mut(id) {
            Some(sym) => {
                sym.shapes = shapes;
                true
            }
            None => false,
        }
    }

    /// Place a new instance of symbol `id` with placement `transform`. Returns
    /// the new instance id, or `None` if no such master exists (so a dangling
    /// instance can never be created).
    pub fn place(&mut self, id: u64, transform: Affine) -> Option<u64> {
        self.get(id)?;
        let inst_id = self.next_instance_id();
        self.instances
            .push(SymbolInstance::new(inst_id, id, transform));
        Some(inst_id)
    }

    /// Look up an instance by id.
    pub fn instance(&self, id: u64) -> Option<&SymbolInstance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Mutable lookup of an instance by id (to retransform it).
    pub fn instance_mut(&mut self, id: u64) -> Option<&mut SymbolInstance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }

    /// Remove instance `id` (an "un-place"; the master is untouched). Returns
    /// `true` if one was removed.
    pub fn remove_instance(&mut self, id: u64) -> bool {
        let before = self.instances.len();
        self.instances.retain(|i| i.id != id);
        self.instances.len() != before
    }

    /// **Resolve** an instance to its drawable shapes: the master's shapes,
    /// cloned and pushed through the instance transform. This is the pure core —
    /// the only place an instance becomes geometry. Returns an empty vector for a
    /// dangling instance (no such master). Because it reads the *live* master,
    /// editing the master changes the resolved output for every instance, which
    /// is exactly edit-master propagation.
    pub fn resolve(&self, instance: &SymbolInstance) -> Vec<Shape> {
        let Some(sym) = self.get(instance.symbol) else {
            return Vec::new();
        };
        resolve_shapes(&sym.shapes, &instance.transform)
    }

    /// Resolve every placed instance to drawable shapes, paired with the
    /// originating instance id (so the canvas can map a hit / selection back to
    /// the instance). Paint / export iterate this after the plain shapes.
    pub fn resolved_instances(&self) -> Vec<(u64, Vec<Shape>)> {
        self.instances
            .iter()
            .map(|inst| (inst.id, self.resolve(inst)))
            .collect()
    }
}

/// Clone `shapes` and apply `transform` to each — the pure transform step shared
/// by [`Symbols::resolve`]. Free function so it is testable without a library.
pub fn resolve_shapes(shapes: &[Shape], transform: &Affine) -> Vec<Shape> {
    shapes
        .iter()
        .map(|s| {
            let mut clone = s.clone();
            if !transform.is_identity() {
                clone.apply_affine(transform);
            }
            clone
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid red square at (0,0) 10×10.
    fn square() -> Shape {
        Shape::Rect {
            rect: [0.0, 0.0, 10.0, 10.0],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: Default::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
            omask: None,
            omask_path: false,
            omask_invert: false,
            blend: None,
            blend_step: false,
            name: None,
            locked: false,
            layer_color: None,
        }
    }

    /// A line from (0,0) to (10,0).
    fn line() -> Shape {
        Shape::Line {
            p0: (0.0, 0.0),
            p1: (10.0, 0.0),
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: Default::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
            omask: None,
            omask_path: false,
            omask_invert: false,
            blend: None,
            blend_step: false,
            name: None,
            locked: false,
            layer_color: None,
        }
    }

    fn rect_of(s: &Shape) -> [f32; 4] {
        match s {
            Shape::Rect { rect, .. } => *rect,
            _ => panic!("expected a rect"),
        }
    }

    /// Defining a symbol from N shapes adds one entry holding all N.
    #[test]
    fn add_defines_a_symbol_from_n_shapes() {
        let mut sym = Symbols::default();
        assert!(sym.is_empty());
        let id = sym.add("Star", vec![square(), line()]);
        assert_eq!(sym.len(), 1);
        let s = sym.get(id).unwrap();
        assert_eq!(s.name, "Star");
        assert_eq!(s.shapes.len(), 2);
    }

    /// Two instances at different transforms each resolve to the master shapes
    /// transformed accordingly.
    #[test]
    fn instances_resolve_through_their_own_transforms() {
        let mut sym = Symbols::default();
        let id = sym.add("Box", vec![square()]);

        // Instance A: moved +100,+50.
        let a = sym.place(id, Affine::translate(100.0, 50.0)).unwrap();
        // Instance B: scaled ×2 about the origin.
        let b = sym.place(id, Affine::scale(2.0, 2.0)).unwrap();

        let ra = sym.resolve(sym.instance(a).unwrap());
        let rb = sym.resolve(sym.instance(b).unwrap());

        // A: the 0,0 10×10 square shifted to 100,50.
        assert_eq!(rect_of(&ra[0]), [100.0, 50.0, 10.0, 10.0]);
        // B: doubled to 0,0 20×20.
        assert_eq!(rect_of(&rb[0]), [0.0, 0.0, 20.0, 20.0]);
    }

    /// Editing the master (move + recolor) changes **both** instances' resolved
    /// geometry and appearance — the core edit-master propagation guarantee.
    #[test]
    fn editing_master_propagates_to_all_instances() {
        let mut sym = Symbols::default();
        let id = sym.add("Box", vec![square()]);
        let a = sym.place(id, Affine::translate(100.0, 0.0)).unwrap();
        let b = sym.place(id, Affine::translate(0.0, 100.0)).unwrap();

        // Before: both resolve to a red square at their offset.
        for &i in &[a, b] {
            let r = sym.resolve(sym.instance(i).unwrap());
            assert_eq!(rect_of(&r[0])[2], 10.0, "10-wide before edit");
            match &r[0] {
                Shape::Rect { fill, .. } => assert_eq!(*fill, [1.0, 0.0, 0.0, 1.0]),
                _ => unreachable!(),
            }
        }

        // Edit the master: make the square 40 wide and blue.
        let mut new_master = square();
        if let Shape::Rect { rect, fill, .. } = &mut new_master {
            *rect = [0.0, 0.0, 40.0, 10.0];
            *fill = [0.0, 0.0, 1.0, 1.0];
        }
        assert!(sym.set_master_shapes(id, vec![new_master]));

        // After: both instances reflect the new geometry AND colour, each still
        // through its own transform.
        let ra = sym.resolve(sym.instance(a).unwrap());
        let rb = sym.resolve(sym.instance(b).unwrap());
        assert_eq!(rect_of(&ra[0]), [100.0, 0.0, 40.0, 10.0]);
        assert_eq!(rect_of(&rb[0]), [0.0, 100.0, 40.0, 10.0]);
        for r in [&ra, &rb] {
            match &r[0] {
                Shape::Rect { fill, .. } => assert_eq!(*fill, [0.0, 0.0, 1.0, 1.0]),
                _ => unreachable!(),
            }
        }
    }

    /// Removing an instance leaves the master and the other instances intact.
    #[test]
    fn remove_instance_keeps_master_and_siblings() {
        let mut sym = Symbols::default();
        let id = sym.add("Box", vec![square()]);
        let a = sym.place(id, Affine::IDENTITY).unwrap();
        let b = sym.place(id, Affine::translate(10.0, 0.0)).unwrap();
        assert!(sym.remove_instance(a));
        assert!(sym.instance(a).is_none());
        assert!(sym.instance(b).is_some());
        assert!(sym.get(id).is_some(), "master survives an un-place");
        assert!(!sym.remove_instance(a), "second remove is a no-op");
    }

    /// Deleting a symbol cascades: its instances go too (no dangling refs).
    #[test]
    fn deleting_a_symbol_removes_its_instances() {
        let mut sym = Symbols::default();
        let a_id = sym.add("A", vec![square()]);
        let b_id = sym.add("B", vec![line()]);
        sym.place(a_id, Affine::IDENTITY);
        sym.place(a_id, Affine::translate(5.0, 5.0));
        sym.place(b_id, Affine::IDENTITY);
        assert_eq!(sym.instances.len(), 3);

        assert!(sym.remove(a_id));
        // Only B's single instance remains.
        assert_eq!(sym.instances.len(), 1);
        assert_eq!(sym.instances[0].symbol, b_id);
    }

    /// A dangling instance (master gone) resolves to nothing rather than panic.
    #[test]
    fn dangling_instance_resolves_empty() {
        let sym = Symbols::default();
        let ghost = SymbolInstance::new(0, 999, Affine::IDENTITY);
        assert!(sym.resolve(&ghost).is_empty());
    }

    /// `place` refuses an unknown symbol id — instances are always well-formed.
    #[test]
    fn place_rejects_unknown_symbol() {
        let mut sym = Symbols::default();
        assert_eq!(sym.place(42, Affine::IDENTITY), None);
        assert!(sym.instances.is_empty());
    }

    /// Resolution is deterministic: the same library + instance yields identical
    /// geometry every time.
    #[test]
    fn resolution_is_deterministic() {
        let mut sym = Symbols::default();
        let id = sym.add("Box", vec![square(), line()]);
        sym.place(id, Affine::rotate(0.5).then(Affine::translate(7.0, 3.0)));
        // Shape has no PartialEq, so compare the serialized resolved geometry.
        let first = serde_json::to_string(&sym.resolved_instances()).unwrap();
        let second = serde_json::to_string(&sym.resolved_instances()).unwrap();
        assert_eq!(first, second);
    }

    /// Symbol ids are one-past-max and never reuse a removed id within a session.
    #[test]
    fn ids_never_reuse_within_a_session() {
        let mut sym = Symbols::default();
        assert_eq!(sym.next_symbol_id(), 0);
        let a = sym.add("A", vec![square()]);
        sym.add("B", vec![square()]);
        assert_eq!(sym.next_symbol_id(), 2);
        sym.remove(a);
        assert_eq!(sym.next_symbol_id(), 2, "removed id 0 is not reused");

        // Instance ids are tracked independently.
        let bid = sym.list[0].id;
        let i0 = sym.place(bid, Affine::IDENTITY).unwrap();
        assert_eq!(i0, 0);
        let i1 = sym.place(bid, Affine::IDENTITY).unwrap();
        assert_eq!(i1, 1);
        sym.remove_instance(i0);
        assert_eq!(sym.next_instance_id(), 2, "removed instance id not reused");
    }

    #[test]
    fn unique_name_appends_a_number() {
        let mut sym = Symbols::default();
        sym.add("Icon", vec![square()]);
        assert_eq!(sym.unique_name("Icon"), "Icon 2");
        assert_eq!(sym.unique_name("Other"), "Other");
        assert_eq!(sym.unique_name("   "), "Symbol");
    }

    #[test]
    fn rename_keeps_names_unique_but_allows_self_rename() {
        let mut sym = Symbols::default();
        let a = sym.add("Alpha", vec![square()]);
        let b = sym.add("Beta", vec![square()]);
        assert_eq!(sym.rename(b, "Alpha").as_deref(), Some("Alpha 2"));
        assert_eq!(sym.rename(a, "Alpha").as_deref(), Some("Alpha"));
        assert_eq!(sym.rename(12345, "X"), None);
    }

    /// The whole library + instances round-trips through serde unchanged.
    #[test]
    fn serde_round_trip_preserves_library_and_instances() {
        let mut sym = Symbols::default();
        let id = sym.add("Box", vec![square(), line()]);
        sym.place(id, Affine::translate(10.0, 20.0));
        sym.place(id, Affine::scale(2.0, 2.0));
        let json = serde_json::to_string(&sym).unwrap();
        let back: Symbols = serde_json::from_str(&json).unwrap();
        // Shape has no PartialEq; re-serializing the round-tripped value must
        // match the original JSON byte-for-byte.
        assert_eq!(json, serde_json::to_string(&back).unwrap());
        assert_eq!(back.list.len(), 1);
        assert_eq!(back.instances.len(), 2);
    }

    /// A default library is empty — the fallback an older document (no `symbols`
    /// key) deserializes into.
    #[test]
    fn default_library_is_empty() {
        let sym = Symbols::default();
        assert!(sym.is_empty());
        assert!(sym.instances.is_empty());
    }
}
