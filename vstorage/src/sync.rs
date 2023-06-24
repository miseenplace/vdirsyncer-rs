//! Components used for synchronising storages.
//!
//! The general gist behind synchronising is:
//!
//! - Create a [`StoragePair`] instance, which has the state saved from the previous sync and the
//!   two storages that are to be synchronised.
//! - Create a [`Plan`][plan::Plan] which contains a list of actions to be executed to sync both
//!   storages. A dry-run should be able to print the plan, although right now the only way to
//!   inspect it is via `dbg!()`.
//! - Run [`Plan::execute`][plan::Plan::execute]. This returns two opaque states that should be
//!   serialised and used as input for the next synchronisation (mostly, this helps understand when
//!   an item has change on one side vs where there is a conflict).
//!
//! The synchronization algorithm is based on [the algorithm from the original
//! vdirsyncer][original-algo].
//!
//! [original-algo]: https://unterwaditzer.net/2016/sync-algorithm.html
mod pair;
pub mod plan;

pub use pair::CollectionMapping;
pub use pair::StoragePair;
pub use pair::StorageState;
