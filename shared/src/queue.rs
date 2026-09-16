//! Compatibility module for the connector-event adapter.
//!
//! New code should import `shared::connector_event_queue`; this module remains
//! as a naming bridge for integrations that used `shared::queue`.
pub use crate::connector_event_queue::*;
