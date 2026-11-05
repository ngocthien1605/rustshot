//! Pin to screen: a screenshot shown in a borderless, always-on-top window, to keep it in view
//! while working in another window. [`model`] is the pure, unit-tested geometry; [`win`] is the
//! Win32 window.

pub mod model;
mod win;

pub use win::open;
