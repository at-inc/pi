//! Providers across the boundary, for `models`: a JS `Provider` object as a Rust `Provider` (`js_provider`), JS image
//! and classifier implementations as Rust ones, a JS `AuthContext` as a Rust one, and a Rust `RefreshModelsContext`
//! as the JS one a JS `refreshModels` receives (and back, for a Rust `refreshModels` called from JS).

use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use pi_ai::auth::types::{AuthContext, Credential};
use pi_ai::{
	AnyModel, AssistantImages, CancelDeferredFunction, ClassifierContext, ClassifierFunction, ClassifierModel,
	ClassifierOptions, ClassifierResult, FetchDeferredFunction, FetchModelsFunction, FilterAllModelsFunction,
	FilterModelsFunction, GetAllModelsFunction, ImageModel, ImagesContext, ImagesFunction, ImagesOptions, Model,
	ModelsPublication, ModelsStoreEntry, Provider, ProviderClassifier, ProviderImages, ProviderImagesOptions,
	ProviderStreams, RefreshModelsContext, RefreshModelsFunction,
};
use pi_host::{AbortController, LocalBoxFuture, PiError, PiResult};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Map;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::models::JsStreams;
use crate::options::{JsOptions, arg, js_callback, request_options_to_js};

/// The Rust `Provider` for a JS `Provider` object. Its plain fields are read once; the optional members exist when the
/// object has them. A throwing `getModels`/`getAllModels` yields no models, as in `Models`; a throwing
/// `filterModels`/`filterAllModels` keeps the models unfiltered, since the Rust member cannot fail.
pub fn js_provider(object: JsValue) -> Result<Provider, JsValue> {
	let id: String = arg(get(&object, "id"))?;
	let name = get(&object, "name").as_string().unwrap_or_else(|| id.clone());
	let headers = get(&object, "headers");
	let streams = Rc::new(JsStreams::new(object.clone()));
	let simple = streams.clone();
	Ok(Provider {
		base_url: get(&object, "baseUrl").as_string(),
		headers: if headers.is_undefined() {
			None
		} else {
			Some(arg(headers)?)
		},
		auth: super::models::provider_auth(&get(&object, "auth")),
		get_models: Rc::new(reader(object.clone(), "getModels")),
		get_all_models: method(&object, "getAllModels")
			.map(|_| Rc::new(reader(object.clone(), "getAllModels")) as GetAllModelsFunction),
		refresh_models: method(&object, "refreshModels").map(|_| refresh_models(object.clone())),
		filter_models: method(&object, "filterModels").map(|filter| filter_models(filter, object.clone())),
		filter_all_models: method(&object, "filterAllModels").map(|filter| filter_all_models(filter, object.clone())),
		stream: Rc::new(move |model, context, options| streams.stream(model, context, options)),
		stream_simple: Rc::new(move |model, context, options| simple.stream_simple(model, context, options)),
		fetch_deferred: method(&object, "fetchDeferred").map(|_| fetch_deferred(object.clone())),
		cancel_deferred: method(&object, "cancelDeferred").map(|_| cancel_deferred(object.clone())),
		generate_images: method(&object, "generateImages").map(|_| {
			let images = JsImages { object: object.clone() };
			Rc::new(
				move |model: &ImageModel, context: &ImagesContext, options: Option<ImagesOptions>| {
					let options = options.map(|base| ProviderImagesOptions {
						base,
						extra: Map::new(),
					});
					images.generate_images(model, context, options)
				},
			) as ImagesFunction
		}),
		classify: method(&object, "classify").map(|_| {
			let classifier = JsClassifier { object: object.clone() };
			Rc::new(move |model: &ClassifierModel, context: &ClassifierContext, options| {
				classifier.classify(model, context, options)
			}) as ClassifierFunction
		}),
		id,
		name,
	})
}

/// A synchronous JS model list member; a throw or an unreadable result yields no models.
fn reader<T: DeserializeOwned + Default>(object: JsValue, key: &'static str) -> impl Fn() -> T {
	move || {
		call_sync(&object, key, &[])
			.and_then(crate::json::from_js)
			.unwrap_or_default()
	}
}

/// A JS `filterModels(models, credential)` (of a provider or of `createProvider`'s input, the `this` it is called on).
pub fn filter_models(filter: Function, this: JsValue) -> FilterModelsFunction {
	Rc::new(move |models: &[Model], credential: Option<&Credential>| {
		filter_with(&filter, &this, models, credential).unwrap_or_else(|_| models.to_vec())
	})
}

/// A JS `filterAllModels(models, credential)`, like `filter_models`.
pub fn filter_all_models(filter: Function, this: JsValue) -> FilterAllModelsFunction {
	Rc::new(move |models: &[AnyModel], credential: Option<&Credential>| {
		filter_with(&filter, &this, models, credential).unwrap_or_else(|_| models.to_vec())
	})
}

fn filter_with<T: Serialize + DeserializeOwned>(
	filter: &Function,
	this: &JsValue,
	models: &[T],
	credential: Option<&Credential>,
) -> PiResult<Vec<T>> {
	let credential = credential.map_or(Ok(JsValue::UNDEFINED), crate::json::to_js)?;
	let result = filter
		.call2(this, &crate::json::to_js(models)?, &credential)
		.map_err(crate::error::from_js)?;
	crate::json::from_js(result)
}

/// `createProvider`'s JS `fetchModels(context)`, with the Rust context as a JS `RefreshModelsContext`.
pub fn fetch_models(fetch: Function, this: JsValue) -> FetchModelsFunction {
	Rc::new(move |context| {
		let (fetch, this) = (fetch.clone(), this.clone());
		Box::pin(async move {
			let (js, _signal) = refresh_context_to_js(&context)?;
			crate::json::from_js(crate::callback::call(&fetch, &this, &[js]).await?)
		})
	})
}

/// `refreshModels(context)` of a JS provider, with the Rust context as a JS `RefreshModelsContext`.
fn refresh_models(object: JsValue) -> RefreshModelsFunction {
	Rc::new(move |context| {
		let object = object.clone();
		Box::pin(async move {
			let (js, _signal) = refresh_context_to_js(&context)?;
			call(&object, "refreshModels", &[js]).await?;
			Ok(())
		})
	})
}

/// The JS `RefreshModelsContext` of a Rust one; the returned guard keeps `signal` following the Rust signal.
fn refresh_context_to_js(context: &RefreshModelsContext) -> PiResult<(JsValue, crate::abort::JsSignal)> {
	let object = Object::new();
	let set = |key: &str, value: &JsValue| {
		let _ = Reflect::set(&object, &key.into(), value);
	};
	if let Some(credential) = &context.credential {
		set("credential", &crate::json::to_js(credential)?);
	}
	if let Some(stored) = &context.stored {
		set("stored", &crate::json::to_js(stored)?);
	}
	let publish = context.publish.clone();
	set(
		"publish",
		&js_callback(move |args| {
			let publish = publish.clone();
			let publication = publication_from_js(&args.get(0));
			async move { Ok(JsValue::from(publish(publication?).await?)) }
		}),
	);
	set("allowNetwork", &JsValue::from(context.allow_network));
	if let Some(force) = context.force {
		set("force", &JsValue::from(force));
	}
	let signal = crate::abort::to_js(&context.signal);
	set("signal", &signal.js);
	if let Some(base_url) = &context.base_url {
		set("baseUrl", &JsValue::from(base_url.as_str()));
	}
	Ok((object.into(), signal))
}

/// The Rust `ModelsPublication` of a JS one: `persist` absent, `null` or an entry, and `update` a JS function. A
/// throwing `update` is ignored, as the Rust update cannot fail.
fn publication_from_js(publication: &JsValue) -> PiResult<ModelsPublication> {
	let persist = get(publication, "persist");
	let persist = if persist.is_undefined() {
		None
	} else if persist.is_null() {
		Some(None)
	} else {
		Some(Some(crate::json::from_js::<ModelsStoreEntry>(persist)?))
	};
	let update = method(publication, "update").map(|update| {
		let this = publication.clone();
		Rc::new(move || {
			let _ = update.call0(&this);
		}) as Rc<dyn Fn()>
	});
	Ok(ModelsPublication { persist, update })
}

/// The Rust `RefreshModelsContext` of a JS one, for calling a Rust provider's `refreshModels` from JS. A context
/// without `signal` gets one that never aborts, as `Models` always passes one.
pub fn refresh_context_from_js(context: &JsValue) -> PiResult<RefreshModelsContext> {
	let optional = |key: &str| {
		let value = get(context, key);
		(!value.is_undefined() && !value.is_null()).then_some(value)
	};
	let publish = method(context, "publish").ok_or_else(|| PiError::type_error("context.publish is not a function"))?;
	let this = context.clone();
	Ok(RefreshModelsContext {
		credential: optional("credential").map(crate::json::from_js).transpose()?,
		stored: optional("stored").map(crate::json::from_js).transpose()?,
		publish: Rc::new(move |publication| {
			let (publish, this) = (publish.clone(), this.clone());
			Box::pin(async move {
				let object = Object::new();
				match publication.persist {
					Some(Some(entry)) => {
						let _ = Reflect::set(&object, &"persist".into(), &crate::json::to_js(&entry)?);
					}
					Some(None) => {
						let _ = Reflect::set(&object, &"persist".into(), &JsValue::NULL);
					}
					None => {}
				}
				if let Some(update) = publication.update {
					let update = Closure::<dyn Fn()>::new(move || update()).into_js_value();
					let _ = Reflect::set(&object, &"update".into(), &update);
				}
				let published = crate::callback::call(&publish, &this, &[object.into()]).await?;
				Ok(published.is_truthy())
			})
		}),
		allow_network: get(context, "allowNetwork").is_truthy(),
		force: get(context, "force").as_bool(),
		signal: crate::abort::from_js(&get(context, "signal")).unwrap_or_else(|| AbortController::new().signal()),
		base_url: get(context, "baseUrl").as_string(),
	})
}

/// `fetchDeferred(model, handle, options)` of a JS provider or `ProviderStreams`.
pub fn fetch_deferred(object: JsValue) -> FetchDeferredFunction {
	Rc::new(move |model, handle, options| {
		let (options, signal) = match options {
			Some(options) => {
				let mut extra = Map::new();
				if let Some(wait) = options.wait {
					extra.insert("wait".into(), wait.into());
				}
				request_options_to_js(&options.base, &extra)?
			}
			None => (JsValue::UNDEFINED, None),
		};
		let args = [crate::json::to_js(model)?, crate::json::to_js(handle)?, options];
		let source = call_sync(&object, "fetchDeferred", &args)?;
		Ok(crate::events::from_js(source, signal))
	})
}

/// `cancelDeferred(model, handle, options)` of a JS provider or `ProviderStreams`.
pub fn cancel_deferred(object: JsValue) -> CancelDeferredFunction {
	Rc::new(move |model, handle, options| {
		let object = object.clone();
		let args = (|| {
			let (options, signal) = match options {
				Some(options) => request_options_to_js(&options, &Map::new())?,
				None => (JsValue::UNDEFINED, None),
			};
			Ok::<_, PiError>((
				[crate::json::to_js(model)?, crate::json::to_js(handle)?, options],
				signal,
			))
		})();
		Box::pin(async move {
			let (args, _signal) = args?;
			call(&object, "cancelDeferred", &args).await?;
			Ok(())
		})
	})
}

/// A `ProviderImages` written in JS (`{ generateImages }`).
pub struct JsImages {
	pub object: JsValue,
}

impl ProviderImages for JsImages {
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		let object = self.object.clone();
		let args = (|| {
			let (options, signal) = match options {
				Some(options) => {
					let mut extra = options.extra.clone();
					if let Some(metadata) = &options.base.metadata {
						extra.insert("metadata".into(), metadata.clone().into());
					}
					request_options_to_js(&options.base.base, &extra)?
				}
				None => (JsValue::UNDEFINED, None),
			};
			Ok::<_, PiError>((
				[crate::json::to_js(model)?, crate::json::to_js(context)?, options],
				signal,
			))
		})();
		Box::pin(async move {
			let (args, _signal) = args?;
			crate::json::from_js(call(&object, "generateImages", &args).await?)
		})
	}
}

/// A `ProviderClassifier` written in JS (`{ classify }`).
pub struct JsClassifier {
	pub object: JsValue,
}

impl ProviderClassifier for JsClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		let object = self.object.clone();
		let args = (|| {
			let (options, signal) = match options {
				Some(options) => {
					let mut extra = Map::new();
					if let Some(temperature) = options.temperature {
						extra.insert("temperature".into(), temperature.into());
					}
					request_options_to_js(&options.base, &extra)?
				}
				None => (JsValue::UNDEFINED, None),
			};
			Ok::<_, PiError>((
				[crate::json::to_js(model)?, crate::json::to_js(context)?, options],
				signal,
			))
		})();
		Box::pin(async move {
			let (args, _signal) = args?;
			crate::json::from_js(call(&object, "classify", &args).await?)
		})
	}
}

/// A JS `AuthContext` (`{ env, fileExists }`) as a Rust one. A throwing or rejecting method reads as unset.
pub struct JsAuthContext {
	pub object: JsValue,
}

impl AuthContext for JsAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let (object, name) = (self.object.clone(), JsValue::from(name));
		Box::pin(async move { call(&object, "env", &[name]).await.ok()?.as_string() })
	}

	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool> {
		let (object, path) = (self.object.clone(), JsValue::from(path));
		Box::pin(async move {
			call(&object, "fileExists", &[path])
				.await
				.is_ok_and(|exists| exists.is_truthy())
		})
	}
}

/// `ProviderImagesOptions` of a JS options object; `None` when the caller passed none.
pub fn images_options(options: JsValue, model: &JsValue) -> Option<ProviderImagesOptions> {
	let options = JsOptions::new(options);
	options.is_given().then(|| options.images_options(model))
}

/// `ClassifierOptions` of a JS options object; `None` when the caller passed none.
pub fn classifier_options(options: JsValue, model: &JsValue) -> Option<ClassifierOptions> {
	let options = JsOptions::new(options);
	options.is_given().then(|| options.classifier_options(model))
}

/// `await object[key](...args)`.
async fn call(object: &JsValue, key: &str, args: &[JsValue]) -> PiResult<JsValue> {
	let function = method(object, key).ok_or_else(|| PiError::type_error(format!("{key} is not a function")))?;
	crate::callback::call(&function, object, args).await
}

/// `object[key](...args)` for a synchronous member.
fn call_sync(object: &JsValue, key: &str, args: &[JsValue]) -> PiResult<JsValue> {
	let function = method(object, key).ok_or_else(|| PiError::type_error(format!("{key} is not a function")))?;
	let args: Array = args.iter().collect();
	function.apply(object, &args).map_err(crate::error::from_js)
}

fn method(object: &JsValue, key: &str) -> Option<Function> {
	get(object, key).dyn_into::<Function>().ok()
}

fn get(target: &JsValue, key: &str) -> JsValue {
	Reflect::get(target, &key.into()).unwrap_or(JsValue::UNDEFINED)
}
