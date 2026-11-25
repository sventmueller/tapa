//! # Line annotations
//!
//! ## Overview
//!
//! This module contains logic for tracking line annotations.
//!
use std::any::{Any, TypeId};
use std::cmp::Eq;
use std::collections::HashMap;
use std::hash::Hash;

/// Dango: one value per type, in place of the unmaintained `anymap2::SendSyncAnyMap`.
#[derive(Default)]
struct SendSyncAnyMap(HashMap<TypeId, Box<dyn Any + Send + Sync>>);

impl SendSyncAnyMap {
    fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.get(&TypeId::of::<T>())?.downcast_ref()
    }

    fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.0.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.0.insert(TypeId::of::<T>(), Box::new(value));
    }
}

/// Tracks different types of annotations for each line within a given buffer.
pub struct LineInfoStore<Line: Eq + Hash> {
    map: HashMap<Line, SendSyncAnyMap>,
}

impl<L> LineInfoStore<L>
where
    L: Eq + Hash,
{
    /// Create a new store.
    pub fn new() -> Self {
        let map = HashMap::new();

        LineInfoStore { map }
    }

    /// Get a reference to the annotation of type `T` on line `L`.
    pub fn get<T: Send + Sync + 'static>(&self, line: &L) -> Option<&T> {
        let info = self.map.get(line)?;

        return info.get();
    }

    /// Get a mutable reference to the annotation of type `T` on line `L`.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self, line: &L) -> Option<&mut T> {
        let info = self.map.get_mut(line)?;

        return info.get_mut();
    }

    /// Create or overwrite an annotation of type `T` on line `L`.
    pub fn set<T: Send + Sync + 'static>(&mut self, line: L, info: T) {
        if let Some(ref mut any) = self.map.get_mut(&line) {
            any.insert(info);
        } else {
            let mut any = SendSyncAnyMap::default();
            any.insert(info);
            self.map.insert(line, any);
        }
    }
}

impl<L> Default for LineInfoStore<L>
where
    L: Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}
