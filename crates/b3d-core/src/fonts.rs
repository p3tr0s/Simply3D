//! Installed fonts: a sorted, de-duplicated list of "Family Style" entries.

use std::sync::Arc;

pub struct FontEntry {
    /// "Arial", "Arial Bold", "Segoe UI Semibold Italic"…
    pub label: String,
    pub family: String,
    id: Option<fontdb::ID>,
}

pub struct FontLibrary {
    db: fontdb::Database,
    pub entries: Vec<FontEntry>,
    builtin: Option<(String, Arc<Vec<u8>>)>,
}

fn weight_name(w: u16) -> &'static str {
    match w {
        0..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    }
}

impl FontLibrary {
    /// Scan the system font folders. `builtin` (name, TrueType data) is always listed
    /// last so text works even on a machine without usable fonts.
    pub fn system(builtin: Option<(&str, &[u8])>) -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut entries: Vec<FontEntry> = db
            .faces()
            .filter_map(|f| {
                let family = f.families.first()?.0.clone();
                let mut label = family.clone();
                let w = weight_name(f.weight.0);
                if !w.is_empty() && !family.to_lowercase().contains(&w.to_lowercase()) {
                    label.push(' ');
                    label.push_str(w);
                }
                if f.style != fontdb::Style::Normal {
                    label.push_str(" Italic");
                }
                Some(FontEntry { label, family, id: Some(f.id) })
            })
            .collect();
        entries.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        entries.dedup_by(|a, b| a.label == b.label);
        let builtin = builtin.map(|(n, d)| (n.to_string(), Arc::new(d.to_vec())));
        if let Some((n, _)) = &builtin {
            entries.push(FontEntry { label: n.clone(), family: n.clone(), id: None });
        }
        FontLibrary { db, entries, builtin }
    }

    pub fn find(&self, label: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.label == label)
    }

    /// A sensible default: Arial, Segoe UI, DejaVu Sans, Liberation Sans… else the first.
    pub fn default_index(&self) -> Option<usize> {
        ["Arial", "Segoe UI", "DejaVu Sans", "Liberation Sans", "Helvetica", "Noto Sans", "Ubuntu"]
            .iter()
            .find_map(|n| self.find(n))
            .or(if self.entries.is_empty() { None } else { Some(0) })
    }

    /// Font file bytes and face index for an entry.
    pub fn data(&self, i: usize) -> Option<(Arc<Vec<u8>>, u32)> {
        let e = self.entries.get(i)?;
        match e.id {
            Some(id) => self.db.with_face_data(id, |d, idx| (Arc::new(d.to_vec()), idx)),
            None => self.builtin.as_ref().map(|b| (b.1.clone(), 0)),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_and_loads() {
        let lib = super::FontLibrary::system(None);
        if let Some(i) = lib.default_index() {
            let (data, idx) = lib.data(i).unwrap();
            assert!(ttf_parser::Face::parse(&data, idx).is_ok());
        }
    }
}
