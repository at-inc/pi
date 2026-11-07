//! Port of `packages/ai/src/api/azure-openai-config.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-openai-responses
use std::ops::{Deref, DerefMut};

use pi_host::PiResult;

use crate::types::{Model, StreamOptions};

/// Azure models ship without a baseUrl: one resource per user, resolved per request.
#[derive(Clone, Default)]
pub struct AzureEndpointOptions {
	pub base: StreamOptions,
	pub azure_api_version: Option<String>,
	pub azure_resource_name: Option<String>,
	pub azure_base_url: Option<String>,
	pub azure_deployment_name: Option<String>,
}

impl Deref for AzureEndpointOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for AzureEndpointOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

pub fn resolve_deployment_name(_model: &Model, _options: Option<&AzureEndpointOptions>) -> String {
	todo!("PORT-STUB")
}

pub fn resolve_azure_base_url(_model: &Model, _options: Option<&AzureEndpointOptions>) -> PiResult<String> {
	todo!("PORT-STUB")
}
