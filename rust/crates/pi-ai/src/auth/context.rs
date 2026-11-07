//! Port of `packages/ai/src/auth/context.ts`.
//! PORT-STATUS: ported
//!
//! TS loads `node:fs/promises` and `node:os` at runtime and reports `false` where they are missing (browsers). The
//! Rust core does no file I/O without the `native` feature, so `fileExists` checks the file system only with
//! `native` and is always `false` otherwise. `os.homedir()` is `std::env::home_dir()`.

use std::rc::Rc;

use pi_host::LocalBoxFuture;

use crate::auth::types::AuthContext;
use crate::utils::text::js_trim;

struct DefaultProviderAuthContext;

impl AuthContext for DefaultProviderAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let value = pi_host::env(name);
		Box::pin(async move { value.filter(|value| !js_trim(value).is_empty()) })
	}

	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool> {
		let exists = file_exists(path);
		Box::pin(async move { exists })
	}
}

#[cfg(feature = "native")]
fn file_exists(path: &str) -> bool {
	let mut resolved = path.to_string();
	if resolved.starts_with('~') {
		let Some(home) = std::env::home_dir() else {
			return false;
		};
		resolved = format!("{}{}", home.to_string_lossy(), &resolved[1..]);
	}
	std::fs::metadata(&resolved).is_ok()
}

#[cfg(not(feature = "native"))]
fn file_exists(_path: &str) -> bool {
	false
}

/// Default auth context: env vars from `process.env` (undefined in browsers),
/// file existence via node:fs (always false in browsers).
pub fn default_provider_auth_context() -> Rc<dyn AuthContext> {
	Rc::new(DefaultProviderAuthContext)
}
