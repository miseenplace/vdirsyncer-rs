use serde::{Deserialize, Serialize};

use crate::base::{Collection, Storage};

/// A pair of storages which are to be kept synchronised.
///
/// Use [`Plan::for_storage_pair`](crate::sync::plan::Plan::for_storage_pair) to plan (and later
/// execute) the synchronisation itself..
pub struct StoragePair<'a> {
    pub(crate) storage_a: &'a mut dyn Storage,
    pub(crate) storage_b: &'a mut dyn Storage,
    pub(crate) previous_state_a: &'a StorageState,
    pub(crate) previous_state_b: &'a StorageState,
    pub(crate) collection_names: &'a Vec<String>,
    pub(crate) current_state_a: StorageState,
    pub(crate) current_state_b: StorageState,
}

impl StoragePair<'_> {
    /// Create a new instance for two given storages.
    ///
    /// Only actions required to synchronise the specified colletions will be planned. If there is
    /// no known previous state for a storage, an empty one should be provided.
    ///
    /// Executes all read operations required to determine the current state of both storages.
    ///
    /// # Errors
    ///
    /// If there are any errors determining the current state of either storage.
    // TODO: use a builder pattern to allow building these but querying later?
    pub async fn new<'a>(
        storage_a: &'a mut dyn Storage,
        storage_b: &'a mut dyn Storage,
        previous_state_a: &'a StorageState,
        previous_state_b: &'a StorageState,
        collection_names: &'a Vec<String>,
    ) -> crate::Result<StoragePair<'a>> {
        let current_state_a =
            StorageState::current_for_storage(previous_state_a, storage_a, collection_names)
                .await?;
        let current_state_b =
            StorageState::current_for_storage(previous_state_b, storage_b, collection_names)
                .await?;

        Ok(StoragePair {
            storage_a,
            storage_b,
            previous_state_a,
            previous_state_b,
            collection_names,
            current_state_a,
            current_state_b,
        })
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone)]
pub(crate) struct ItemState {
    pub(crate) href: String,
    pub(crate) uid: String,
    pub(crate) etag: String,
    pub(crate) hash: String,
}

/// The state of a storage at a given point in time.
///
/// Generally, this should be treated as opaque data and not modified by consumers of this library.
/// It should, however, be serialised and saved into persistent storages between synchronisation
/// operations.
#[derive(Serialize, Deserialize, Clone)]
pub struct StorageState {
    collections: Vec<CollectionState>,
}

impl StorageState {
    /// Returns the state of the collection with the given name.
    ///
    /// Returns `None` if the collection does not exist in this state (which is
    /// distinct from the collection existing and being empty).
    #[must_use]
    #[inline]
    pub(crate) fn get_collection(&self, name: &str) -> Option<&CollectionState> {
        self.collections.iter().find(|c| c.collection_name == name)
    }

    #[must_use]
    #[inline]
    pub(crate) fn get_collection_mut(&mut self, name: &str) -> Option<&mut CollectionState> {
        self.collections
            .iter_mut()
            .find(|c| c.collection_name == name)
    }

    async fn current_for_storage(
        previous_state: &StorageState,
        storage: &dyn Storage,
        collection_names: &Vec<String>,
    ) -> crate::Result<StorageState> {
        let mut collection_states = Vec::new();

        // TODO: need to run a discovery here to map names to hrefs.
        //       from this point on, i CAN have collection instances.
        let collections = storage.discover_collections().await?;

        for name in collection_names {
            let Some(collection) = collections.iter().find(|c| {
                if let Ok(id) = storage.collection_id(c) {
                    id == *name
                } else {
                    false
                }
            }) else {
                // If a collection does not exist the there is no state for it.
                continue
            };

            let previous = previous_state.get_collection(name);
            let state =
                CollectionState::current_for_storage(previous, storage, collection, name.clone())
                    .await;
            collection_states.push(state?);
        }

        Ok(StorageState {
            collections: collection_states,
        })
    }

    pub(crate) fn add_collection(&mut self, name: String, href: String) {
        self.collections.push({
            CollectionState {
                collection_href: href,
                collection_name: name,
                items: Vec::new(),
            }
        });
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct CollectionState {
    // TODO: keep etag (to delete when empty).
    pub(crate) collection_href: String, // TODO: reference?
    pub(crate) collection_name: String, // TODO: reference?
    // TODO: keep the collection instance itself?
    pub(crate) items: Vec<ItemState>,
}

impl CollectionState {
    async fn current_for_storage(
        previous_state: Option<&CollectionState>,
        storage: &dyn Storage,
        collection: &Collection,
        collection_name: String,
    ) -> crate::Result<Self> {
        let mut state = CollectionState {
            collection_name,
            // TODO: to_string here was a quick hack
            collection_href: collection.href().to_string(),
            items: Vec::new(),
        };
        let mut prefetch = Vec::new();

        // TODO: I could special case if previous_state is None and just get_all

        for item_ref in storage.list_items(collection).await? {
            if let Some(ps) = previous_state {
                if let Some(p) = ps.get_item_by_href(&item_ref.href) {
                    if p.etag == item_ref.etag {
                        state.items.push(ItemState {
                            href: item_ref.href,
                            etag: item_ref.etag,
                            uid: p.uid.clone(),
                            hash: p.hash.clone(),
                        });
                        continue;
                    }
                }
            }

            prefetch.push(item_ref.href);
        }
        let prefetched = storage
            .get_many_items(
                collection,
                prefetch
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .await?;
        for (href, item, etag) in prefetched {
            state.items.push(ItemState {
                href,
                uid: item.ident(),
                etag,
                hash: item.hash(),
            });
        }

        Ok(state)
    }

    #[inline]
    pub(crate) fn get_item_by_uid(&self, uid: &String) -> Option<&ItemState> {
        self.items.iter().find(|i| i.uid == **uid)
    }

    #[inline]
    pub(crate) fn get_item_by_uid_mut(&mut self, uid: &String) -> Option<&mut ItemState> {
        self.items.iter_mut().find(|i| i.uid == **uid)
    }

    #[inline]
    pub(crate) fn get_item_by_href(&self, href: &String) -> Option<&ItemState> {
        self.items.iter().find(|i| i.href == **href)
    }
}

/// A transition that has occurred to a pair of items or collections.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    /// Mutated or created.
    Changed,
    /// Deleted.
    Deleted,
    /// The item exists and has not changed.
    NoChange,
    /// The item does not exist and did not exist before.
    Absent,
}

impl Change {
    #[must_use]
    pub(crate) fn for_item(
        current: Option<&CollectionState>,
        previous: Option<&CollectionState>,
        uid: &String,
    ) -> Change {
        match (current, previous) {
            (Some(c), Some(p)) => {
                let c_item_state = c.items.iter().find(|i| i.uid == *uid);
                let p_item_state = p.items.iter().find(|i| i.uid == *uid);

                if let (Some(ci), Some(pi)) = (c_item_state, p_item_state) {
                    if ci.uid == pi.uid && ci.etag == pi.etag && ci.hash == pi.hash {
                        Change::NoChange
                    } else {
                        Change::Changed
                    }
                } else if c_item_state.is_some() {
                    Change::Changed
                } else if p_item_state.is_some() {
                    Change::Deleted
                } else {
                    Change::Absent
                }
            }
            (Some(c), None) => {
                if c.items.iter().any(|i| i.uid == *uid) {
                    Change::Changed
                } else {
                    Change::Absent
                }
            }
            (None, Some(_)) => Change::Deleted,
            (None, None) => Change::Absent,
        }
    }

    #[must_use]
    pub(crate) fn for_collection(
        current: Option<&CollectionState>,
        previous: Option<&CollectionState>,
    ) -> Change {
        match (current, previous) {
            (None, None) => Change::Absent,
            (None, Some(_)) => Change::Deleted,
            (Some(_), None) => Change::Changed,
            // TODO: Ignores meta; considers collections immutable:
            // they might change etag (or meta!?!?!)
            (Some(_), Some(_)) => Change::NoChange,
        }
    }
}
