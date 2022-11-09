//! This crate is part of the `vdirsyncer` project, and implements a common API for
//! reading different underlying storages which can contain `icalendar` or `vcard`
//! entries.

#![feature(io_error_more)]
#![feature(iterator_try_collect)]
// XXX: Hopefully this'll be stabilised before our first stable release
#![feature(async_fn_in_trait)]

pub mod base;
pub mod filesystem;
pub mod webcal;
