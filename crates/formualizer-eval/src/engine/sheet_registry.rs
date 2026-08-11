use std::collections::HashMap;

use crate::SheetId;

/// Case-fold a sheet name for the secondary lookup index. Unicode
/// `to_lowercase` to match `normalize_name_key` (named ranges/tables use the
/// same folding); ASCII-only folding would make sheets the odd one out.
fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// Sheet name ↔ id registry.
///
/// Excel resolves sheet names case-insensitively (`=data!A1` reaches a sheet
/// named `Data`, and the formula bar rewrites to the display spelling), so
/// lookups probe the exact-spelling map first and fall back to a case-folded
/// index. Exact-first keeps any pre-existing pair of sheets differing only by
/// case behaving exactly as before (each spelling resolves to itself); the
/// folded index only decides otherwise-missing lookups, first insertion wins.
#[derive(Default, Debug)]
pub struct SheetRegistry {
    id_by_name: HashMap<String, SheetId>,
    /// Case-folded name → id, first-inserted-wins. Kept in sync by
    /// `remove`/`rename` re-pointing a shared folded key at a surviving sheet.
    id_by_folded: HashMap<String, SheetId>,
    name_by_id: Vec<String>,
}

impl SheetRegistry {
    pub fn new() -> Self {
        SheetRegistry::default()
    }

    /// Re-point (or clear) the folded entry for `folded` after the sheet it
    /// pointed at was removed/renamed: any surviving sheet with the same
    /// folded name takes it over.
    fn repoint_folded(&mut self, folded: &str, vacated_id: SheetId) {
        if self.id_by_folded.get(folded) != Some(&vacated_id) {
            return;
        }
        let survivor = self
            .name_by_id
            .iter()
            .enumerate()
            .find(|(id, name)| *id as SheetId != vacated_id && !name.is_empty() && fold(name) == folded)
            .map(|(id, _)| id as SheetId);
        match survivor {
            Some(id) => {
                self.id_by_folded.insert(folded.to_string(), id);
            }
            None => {
                self.id_by_folded.remove(folded);
            }
        }
    }

    pub fn id_for(&mut self, name: &str) -> SheetId {
        if let Some(&id) = self.id_by_name.get(name) {
            return id;
        }
        // A case-variant of an existing sheet resolves to it instead of
        // minting a phantom sheet (Excel forbids case-colliding names, so a
        // reference-driven `id_for` here is a lookup, not a creation).
        if let Some(&id) = self.id_by_folded.get(&fold(name)) {
            return id;
        }

        let id = self.name_by_id.len() as SheetId;
        self.name_by_id.push(name.to_string());
        self.id_by_name.insert(name.to_string(), id);
        self.id_by_folded.entry(fold(name)).or_insert(id);
        id
    }

    pub fn name(&self, id: SheetId) -> &str {
        if (id as usize) < self.name_by_id.len() {
            &self.name_by_id[id as usize]
        } else {
            ""
        }
    }

    pub fn get_id(&self, name: &str) -> Option<SheetId> {
        self.id_by_name
            .get(name)
            .or_else(|| self.id_by_folded.get(&fold(name)))
            .copied()
    }

    /// Count active sheets without cloning sheet names.
    pub fn active_len(&self) -> usize {
        self.name_by_id
            .iter()
            .filter(|name| !name.is_empty())
            .count()
    }

    /// Excel-style 1-based active sheet position for a sheet id.
    pub fn active_position_by_id(&self, id: SheetId) -> Option<usize> {
        let idx = id as usize;
        if idx >= self.name_by_id.len() || self.name_by_id[idx].is_empty() {
            return None;
        }
        Some(
            self.name_by_id
                .iter()
                .take(idx + 1)
                .filter(|name| !name.is_empty())
                .count(),
        )
    }

    /// Excel-style 1-based active sheet position for a sheet name.
    pub fn active_position(&self, name: &str) -> Option<usize> {
        self.get_id(name)
            .and_then(|id| self.active_position_by_id(id))
    }

    /// Inclusive count of active sheets between two sheet names.
    pub fn active_span_len(&self, first: &str, last: &str) -> Option<usize> {
        let a = self.active_position(first)?;
        let b = self.active_position(last)?;
        Some(a.abs_diff(b) + 1)
    }

    /// Get all sheet IDs and names (excluding removed sheets)
    pub fn all_sheets(&self) -> Vec<(SheetId, String)> {
        self.name_by_id
            .iter()
            .enumerate()
            .filter(|(_, name)| !name.is_empty())
            .map(|(id, name)| (id as SheetId, name.clone()))
            .collect()
    }

    /// Remove a sheet from the registry
    /// Note: This doesn't actually free the ID, it just marks it as removed
    pub fn remove(&mut self, id: SheetId) -> Result<(), formualizer_common::ExcelError> {
        use formualizer_common::{ExcelError, ExcelErrorKind};

        // Check if the ID exists
        if id as usize >= self.name_by_id.len() {
            return Err(
                ExcelError::new(ExcelErrorKind::Value).with_message("Sheet ID does not exist")
            );
        }

        // Get the name to remove from id_by_name
        let name = self.name_by_id[id as usize].clone();
        if name.is_empty() {
            // Already removed
            return Ok(());
        }

        // Remove from id_by_name mapping
        self.id_by_name.remove(&name);

        // Mark as removed in name_by_id (we can't actually remove it to preserve IDs)
        self.name_by_id[id as usize] = String::new();

        self.repoint_folded(&fold(&name), id);

        Ok(())
    }

    /// Rename a sheet
    pub fn rename(
        &mut self,
        id: SheetId,
        new_name: &str,
    ) -> Result<(), formualizer_common::ExcelError> {
        use formualizer_common::{ExcelError, ExcelErrorKind};

        // Check if the ID exists
        if id as usize >= self.name_by_id.len() {
            return Err(
                ExcelError::new(ExcelErrorKind::Value).with_message("Sheet ID does not exist")
            );
        }

        // Get the old name
        let old_name = self.name_by_id[id as usize].clone();

        // Check if new name is already taken by another sheet (exact spelling
        // only — rejecting case-collisions is a follow-up gated on callers
        // surfacing the error instead of discarding it).
        if let Some(&existing_id) = self.id_by_name.get(new_name)
            && existing_id != id
        {
            return Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message(format!("Sheet name '{new_name}' already exists")));
        }

        // Remove old name mapping
        self.id_by_name.remove(&old_name);

        // Update to new name
        self.name_by_id[id as usize] = new_name.to_string();
        self.id_by_name.insert(new_name.to_string(), id);

        // Folded index: vacate the old key (re-pointing it at any surviving
        // case-sibling), then claim the new key if free. A case-only rename
        // (Data → DATA) re-points the shared key back at this id.
        self.repoint_folded(&fold(&old_name), id);
        self.id_by_folded.entry(fold(new_name)).or_insert(id);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_with_exact_priority() {
        let mut reg = SheetRegistry::new();
        let data = reg.id_for("Data");
        assert_eq!(reg.get_id("data"), Some(data));
        assert_eq!(reg.get_id("DATA"), Some(data));
        assert_eq!(reg.id_for("dAtA"), data, "id_for resolves instead of minting a phantom");
        assert_eq!(reg.active_len(), 1);

        // A pre-existing case-variant pair keeps exact-spelling resolution.
        reg.name_by_id.push("data".to_string());
        reg.id_by_name.insert("data".to_string(), 1);
        assert_eq!(reg.get_id("Data"), Some(data));
        assert_eq!(reg.get_id("data"), Some(1));
    }

    #[test]
    fn remove_repoints_folded_key_at_surviving_case_sibling() {
        let mut reg = SheetRegistry::new();
        let a = reg.id_for("Data");
        // Force a case-variant pair (as a pre-fix document could contain).
        reg.name_by_id.push("DATA".to_string());
        reg.id_by_name.insert("DATA".to_string(), 1);
        reg.remove(a).unwrap();
        assert_eq!(reg.get_id("data"), Some(1), "folded key re-points at the survivor");
        reg.remove(1).unwrap();
        assert_eq!(reg.get_id("data"), None);
    }

    #[test]
    fn case_only_rename_keeps_folded_resolution() {
        let mut reg = SheetRegistry::new();
        let id = reg.id_for("Data");
        reg.rename(id, "DATA").unwrap();
        assert_eq!(reg.name(id), "DATA");
        assert_eq!(reg.get_id("data"), Some(id));
        assert_eq!(reg.get_id("Data"), Some(id));
    }

    #[test]
    fn rename_moves_folded_key_and_frees_old_one() {
        let mut reg = SheetRegistry::new();
        let id = reg.id_for("Data");
        reg.rename(id, "Numbers").unwrap();
        assert_eq!(reg.get_id("data"), None);
        assert_eq!(reg.get_id("numbers"), Some(id));
    }

    #[test]
    fn unicode_names_fold() {
        let mut reg = SheetRegistry::new();
        let id = reg.id_for("Résumé");
        assert_eq!(reg.get_id("résumé"), Some(id));
        assert_eq!(reg.get_id("RÉSUMÉ"), Some(id));
    }
}
