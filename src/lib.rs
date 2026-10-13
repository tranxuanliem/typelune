//! Typelune core: matching, templates, storage, migration and opt-in system-wide expansion
//! for the native macOS app. The SwiftUI shell talks to it through `ffi` (C ABI).

pub mod engine;
pub mod ffi;
pub mod migrate;
pub mod model;
pub mod native;
pub mod saver;
pub mod store;
pub mod template;
