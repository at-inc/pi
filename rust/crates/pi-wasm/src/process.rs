//! `pi_host::process::ChildProcess` over the host's `spawnProcess` (`PiChildProcess`): the options and listeners
//! go to JS as plain objects and functions, and the process object answers the Rust calls.

use std::rc::Rc;

use bytes::Bytes;
use js_sys::{Object, Promise, Reflect, Uint8Array};
use pi_host::process::{ChildProcess, ProcessListeners, ProcessSignal, SpawnOptions};
use pi_host::{LocalBoxFuture, PiResult};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
	type JsChild;
	#[wasm_bindgen(method, getter)]
	fn pid(this: &JsChild) -> Option<f64>;
	#[wasm_bindgen(method, catch)]
	fn write(this: &JsChild, data: &[u8]) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, js_name = endStdin)]
	fn end_stdin(this: &JsChild);
	#[wasm_bindgen(method, getter, js_name = stdinWritable)]
	fn stdin_writable(this: &JsChild) -> bool;
	#[wasm_bindgen(method, getter)]
	fn exited(this: &JsChild) -> bool;
	#[wasm_bindgen(method)]
	fn kill(this: &JsChild, signal: &str);
}

/// The `PiSpawnOptions` object.
pub fn options_to_js(options: &SpawnOptions) -> JsValue {
	let object = Object::new();
	let set = |key: &str, value: &JsValue| {
		let _ = Reflect::set(&object, &key.into(), value);
	};
	set("command", &options.command.as_str().into());
	let args: js_sys::Array = options.args.iter().map(|arg| JsValue::from_str(arg)).collect();
	set("args", &args);
	if let Some(cwd) = &options.cwd {
		set("cwd", &cwd.as_str().into());
	}
	let env = Object::new();
	for (key, value) in &options.env {
		let _ = Reflect::set(&env, &key.as_str().into(), &value.as_str().into());
	}
	set("env", &env);
	set("clearEnv", &options.clear_env.into());
	set("inheritStderr", &options.inherit_stderr.into());
	object.into()
}

/// The `PiProcessListeners` object: JS functions over the Rust listeners, owned by the JS process.
pub fn listeners_to_js(listeners: ProcessListeners) -> JsValue {
	let object = Object::new();
	let set = |key: &str, value: JsValue| {
		let _ = Reflect::set(&object, &key.into(), &value);
	};
	let ProcessListeners {
		on_stdout,
		on_stderr,
		on_error,
		on_close,
	} = listeners;
	set(
		"onStdout",
		Closure::<dyn Fn(Uint8Array)>::new(move |chunk: Uint8Array| on_stdout(&chunk.to_vec())).into_js_value(),
	);
	set(
		"onStderr",
		Closure::<dyn Fn(Uint8Array)>::new(move |chunk: Uint8Array| on_stderr(&chunk.to_vec())).into_js_value(),
	);
	set(
		"onError",
		Closure::<dyn Fn(JsValue)>::new(move |error| on_error(crate::error::from_js(error))).into_js_value(),
	);
	set("onClose", Closure::<dyn Fn()>::new(move || on_close()).into_js_value());
	object.into()
}

/// The process of a `spawnProcess` call: `undefined` is a host without processes.
pub fn spawned(spawned: Result<JsValue, JsValue>) -> LocalBoxFuture<'static, PiResult<Rc<dyn ChildProcess>>> {
	Box::pin(async move {
		let pending = spawned.map_err(crate::error::from_js)?;
		if pending.is_undefined() || pending.is_null() {
			return Err(pi_host::process::processes_unsupported());
		}
		let child = JsFuture::from(Promise::resolve(&pending))
			.await
			.map_err(crate::error::from_js)?;
		Ok(Rc::new(JsChildProcess {
			child: child.unchecked_into(),
		}) as Rc<dyn ChildProcess>)
	})
}

struct JsChildProcess {
	child: JsChild,
}

impl ChildProcess for JsChildProcess {
	fn pid(&self) -> Option<u32> {
		self.child.pid().map(|pid| pid as u32)
	}

	fn write(&self, data: Bytes) -> LocalBoxFuture<'static, PiResult<()>> {
		let written = self.child.write(&data);
		Box::pin(async move {
			let promise = written.map_err(crate::error::from_js)?;
			JsFuture::from(promise).await.map_err(crate::error::from_js)?;
			Ok(())
		})
	}

	fn end_stdin(&self) {
		self.child.end_stdin();
	}

	fn stdin_writable(&self) -> bool {
		self.child.stdin_writable()
	}

	fn exited(&self) -> bool {
		self.child.exited()
	}

	fn kill(&self, signal: ProcessSignal) {
		self.child.kill(match signal {
			ProcessSignal::Term => "SIGTERM",
			ProcessSignal::Kill => "SIGKILL",
		});
	}
}
