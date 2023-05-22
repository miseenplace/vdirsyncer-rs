//! Components used for synchronising storages.
//!
//! [`StoragePair`] is the main entry point for this module.
mod pair;
pub mod plan;

pub use pair::StoragePair;
pub use pair::StorageState;
