//! Exports of `models`: `createProvider` and `createModels` over Rust `ProviderStreams`, and the handles the facade
//! classes own.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::auth::types::AuthType;
use pi_ai::{
	CreateModelsOptions, CreateProviderOptions, ModelsApiStreamOptions, ModelsSimpleStreamOptions, MutableModels,
	Provider, ProviderApi, ProviderModel, ProviderStreams,
};
use pi_host::PiResult;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::events::EventSink;
use crate::options::{arg, provider_stream_options, simple_stream_options, throw};

/// A Rust API implementation (`ProviderStreams`), the value of `openAICompletionsApi()` and the other `*Api()`
/// factories. The facade wraps it in a JS `ProviderStreams` object that keeps it, so `createProvider` hands the Rust
/// implementation back to Rust without a JS hop.
#[wasm_bindgen]
pub struct NativeStreams {
	inner: Rc<dyn ProviderStreams>,
	is_native: bool,
}

impl NativeStreams {
	pub fn new(inner: Rc<dyn ProviderStreams>) -> Self {
		Self { inner, is_native: true }
	}
}

#[wasm_bindgen]
impl NativeStreams {
	/// Another handle to the same implementation, for passing by value.
	pub fn share(&self) -> NativeStreams {
		NativeStreams::new(self.inner.clone())
	}

	/// A placeholder for an `api` map entry that is a JS implementation.
	pub fn placeholder() -> NativeStreams {
		NativeStreams {
			inner: Rc::new(JsStreams {
				object: JsValue::UNDEFINED,
			}),
			is_native: false,
		}
	}

	/// `streams.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = provider_stream_options(options, &model);
		crate::events::send(throw(self.inner.stream(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `streams.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = simple_stream_options(options, &model);
		crate::events::send(throw(self.inner.stream_simple(&parsed, &context, options))?, sink);
		Ok(())
	}
}

/// A `ProviderStreams` written in JS (`{ stream, streamSimple }`), called with the model, the transcript and the
/// options as JS values; its JS event stream feeds a Rust one (`events::from_js`).
struct JsStreams {
	object: JsValue,
}

impl JsStreams {
	fn call(
		&self,
		method: &str,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: PiResult<Option<(JsValue, Option<crate::abort::JsSignal>)>>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let function: Function = Reflect::get(&self.object, &method.into())
			.map_err(crate::error::from_js)?
			.dyn_into()
			.map_err(|_| pi_host::PiError::type_error(format!("{method} is not a function")))?;
		let (options, signal) = options?.unwrap_or((JsValue::UNDEFINED, None));
		let args = js_sys::Array::of3(&crate::json::to_js(model)?, &crate::json::to_js(context)?, &options);
		let source = function.apply(&self.object, &args).map_err(crate::error::from_js)?;
		Ok(crate::events::from_js(source, signal))
	}
}

impl ProviderStreams for JsStreams {
	fn stream(
		&self,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: Option<pi_ai::ProviderStreamOptions>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let options = options
			.map(|options| crate::options::stream_options_to_js(&options.base, &options.extra))
			.transpose();
		self.call("stream", model, context, options)
	}

	fn stream_simple(
		&self,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: Option<pi_ai::SimpleStreamOptions>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let options = options
			.map(|options| crate::options::simple_options_to_js(&options))
			.transpose();
		self.call("streamSimple", model, context, options)
	}
}

/// The chat implementation of `input.api` (or one entry of an `api` map): a facade `*Api()` value carries its
/// `NativeStreams` and is used directly; any other object is a JS implementation.
fn provider_streams(native: Option<NativeStreams>, object: JsValue) -> Rc<dyn ProviderStreams> {
	match native {
		Some(native) => native.inner,
		None => Rc::new(JsStreams { object }),
	}
}

/// `createProvider(input)`. The facade passes the plain fields and `auth` in `input` and the chat implementation
/// separately: `single` (with its `NativeStreams` when it is a facade value) for one implementation, or `apis` with the
/// matching `objects` and `natives` for a map keyed by `model.api`. `natives` has an entry for each of `apis`;
/// `share()` of a facade value, or a placeholder the facade marks with `isNative: false`.
#[wasm_bindgen(js_name = aiCreateProvider)]
pub fn create_provider(
	input: JsValue,
	single: JsValue,
	single_native: Option<NativeStreams>,
	apis: Vec<String>,
	objects: Vec<JsValue>,
	natives: Vec<NativeStreams>,
) -> Result<ProviderHandle, JsValue> {
	let field = |name: &str| Reflect::get(&input, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let api = if single.is_object() {
		Some(ProviderApi::Single(provider_streams(single_native, single)))
	} else if apis.is_empty() {
		None
	} else {
		let entries = apis.into_iter().zip(objects).zip(natives);
		Some(ProviderApi::ByApi(
			entries
				.map(|((api, object), native)| {
					let native = native.is_native.then_some(native);
					(api, provider_streams(native, object))
				})
				.collect(),
		))
	};
	let provider = throw(pi_ai::create_provider(CreateProviderOptions {
		id: arg(field("id"))?,
		name: optional(field("name"))?,
		base_url: optional(field("baseUrl"))?,
		headers: optional(field("headers"))?,
		auth: super::auth::provider_auth(&field("auth")),
		models: optional::<Vec<ProviderModel>>(field("models"))?.unwrap_or_default(),
		fetch_models: None,
		filter_models: None,
		filter_all_models: None,
		api,
		images: None,
		classifiers: None,
	}))?;
	Ok(ProviderHandle {
		inner: Rc::new(provider),
	})
}

/// An optional argument field: `undefined` is `None`.
fn optional<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<Option<T>, JsValue> {
	if value.is_undefined() {
		Ok(None)
	} else {
		arg(value).map(Some)
	}
}

/// A Rust `Provider`, owned by the facade's `Provider` object.
#[wasm_bindgen]
pub struct ProviderHandle {
	inner: Rc<Provider>,
}

#[wasm_bindgen]
impl ProviderHandle {
	/// Another handle to the same provider, for passing by value.
	pub fn share(&self) -> ProviderHandle {
		ProviderHandle {
			inner: self.inner.clone(),
		}
	}

	/// The plain fields: `{ id, name, baseUrl?, headers? }`.
	pub fn fields(&self) -> Result<JsValue, JsValue> {
		let object = Object::new();
		let _ = Reflect::set(&object, &"id".into(), &self.inner.id.as_str().into());
		let _ = Reflect::set(&object, &"name".into(), &self.inner.name.as_str().into());
		if let Some(base_url) = &self.inner.base_url {
			let _ = Reflect::set(&object, &"baseUrl".into(), &base_url.as_str().into());
		}
		if let Some(headers) = &self.inner.headers {
			let _ = Reflect::set(&object, &"headers".into(), &throw(crate::json::to_js(headers))?);
		}
		Ok(object.into())
	}

	/// `provider.getModels()`.
	#[wasm_bindgen(js_name = getModels)]
	pub fn get_models(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&(self.inner.get_models)()))
	}

	/// `provider.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = provider_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream)(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `provider.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = simple_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream_simple)(&parsed, &context, options))?, sink);
		Ok(())
	}
}

/// `createModels(options)`. `credentials`, `modelsStore` and `authContext` are JS objects whose methods the Rust
/// collection calls.
#[wasm_bindgen(js_name = aiCreateModels)]
pub fn create_models(options: JsValue) -> Result<ModelsHandle, JsValue> {
	let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let present = |name: &str| Some(field(name)).filter(JsValue::is_object);
	Ok(ModelsHandle {
		inner: pi_ai::create_models(Some(CreateModelsOptions {
			identity: optional(field("identity"))?,
			credentials: present("credentials").map(|store| super::auth::credential_store_from_js(&store)),
			models_store: present("modelsStore").map(|store| super::auth::models_store_from_js(&store)),
			auth_context: present("authContext").map(|context| super::auth::auth_context_from_js(&context)),
		})),
	})
}

/// A Rust `MutableModels`, owned by the facade's `Models` object.
#[wasm_bindgen]
pub struct ModelsHandle {
	inner: Rc<dyn MutableModels>,
}

#[wasm_bindgen]
impl ModelsHandle {
	/// `models.setProvider(provider)`.
	#[wasm_bindgen(js_name = setProvider)]
	pub fn set_provider(&self, provider: ProviderHandle) {
		self.inner.set_provider(provider.inner);
	}

	/// `models.deleteProvider(id)`.
	#[wasm_bindgen(js_name = deleteProvider)]
	pub fn delete_provider(&self, id: &str) {
		self.inner.delete_provider(id);
	}

	/// `models.clearProviders()`.
	#[wasm_bindgen(js_name = clearProviders)]
	pub fn clear_providers(&self) {
		self.inner.clear_providers();
	}

	/// `models.getModels(provider)`.
	#[wasm_bindgen(js_name = getModels)]
	pub fn get_models(&self, provider: Option<String>) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.inner.get_models(provider.as_deref())))
	}

	/// `models.getModel(provider, id)`.
	#[wasm_bindgen(js_name = getModel)]
	pub fn get_model(&self, provider: &str, id: &str) -> Result<JsValue, JsValue> {
		match self.inner.get_model(provider, id) {
			Some(model) => throw(crate::json::to_js(&model)),
			None => Ok(JsValue::UNDEFINED),
		}
	}

	/// `models.login(providerId, type, interaction, options)`.
	pub fn login(
		&self,
		provider_id: &str,
		r#type: JsValue,
		interaction: JsValue,
		options: JsValue,
	) -> Result<Promise, JsValue> {
		let r#type: AuthType = arg(r#type)?;
		let login = self.inner.login(
			provider_id,
			r#type,
			super::auth::interaction_from_js(&interaction),
			super::auth::login_options_from_js(&options),
		);
		Ok(super::auth::promise(async move { crate::json::to_js(&login.await?) }))
	}

	/// `models.logout(providerId, options)`.
	pub fn logout(&self, provider_id: &str, options: JsValue) -> Promise {
		let logout = self
			.inner
			.logout(provider_id, super::auth::auth_operation_options(&options));
		super::auth::promise(async move {
			logout.await?;
			Ok(JsValue::UNDEFINED)
		})
	}

	/// `models.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context: pi_ai::Context = arg(context)?;
		let options = provider_stream_options(options, &model).map(ModelsApiStreamOptions::from);
		crate::events::send(self.inner.stream(&parsed, &context, options), sink);
		Ok(())
	}

	/// `models.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context: pi_ai::Context = arg(context)?;
		let options = simple_stream_options(options, &model).map(ModelsSimpleStreamOptions::from);
		crate::events::send(self.inner.stream_simple(&parsed, &context, options), sink);
		Ok(())
	}
}
