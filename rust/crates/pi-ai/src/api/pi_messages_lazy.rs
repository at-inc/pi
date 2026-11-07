//! Port of `packages/ai/src/api/pi-messages.lazy.ts`.
//! PORT-STATUS: ported

use std::rc::Rc;

use super::pi_messages::pi_messages_streams;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `piMessagesApi()`: streams through the `pi-messages` module.
pub fn pi_messages_api() -> Rc<dyn ProviderStreams> {
	lazy_api(Rc::new(|| Box::pin(async { Ok(pi_messages_streams()) })), None)
}
