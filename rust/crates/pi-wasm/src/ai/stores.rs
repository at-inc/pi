//! Stores across the boundary, for `createModels`: a JS `CredentialStore` and a JS `ModelsStore` as Rust ones. Each
//! method call passes `{ signal }` when the Rust caller gave a signal, as the TS passes its options through.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Object, Reflect};
use pi_ai::auth::types::{AuthOperationOptions, Credential, CredentialInfo, CredentialModifier, CredentialStore};
use pi_ai::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

use crate::options::js_callback;

/// A `CredentialStore` written in JS.
pub struct JsCredentialStore {
	pub object: JsValue,
}

impl CredentialStore for JsCredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let call = call(&self.object, "read", provider_id, options.and_then(|options| options.signal));
		Box::pin(async move { optional(call.await?) })
	}

	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		let (object, signal) = (self.object.clone(), options.and_then(|options| options.signal));
		Box::pin(async move {
			let (options, _guard) = options_object(signal.as_ref());
			crate::json::from_js(invoke(&object, "list", &[options]).await?)
		})
	}

	fn modify(
		&self,
		provider_id: &str,
		r#fn: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let (object, provider_id) = (self.object.clone(), JsValue::from(provider_id));
		let signal = options.and_then(|options| options.signal);
		let modifier = Rc::new(RefCell::new(Some(r#fn)));
		let callback = js_callback(move |args| {
			let modifier = modifier.borrow_mut().take();
			let current = args.get(0);
			async move {
				let modifier = modifier.ok_or_else(|| PiError::type_error("modify fn called twice"))?;
				let current: Option<Credential> = optional(current)?;
				match modifier(current).await? {
					Some(credential) => crate::json::to_js(&credential),
					None => Ok(JsValue::UNDEFINED),
				}
			}
		});
		Box::pin(async move {
			let (options, _guard) = options_object(signal.as_ref());
			optional(invoke(&object, "modify", &[provider_id, callback, options]).await?)
		})
	}

	fn delete(&self, provider_id: &str, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<()>> {
		let call = call(&self.object, "delete", provider_id, options.and_then(|options| options.signal));
		Box::pin(async move {
			call.await?;
			Ok(())
		})
	}
}

/// A `ModelsStore` written in JS.
pub struct JsModelsStore {
	pub object: JsValue,
}

impl ModelsStore for JsModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>> {
		let call = call(&self.object, "read", provider_id, options.and_then(|options| options.signal));
		Box::pin(async move { optional(call.await?) })
	}

	fn write(
		&self,
		provider_id: &str,
		entry: ModelsStoreEntry,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let (object, provider_id) = (self.object.clone(), JsValue::from(provider_id));
		let signal = options.and_then(|options| options.signal);
		Box::pin(async move {
			let (options, _guard) = options_object(signal.as_ref());
			invoke(&object, "write", &[provider_id, crate::json::to_js(&entry)?, options]).await?;
			Ok(())
		})
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let call = call(&self.object, "delete", provider_id, options.and_then(|options| options.signal));
		Box::pin(async move {
			call.await?;
			Ok(())
		})
	}
}

/// `await object[key](providerId, options)`.
fn call(
	object: &JsValue,
	key: &'static str,
	provider_id: &str,
	signal: Option<AbortSignal>,
) -> LocalBoxFuture<'static, PiResult<JsValue>> {
	let (object, provider_id) = (object.clone(), JsValue::from(provider_id));
	Box::pin(async move {
		let (options, _guard) = options_object(signal.as_ref());
		invoke(&object, key, &[provider_id, options]).await
	})
}

/// `{ signal }` for a Rust signal, `undefined` without one; the guard keeps the JS signal following the Rust one.
fn options_object(signal: Option<&AbortSignal>) -> (JsValue, Option<crate::abort::JsSignal>) {
	let Some(signal) = signal else {
		return (JsValue::UNDEFINED, None);
	};
	let signal = crate::abort::to_js(signal);
	let object = Object::new();
	let _ = Reflect::set(&object, &"signal".into(), &signal.js);
	(object.into(), Some(signal))
}

async fn invoke(object: &JsValue, key: &str, args: &[JsValue]) -> PiResult<JsValue> {
	let function: js_sys::Function = Reflect::get(object, &key.into())
		.map_err(crate::error::from_js)?
		.dyn_into()
		.map_err(|_| PiError::type_error(format!("{key} is not a function")))?;
	crate::callback::call(&function, object, args).await
}

/// A JS value read with serde, `undefined` and `null` as `None`.
fn optional<T: DeserializeOwned>(value: JsValue) -> PiResult<Option<T>> {
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	crate::json::from_js(value).map(Some)
}
