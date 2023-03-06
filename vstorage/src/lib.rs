//! This crate is part of the `vdirsyncer` project, and implements a common API for
//! reading different underlying storages which can contain `icalendar` or `vcard`
//! entries.

#![feature(io_error_more)]

pub mod base;
pub mod caldav;
pub mod filesystem;
pub mod readonly;
mod simple_component;
pub mod util;
pub mod webcal;
