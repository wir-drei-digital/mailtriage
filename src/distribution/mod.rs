//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule, a private tested Himalaya, Homebrew launch
//! paths, and `self install`.
pub mod brew;
pub mod himalaya;
pub mod protected;
pub mod self_install;
