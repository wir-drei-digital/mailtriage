pub mod categories;
pub mod config;
pub mod domain;
pub mod engine;
pub mod filing;
/// Compatibility path for `mailtriage::himalaya::Himalaya`.
pub mod himalaya {
    pub use crate::engine::himalaya::*;
}
pub mod normalize;
pub mod policy;
pub mod process;
pub mod prompt;
pub mod provider;
pub mod secrets;
pub mod service;
pub mod setup;
pub mod store;
pub mod system_service;
