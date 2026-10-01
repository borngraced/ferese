use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use ferese_animation::{AnimatedValue, WindowGeometry};
use ferese_core::WorkspaceId;
use ferese_layout::{ColumnWidth, Rect, WindowId};

use crate::resize_transaction::ResizeTransaction;

/// Metadata with exactly the lifetime of a managed window. Layouts and focus
/// history reference its ID; they do not own or duplicate this metadata.
#[derive(Default)]
pub(crate) struct WindowRecord {
    pub geometry: Option<WindowGeometry>,
    pub resize: Option<ResizeTransaction>,
    pub focus: Option<crate::dimming::DimAnimation>,
    pub dimming: Option<crate::dimming::DimAnimation>,
    pub(super) closing: Option<super::ClosingAnimation>,
    pub world_x: Option<(WorkspaceId, AnimatedValue)>,
    pub coupled_width: Option<(WorkspaceId, AnimatedValue)>,
    pub maximized: bool,
    pub maximized_column_width: Option<ColumnWidth>,
    pub natural_floating_pending: bool,
    pub floating_memory: Option<crate::floating::Remembered>,
    pub placement_anchor: Option<Rect>,
    pub resize_anchor: Option<(bool, bool, Rect)>,
    pub column_width_pending: bool,
    pub rules_applied: bool,
}

/// The handle index is read-only outside this owner. Registering and removing a
/// handle always updates its record as well. IDs are never reused on remapping.
pub(crate) struct WindowRegistry<W> {
    ids: HashMap<W, WindowId>,
    pub(super) records: HashMap<WindowId, WindowRecord>,
    next_id: u64,
}

impl<W> Default for WindowRegistry<W> {
    fn default() -> Self {
        Self {
            ids: HashMap::new(),
            records: HashMap::new(),
            next_id: 1,
        }
    }
}

impl<W: Eq + Hash> WindowRegistry<W> {
    pub fn allocate_id(&mut self) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id = self.next_id.checked_add(1).expect("window IDs exhausted");

        id
    }

    pub fn register(&mut self, window: W, id: WindowId) {
        assert!(!self.ids.contains_key(&window), "window already registered");
        assert!(!self.records.contains_key(&id), "window ID already registered");

        self.ids.insert(window, id);
        self.records.insert(id, WindowRecord::default());
    }

    pub fn ids(&self) -> &HashMap<W, WindowId> {
        &self.ids
    }

    pub fn record(&self, id: WindowId) -> Option<&WindowRecord> {
        self.records.get(&id)
    }

    pub fn record_mut(&mut self, id: WindowId) -> Option<&mut WindowRecord> {
        self.records.get_mut(&id)
    }

    pub fn records_mut(&mut self) -> impl Iterator<Item = (&WindowId, &mut WindowRecord)> {
        self.records.iter_mut()
    }

    pub fn records(&self) -> impl Iterator<Item = (&WindowId, &WindowRecord)> {
        self.records.iter()
    }

    pub fn take_world_positions(&mut self) -> HashMap<WindowId, (WorkspaceId, AnimatedValue)> {
        self.records
            .iter_mut()
            .filter_map(|(id, record)| Some((*id, record.world_x.take()?)))
            .collect()
    }

    pub fn geometry(&self, id: &WindowId) -> Option<&WindowGeometry> {
        self.records.get(id)?.geometry.as_ref()
    }

    pub fn geometry_mut(&mut self, id: &WindowId) -> Option<&mut WindowGeometry> {
        self.records.get_mut(id)?.geometry.as_mut()
    }

    pub fn geometries(&self) -> impl Iterator<Item = (&WindowId, &WindowGeometry)> {
        self.records
            .iter()
            .filter_map(|(id, record)| Some((id, record.geometry.as_ref()?)))
    }

    pub fn set_geometry(&mut self, id: WindowId, geometry: WindowGeometry) {
        self.update(id, |record| record.geometry = Some(geometry));
    }

    pub fn transaction(&self, id: &WindowId) -> Option<&ResizeTransaction> {
        self.records.get(id)?.resize.as_ref()
    }

    pub fn set_transaction(&mut self, id: WindowId, transaction: ResizeTransaction) {
        self.update(id, |record| record.resize = Some(transaction));
    }

    pub fn clear_transaction(&mut self, id: &WindowId) {
        self.update(*id, |record| record.resize = None);
    }

    pub fn resizing(&self) -> impl Iterator<Item = &WindowId> {
        self.records
            .iter()
            .filter_map(|(id, record)| record.resize.is_some().then_some(id))
    }

    pub fn expire_transactions(&mut self, now: std::time::Duration) {
        for (id, record) in &mut self.records {
            if record.resize.is_some_and(|transaction| transaction.expired(now)) {
                tracing::warn!(?id, "resize presentation deadline reached");
                record.resize = None;
            }
        }
    }

    pub fn update<R>(&mut self, id: WindowId, update: impl FnOnce(&mut WindowRecord) -> R) -> Option<R> {
        self.records.get_mut(&id).map(update)
    }

    pub fn remove(&mut self, window: &W) -> Option<WindowId> {
        let id = self.ids.remove(window)?;
        self.records.remove(&id);

        Some(id)
    }

    pub fn take_column_width_requests(&mut self) -> HashSet<WindowId> {
        self.records
            .iter_mut()
            .filter_map(|(id, record)| std::mem::take(&mut record.column_width_pending).then_some(*id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmap_drops_metadata_and_late_updates_cannot_resurrect_it() {
        let mut windows = WindowRegistry::default();
        let id = windows.allocate_id();
        windows.register("surface", id);
        windows.update(id, |record| {
            record.maximized = true;
            record.resize_anchor = Some((true, false, Rect::new(0., 0., 800., 600.)));
            record.column_width_pending = true;
            record.rules_applied = true;
            record.focus = Some(crate::dimming::DimAnimation::new(1.0));
            record.closing = Some(super::super::ClosingAnimation::default());
            record.world_x = Some((WorkspaceId(1), AnimatedValue::new(50.0)));
            record.coupled_width = Some((WorkspaceId(1), AnimatedValue::new(800.0)));
        });
        let geometry = WindowGeometry::new(Rect::new(0., 0., 800., 600.), None);
        let resize = ResizeTransaction::new(12.into(), std::time::Duration::ZERO);
        windows.set_geometry(id, geometry);
        windows.set_transaction(id, resize);

        assert_eq!(windows.remove(&"surface"), Some(id));
        assert_eq!(windows.remove(&"surface"), None);
        assert!(windows.record(id).is_none());
        assert!(windows.update(id, |record| record.maximized = true).is_none());

        windows.set_geometry(id, geometry);
        windows.set_transaction(id, resize);

        assert!(windows.geometry(&id).is_none());
        assert!(windows.transaction(&id).is_none());
        assert_eq!(windows.resizing().count(), 0);
        assert!(windows.ids().is_empty());
        assert!(windows.take_column_width_requests().is_empty());
    }

    #[test]
    fn remap_has_new_identity_and_does_not_inherit_old_requests() {
        let mut windows = WindowRegistry::default();
        let old = windows.allocate_id();
        windows.register("surface", old);
        windows.update(old, |record| record.rules_applied = true);
        windows.remove(&"surface");

        let new = windows.allocate_id();
        windows.register("surface", new);
        assert_ne!(old, new);
        assert!(
            windows
                .update(old, |record| record.column_width_pending = true)
                .is_none()
        );
        assert!(!windows.record(new).unwrap().rules_applied);
        assert!(windows.take_column_width_requests().is_empty());
    }

    #[test]
    fn pending_width_changes_are_consumed_once_and_survive_other_window_removal() {
        let mut windows = WindowRegistry::default();
        let first = windows.allocate_id();
        let second = windows.allocate_id();
        windows.register("first", first);
        windows.register("second", second);
        windows.update(first, |record| record.column_width_pending = true);
        windows.update(second, |record| record.column_width_pending = true);

        windows.remove(&"first");

        assert_eq!(windows.take_column_width_requests(), HashSet::from([second]));
        assert!(windows.take_column_width_requests().is_empty());
        assert!(windows.record(second).is_some());
    }

    #[test]
    fn expired_resize_releases_only_its_barrier_and_preserves_geometry() {
        use std::time::Duration;

        let mut windows = WindowRegistry::default();
        let first = windows.allocate_id();
        let second = windows.allocate_id();
        windows.register("first", first);
        windows.register("second", second);
        let geometry = WindowGeometry::new(Rect::new(0., 0., 800., 600.), None);
        windows.set_geometry(first, geometry);
        windows.set_transaction(first, ResizeTransaction::new(1.into(), Duration::ZERO));
        windows.set_transaction(second, ResizeTransaction::new(2.into(), Duration::from_millis(100)));

        windows.expire_transactions(Duration::from_millis(300));

        assert_eq!(windows.geometry(&first), Some(&geometry));
        assert!(windows.transaction(&first).is_none());
        assert!(windows.transaction(&second).unwrap().accepts(Some(2.into())));
        assert_eq!(windows.resizing().copied().collect::<Vec<_>>(), vec![second]);
    }
}
