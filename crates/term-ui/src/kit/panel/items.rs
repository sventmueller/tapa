//! The objects a panel's outline view holds for its rows: one per key, the same one for as long as the panel keeps the
//! key, since the outline view knows its rows, their selection and whether they are open by the object's identity.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::NSObjectProtocol;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `RowItem` does not implement Drop.
    /// The object an outline view holds for one row, carrying the row's key.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermRowItem"]
    #[ivars = Box<dyn Any>]
    pub struct RowItem;

    unsafe impl NSObjectProtocol for RowItem {}
);

/// The row items of one outline view, by the key of the row each stands for.
pub struct RowItems<K> {
    items: RefCell<HashMap<K, Retained<RowItem>>>,
}

impl<K> Default for RowItems<K> {
    fn default() -> Self {
        RowItems {
            items: RefCell::new(HashMap::new()),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static> RowItems<K> {
    /// The item that stands for `key`, made the first time it is asked for.
    pub fn item(&self, mtm: MainThreadMarker, key: &K) -> Retained<RowItem> {
        if let Some(item) = self.get(key) {
            return item;
        }
        let item = RowItem::alloc(mtm).set_ivars(Box::new(key.clone()) as Box<dyn Any>);
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        let item: Retained<RowItem> = unsafe { msg_send![super(item), init] };
        self.items.borrow_mut().insert(key.clone(), item.clone());
        item
    }

    /// The item that stands for `key`, if one was made and kept.
    pub fn get(&self, key: &K) -> Option<Retained<RowItem>> {
        self.items.borrow().get(key).cloned()
    }

    /// The key `item` stands for, if it is a row item of this kind of key.
    pub fn key_of(item: &AnyObject) -> Option<K> {
        item.downcast_ref::<RowItem>()?.ivars().downcast_ref::<K>().cloned()
    }

    /// Lets go of the items whose key `live` no longer knows, once the outline view read its rows anew.
    pub fn keep(&self, mut live: impl FnMut(&K) -> bool) {
        self.items.borrow_mut().retain(|key, _| live(key));
    }
}
