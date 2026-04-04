//! ModelRelay Web — open-source admin dashboard and monitoring UI.
//!
//! This crate provides the self-hostable admin interface for ModelRelay.
//! It includes shared HTML templates, a health endpoint, and a monitoring
//! dashboard. The commercial `modelrelay-cloud` crate depends on this and
//! adds Stripe billing, user accounts, and subscription management on top.

/// Shared HTML templates usable by both the OSS UI and the commercial cloud crate.
pub mod templates;

mod routes;

pub use routes::router;
