//! Exports of `models`: `createProvider` and `createModels` over Rust `ProviderStreams`, and the handles the facade
//! classes own.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::auth::types::{ApiKeyAuth, ApiKeyAuthInput, AuthContext, ProviderAuth};
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
		auth: provider_auth(&field("auth")),
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

/// `ProviderAuth` of a JS `auth` object. `apiKey.resolve` and `apiKey.check` are bridged; `login` and `oauth` belong
/// to the auth facade and are not bridged yet.
fn provider_auth(auth: &JsValue) -> ProviderAuth {
	let api_key = Reflect::get(auth, &"apiKey".into()).unwrap_or(JsValue::UNDEFINED);
	if !api_key.is_object() {
		return ProviderAuth::default();
	}
	let method = |name: &str| Reflect::get(&api_key, &name.into()).ok()?.dyn_into::<Function>().ok();
	let name = Reflect::get(&api_key, &"name".into())
		.ok()
		.and_then(|name| name.as_string())
		.unwrap_or_default();
	let resolve = method("resolve");
	let check = method("check");
	let this = api_key.clone();
	ProviderAuth {
		api_key: Some(ApiKeyAuth {
			name,
			login: None,
			check: check.map(|check| {
				let this = this.clone();
				Rc::new(move |input: ApiKeyAuthInput| {
					let (check, this) = (check.clone(), this.clone());
					Box::pin(async move { optional_result(&check, &this, input).await }) as pi_host::LocalBoxFuture<_>
				}) as pi_ai::auth::types::ApiKeyCheckFunction
			}),
			resolve: match resolve {
				Some(resolve) => Rc::new(move |input: ApiKeyAuthInput| {
					let (resolve, this) = (resolve.clone(), this.clone());
					Box::pin(async move { optional_result(&resolve, &this, input).await })
				}),
				None => Rc::new(|_| Box::pin(async { Ok(None) })),
			},
		}),
		oauth: None,
	}
}

/// Calls an api-key auth method with `{ ctx, credential, signal }`; `undefined` is `None`.
async fn optional_result<T: serde::de::DeserializeOwned>(
	function: &Function,
	this: &JsValue,
	input: ApiKeyAuthInput,
) -> PiResult<Option<T>> {
	let object = Object::new();
	let ctx: JsValue = AuthContextHandle { inner: input.ctx }.into();
	let _ = Reflect::set(&object, &"ctx".into(), &ctx);
	if let Some(credential) = &input.credential {
		let _ = Reflect::set(&object, &"credential".into(), &crate::json::to_js(credential)?);
	}
	let signal = crate::abort::to_js(&input.signal);
	let _ = Reflect::set(&object, &"signal".into(), &signal.js);
	let result = crate::callback::call(function, this, &[object.into()]).await?;
	if result.is_undefined() || result.is_null() {
		return Ok(None);
	}
	crate::json::from_js(result).map(Some)
}

/// The `AuthContext` an api-key auth method receives as `ctx`.
#[wasm_bindgen]
pub struct AuthContextHandle {
	inner: Rc<dyn AuthContext>,
}

#[wasm_bindgen]
impl AuthContextHandle {
	/// `ctx.env(name)`.
	pub fn env(&self, name: String) -> Promise {
		let pending = self.inner.env(&name);
		wasm_bindgen_futures::future_to_promise(
			async move { Ok(pending.await.map_or(JsValue::UNDEFINED, JsValue::from)) },
		)
	}

	/// `ctx.fileExists(path)`.
	#[wasm_bindgen(js_name = fileExists)]
	pub fn file_exists(&self, path: String) -> Promise {
		let pending = self.inner.file_exists(&path);
		wasm_bindgen_futures::future_to_promise(async move { Ok(JsValue::from(pending.await)) })
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

/// `createModels(options)`. Only `identity` is bridged; `credentials`, `modelsStore` and `authContext` belong to the
/// auth facade.
#[wasm_bindgen(js_name = aiCreateModels)]
pub fn create_models(options: JsValue) -> Result<ModelsHandle, JsValue> {
	let identity = Reflect::get(&options, &"identity".into()).unwrap_or(JsValue::UNDEFINED);
	let identity = if identity.is_undefined() {
		None
	} else {
		Some(arg(identity)?)
	};
	Ok(ModelsHandle {
		inner: pi_ai::create_models(Some(CreateModelsOptions {
			identity,
			..CreateModelsOptions::default()
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
