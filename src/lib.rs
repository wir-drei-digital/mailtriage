pub mod config;
pub mod domain;
pub mod engine;
/// Compatibility path for `mailtriage::himalaya::Himalaya`.
pub mod himalaya {
    pub use crate::engine::himalaya::*;
}
pub mod normalize;
pub mod policy;
pub mod provider;
pub mod service;
pub mod store;
