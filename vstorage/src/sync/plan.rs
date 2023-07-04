// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Components to plan a synchronisation.

use crate::base::{Item, Storage};
use crate::sync::pair::{Change, CollectionState, StoragePair, StorageState};
use itertools::Itertools;
use log::trace;
use std::collections::HashMap;
use std::fmt::Display;

use super::pair::{CollectionMapping, ItemState};

#[derive(Debug)]
pub enum SyncResource {
    Item { uid: String },
    Collection { name: String },
}

/// An error synchronising two items between storages.
#[derive(Debug)]
pub struct SynchronizationError {
    action: Action,
    resource: SyncResource,
    error: Box<dyn std::error::Error + 'static>,
}

impl SynchronizationError {
    /// The action that failed to execute.
    #[must_use]
    pub fn action(&self) -> &Action {
        &self.action
    }

    /// The resource that failed to execute.
    #[must_use]
    pub fn resource(&self) -> &SyncResource {
        &self.resource
    }
}

impl Display for SynchronizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            // TODO: check formatting of resource.
            "Error performing {:?} on {:?}: {}",
            self.action, self.resource, self.error
        )
    }
}

impl std::error::Error for SynchronizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.error)
    }
}

/// An action to executing when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum Action {
    // TODO: keep href of items that need to be acted upon?
    NoOp,
    CopyToA,
    CopyToB,
    DeleteInA,
    DeleteInB,
    Conflict, // TODO: content might still match on both sides
}

impl Action {
    /// Return the correct action given a pair of changes.
    #[must_use]
    fn from_changes(left: Change, right: Change) -> Action {
        match (left, right) {
            (Change::Changed, Change::Changed) => Action::Conflict,
            (Change::NoChange, Change::Deleted) => Action::DeleteInA,
            (Change::Deleted, Change::NoChange) => Action::DeleteInB,
            (Change::Deleted | Change::NoChange | Change::Absent, Change::Changed)
            | (Change::Absent, Change::NoChange) => Action::CopyToA,
            (Change::Changed, Change::Deleted | Change::NoChange | Change::Absent)
            | (Change::NoChange, Change::Absent) => Action::CopyToB,
            (Change::Deleted | Change::Absent, Change::Deleted | Change::Absent)
            | (Change::NoChange, Change::NoChange) => Action::NoOp,
        }
    }

    #[inline]
    async fn execute_on_item<I: Item>(
        &self,
        uid: &str,
        storage_a: &mut dyn Storage<I>,
        storage_b: &mut dyn Storage<I>,
        state_a: Option<&mut CollectionState>,
        state_b: Option<&mut CollectionState>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Action::NoOp => {}
            Action::CopyToB => {
                copy_item(
                    state_a.ok_or("state a is missing")?,
                    state_b.ok_or("state b is missing")?,
                    storage_a,
                    storage_b,
                    uid,
                )
                .await?;
            }
            Action::CopyToA => {
                copy_item(
                    state_b.ok_or("state b is missing")?,
                    state_a.ok_or("state a is missing")?,
                    storage_b,
                    storage_a,
                    uid,
                )
                .await?;
            }
            Action::DeleteInA => {
                delete_item(
                    state_a.ok_or("collection is missing from state a")?,
                    storage_a,
                    uid,
                )
                .await?;
            }
            Action::DeleteInB => {
                delete_item(
                    state_b.ok_or("collection is missing from state b")?,
                    storage_b,
                    uid,
                )
                .await?;
            }
            Action::Conflict => todo!("conflict resolution"),
        }

        Ok(())
    }
}

async fn copy_item<I: Item>(
    src_state: &CollectionState,
    dst_state: &mut CollectionState,
    src_storage: &dyn Storage<I>,
    dst_storage: &mut dyn Storage<I>,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let col_a = src_storage.open_collection(&src_state.collection_href)?;

    let item_state = src_state.get_item_by_uid(uid).ok_or("item is missing")?;
    let (item, _) = src_storage.get_item(&col_a, &item_state.href).await?;

    let col = dst_storage.open_collection(&dst_state.collection_href)?;

    if let Some(dst_item_state) = dst_state.get_item_by_uid_mut(uid) {
        trace!("Updating {uid}");
        let new_etag = dst_storage
            .update_item(&col, &dst_item_state.href, &dst_item_state.etag, &item)
            .await?;
        dst_item_state.etag = new_etag;
        dst_item_state.hash = item.hash();
    } else {
        trace!("Creating {uid}");
        let new_ref = dst_storage.add_item(&col, &item).await?;
        dst_state.items.push(ItemState {
            href: new_ref.href,
            uid: uid.to_string(),
            etag: new_ref.etag,
            hash: item.hash(),
        });
    };

    Ok(())
}

async fn delete_item<I: Item>(
    state: &mut CollectionState,
    storage: &mut dyn Storage<I>,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let col = storage.open_collection(&state.collection_href)?;
    let pos = state
        .items
        .iter()
        .position(|i| i.uid == *uid)
        .ok_or("item pending deletion is missing from state")?;
    let item_state = &state.items[pos];

    storage
        .delete_item(&col, &item_state.href, &item_state.etag)
        .await?;

    state.items.swap_remove(pos);

    Ok(())
}

/// A series of actions that would synchronise a pair of storages.
#[derive(Debug)]
pub struct Plan {
    collection_plans: Vec<CollectionPlan>,
}

impl Plan {
    /// Create a plan to synchronise both storages.
    ///
    /// Compares the previous and current state of both storages and calculate all actions required
    /// to bring them into a synchronised state.
    #[must_use]
    pub fn for_storage_pair<I>(pair: &'_ StoragePair<'_, I>) -> Plan {
        // TODO: this method's implementation is not performant; it mostly "just works"
        //       Performance will be tweaked at a later date. In particular, we need a
        //       fully functioning system to properly benchmark different approaches.

        let mut collection_plans = Vec::new();
        for collection in &pair.collections {
            let cur_a = pair
                .current_state_a
                .find_collection_state(collection.name_a());
            let cur_b = pair
                .current_state_b
                .find_collection_state(collection.name_b());
            let prev_a = pair
                .previous_state_a
                .find_collection_state(collection.name_a());
            let prev_b = pair
                .previous_state_b
                .find_collection_state(collection.name_b());

            let plan = CollectionPlan::new(collection.clone(), prev_a, cur_a, prev_b, cur_b);
            collection_plans.push(plan);
        }

        Plan { collection_plans }
    }

    /// Executes a synchronization plan.
    ///
    /// FIXME: These docs are out of date!
    ///
    /// Always returns a final state, regardless of what changes were applied.
    /// The `FinalState` will include the error that forced aborting, if any. If
    /// the error is not None, then both storages may still be out of sync.
    pub async fn execute<I: Item>(&self, pair: &mut StoragePair<'_, I>) -> FinalState {
        let mut final_state = FinalState {
            state_a: pair.current_state_a.clone(),
            state_b: pair.current_state_b.clone(),
            errors: Vec::new(),
        };
        let storage_a = &mut pair.storage_a;
        let storage_b = &mut pair.storage_b;

        for cp in &self.collection_plans {
            let mut delete_collection_in_a = false;
            let mut delete_collection_in_b = false;
            match cp.collection_action {
                Action::NoOp => {}
                Action::CopyToB => {
                    create_collection(
                        *storage_b,
                        cp.mapping.name_b(),
                        &mut final_state.state_b,
                        &mut final_state.errors,
                        &cp.collection_action,
                    )
                    .await;
                }
                Action::CopyToA => {
                    create_collection(
                        *storage_a,
                        cp.mapping.name_a(),
                        &mut final_state.state_a,
                        &mut final_state.errors,
                        &cp.collection_action,
                    )
                    .await;
                }
                Action::Conflict => {
                    final_state.errors.push(SynchronizationError {
                        action: cp.collection_action.clone(),
                        resource: SyncResource::Collection {
                            name: cp.mapping.name().to_string(),
                        },
                        error: "Invalid input: conflict between storages is senseless".into(),
                    });
                }
                Action::DeleteInA => {
                    delete_collection_in_a = true;
                }
                Action::DeleteInB => {
                    delete_collection_in_b = true;
                }
            }

            for (uid, action) in &cp.item_actions {
                // FIXME: I need to somehow move these two calls outside of the "for" loop.
                let state_a = final_state
                    .state_a
                    .find_collection_state_mut(cp.mapping.name_a());
                let state_b = final_state
                    .state_b
                    .find_collection_state_mut(cp.mapping.name_b());

                if let Err(err) = action
                    .execute_on_item(uid, *storage_a, *storage_b, state_a, state_b)
                    .await
                {
                    final_state.errors.push(SynchronizationError {
                        action: action.clone(),
                        resource: SyncResource::Item {
                            uid: uid.to_string(),
                        },
                        error: err,
                    });
                };
            }
            if delete_collection_in_a {
                delete_collection(
                    *storage_a,
                    cp.mapping.name_a(),
                    &mut final_state.state_a,
                    &mut final_state.errors,
                    &cp.collection_action,
                )
                .await;
            }
            if delete_collection_in_b {
                delete_collection(
                    *storage_b,
                    cp.mapping.name_b(),
                    &mut final_state.state_b,
                    &mut final_state.errors,
                    &cp.collection_action,
                )
                .await;
            }
        }

        final_state
    }
}

async fn create_collection<I: Item>(
    storage: &mut dyn Storage<I>,
    name: &str,
    state: &mut StorageState,
    errors: &mut Vec<SynchronizationError>,
    action: &Action,
) {
    match storage.create_collection(name).await {
        Ok(c) => {
            state.add_collection(name.to_string(), c.href().to_string());
        }
        Err(e) => {
            errors.push(SynchronizationError {
                action: action.clone(),
                resource: SyncResource::Collection {
                    name: name.to_string(),
                },
                error: Box::new(e),
            });
        }
    };
}

async fn delete_collection<I: Item>(
    storage: &mut dyn Storage<I>,
    name: &str,
    state: &mut StorageState,
    errors: &mut Vec<SynchronizationError>,
    action: &Action,
) {
    match storage.destroy_collection(name).await {
        Ok(()) => {
            state.remove_collection(name);
        }
        Err(e) => {
            errors.push(SynchronizationError {
                action: action.clone(),
                resource: SyncResource::Collection {
                    name: name.to_string(),
                },
                error: Box::new(e),
            });
        }
    };
}

/// The state of a storage pair after synchronisation.
///
/// Storages may have been mutated before an error occurred, so the final state for both is always
/// returned, even in case of an error.
#[must_use]
pub struct FinalState {
    /// The state of `storage_a` after executing a plan.
    pub state_a: StorageState,
    /// The state of `storage_b` after executing a plan.
    pub state_b: StorageState,
    /// Any errors that may have occurred during synchronisation.
    pub errors: Vec<SynchronizationError>,
}

impl FinalState {
    /// Returns true if both storages are in sync.
    #[must_use]
    pub fn synchronised_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// A set of actions required to sync a collection between two storages.
#[derive(Debug)]
pub(crate) struct CollectionPlan {
    mapping: CollectionMapping,
    collection_action: Action,
    item_actions: HashMap<String, Action>,
}

impl CollectionPlan {
    /// Calculate actions to sync a collection between two storages.
    ///
    /// If a previous state is `None` it means that the collection did not previously exist.
    /// If a current state is None it means that the collection does not exist.
    #[must_use]
    fn new<'a>(
        mapping: CollectionMapping,
        previous_state_a: Option<&'a CollectionState>,
        current_state_a: Option<&'a CollectionState>,
        previous_state_b: Option<&'a CollectionState>,
        current_state_b: Option<&'a CollectionState>,
    ) -> CollectionPlan {
        // TODO: this method is very inefficient and needs to be improved.
        //       this is deliberately left for a later date when we already have a
        //       working system which we can properly benchmark.

        let mut all_items = Vec::new();
        if let Some(s) = current_state_a {
            all_items.extend(&s.items);
        }
        if let Some(s) = current_state_b {
            all_items.extend(&s.items);
        }
        if let Some(s) = previous_state_a {
            all_items.extend(&s.items);
        }
        if let Some(s) = previous_state_b {
            all_items.extend(&s.items);
        }

        let item_actions = all_items
            .iter()
            .map(|i| &i.uid)
            .unique()
            .map(|uid| {
                let a_changed = Change::for_item(current_state_a, previous_state_a, uid);
                let b_changed = Change::for_item(current_state_b, previous_state_b, uid);

                let action = Action::from_changes(a_changed, b_changed);
                trace!("For item {uid}, changes: {a_changed:?}, {b_changed:?}, action: {action:?}");
                (uid.clone(), action)
            })
            .collect();

        let collection_action = Action::from_changes(
            Change::for_collection(current_state_a, previous_state_a),
            Change::for_collection(current_state_b, previous_state_b),
        );

        CollectionPlan {
            mapping,
            collection_action,
            item_actions,
        }
    }
}
