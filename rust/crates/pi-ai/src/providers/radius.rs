//! Port of `packages/ai/src/providers/radius.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `radiusProvider(options = {})` takes `Option<RadiusProviderOptions>`.
//! - Restoring `context.stored` keeps only its chat models (the TS casts every model of the provider to `Model`).

use std::cell::RefCell;
use std::rc::Rc;

use crate::api::pi_messages_lazy::pi_messages_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_radius_oauth;
use crate::auth::types::{Credential, ProviderAuth};
use crate::models::{ModelsPublication, Provider, RefreshModelsContext};
use crate::models_store::ModelsStoreEntry;
use crate::providers::radius_config::{
	DEFAULT_RADIUS_GATEWAY, get_radius_models, get_radius_models_from_config, load_radius_gateway_config,
	normalize_radius_gateway_url,
};
use crate::providers::radius_models::RADIUS_MODELS;
use crate::types::{AnyModel, Model};

#[derive(Clone, Debug, Default)]
pub struct RadiusProviderOptions {
	pub id: Option<String>,
	pub name: Option<String>,
	pub gateway: Option<String>,
}

/// Publication of a new gateway catalog: optionally persisted, then swapped into the provider's dynamic list.
fn publication(
	dynamic_models: &Rc<RefCell<Option<Vec<Model>>>>,
	models: Vec<Model>,
	persist: bool,
) -> ModelsPublication {
	let target = dynamic_models.clone();
	let persist = persist.then(|| {
		Some(ModelsStoreEntry {
			models: models.iter().cloned().map(AnyModel::Chat).collect(),
			checked_at: Some(pi_host::now_ms()),
			..ModelsStoreEntry::default()
		})
	});
	ModelsPublication {
		persist,
		update: Some(Rc::new(move || {
			*target.borrow_mut() = Some(models.clone());
		})),
	}
}

/// Radius gateway provider with a persisted, dynamically refreshed catalog.
pub fn radius_provider(options: Option<RadiusProviderOptions>) -> Provider {
	let options = options.unwrap_or_default();
	let id = options.id.unwrap_or_else(|| "radius".into());
	let name = options.name.unwrap_or_else(|| "Radius".into());
	let gateway = normalize_radius_gateway_url(options.gateway.as_deref().unwrap_or(DEFAULT_RADIUS_GATEWAY));
	let baseline_models: Vec<Model> = if gateway == normalize_radius_gateway_url(DEFAULT_RADIUS_GATEWAY) {
		RADIUS_MODELS
			.values()
			.map(|model| {
				let mut model = model.clone();
				model.provider = id.as_str().into();
				model
			})
			.collect()
	} else {
		Vec::new()
	};
	// Gateway catalog for this account. Radius org owners can disable models, so once known it replaces
	// the shipped baseline instead of overlaying it. The baseline only covers the time before any catalog exists.
	let dynamic_models: Rc<RefCell<Option<Vec<Model>>>> = Rc::new(RefCell::new(None));
	let streams = pi_messages_api();

	let oauth_load = {
		let (name, gateway) = (name.clone(), gateway.clone());
		Rc::new(move || Box::pin(load_radius_oauth(name.clone(), gateway.clone())) as _)
	};
	let get_models = {
		let dynamic_models = dynamic_models.clone();
		Rc::new(move || {
			dynamic_models
				.borrow()
				.clone()
				.unwrap_or_else(|| baseline_models.clone())
		})
	};
	let refresh_models = {
		let (id, gateway) = (id.clone(), gateway.clone());
		Rc::new(move |context: RefreshModelsContext| {
			let (id, gateway, dynamic_models) = (id.clone(), gateway.clone(), dynamic_models.clone());
			Box::pin(async move {
				let stored = context.stored.clone();
				if let Some(stored) = &stored {
					let restored: Vec<Model> = stored
						.models
						.iter()
						.filter_map(|model| match model {
							AnyModel::Chat(model) if *model.provider == *id => Some(model.clone()),
							_ => None,
						})
						.collect();
					if !(context.publish)(publication(&dynamic_models, restored, false)).await? {
						return Ok(());
					}
				}

				// Import catalogs cached by the pre-ModelsStore Radius implementation.
				if stored.is_none()
					&& let Some(Credential::OAuth(credential)) = &context.credential
				{
					let legacy = get_radius_models(&id, Some(credential));
					if !legacy.is_empty() && !(context.publish)(publication(&dynamic_models, legacy, true)).await? {
						return Ok(());
					}
				}

				if !context.allow_network || context.signal.aborted() {
					return Ok(());
				}
				let api_key = match &context.credential {
					Some(Credential::OAuth(credential)) => Some(credential.access.clone()),
					Some(Credential::ApiKey(credential)) => credential.key.clone(),
					None => None,
				};
				let config =
					load_radius_gateway_config(&gateway, api_key.as_deref(), Some(context.signal.clone())).await?;
				if context.signal.aborted() {
					return Ok(());
				}
				let refreshed = get_radius_models_from_config(&id, &config);
				(context.publish)(publication(&dynamic_models, refreshed, true)).await?;
				Ok(())
			}) as _
		})
	};
	let stream = {
		let streams = streams.clone();
		Rc::new(move |model: &Model, context: &_, options| streams.stream(model, context, options))
	};
	let stream_simple =
		Rc::new(move |model: &Model, context: &_, options| streams.stream_simple(model, context, options));

	Provider {
		id,
		name: name.clone(),
		base_url: None,
		headers: None,
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Radius API key", &["RADIUS_API_KEY"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name,
				is_subscription: None,
				login_label: None,
				load: oauth_load,
			})),
		},
		get_models,
		get_all_models: None,
		refresh_models: Some(refresh_models),
		filter_models: None,
		filter_all_models: None,
		stream,
		stream_simple,
		fetch_deferred: None,
		cancel_deferred: None,
		generate_images: None,
		classify: None,
	}
}
