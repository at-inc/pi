//! Port of `packages/ai/src/auth/context.ts`.
//! PORT-STATUS: reviewed
//!
//! TS loads `node:fs/promises` and `node:os` at runtime and reports `false` where they are missing (browsers). The
//! port asks the host: `pi_host::file_exists` for `fs.access` and `pi_host::home_dir` for `os.homedir()`, both
//! `false`/`None` on hosts without a file system. The file system is checked when the future is first polled.

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
		let path = path.to_string();
		Box::pin(async move {
			let mut resolved = path;
			if resolved.starts_with('~') {
				let Some(home) = pi_host::home_dir() else {
					return false;
				};
				resolved = format!("{home}{}", &resolved[1..]);
			}
			pi_host::file_exists(&resolved)
		})
	}
}

/// Default auth context: env vars from `process.env` (undefined in browsers),
/// file existence via node:fs (always false in browsers).
pub fn default_provider_auth_context() -> Rc<dyn AuthContext> {
	Rc::new(DefaultProviderAuthContext)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn file_exists_checks_the_host_file_system_and_expands_the_home_dir() {
		pi_host::testing::run(|host| async move {
			host.set_home_dir("/home/pi");
			host.add_file("/home/pi/.aws/credentials");
			host.add_file("/etc/app.json");
			let ctx = default_provider_auth_context();
			assert!(ctx.file_exists("~/.aws/credentials").await);
			assert!(ctx.file_exists("/etc/app.json").await);
			assert!(!ctx.file_exists("~/.config/gcloud").await);
		});
	}

	#[test]
	fn file_exists_is_false_for_a_home_path_without_a_file_system() {
		pi_host::testing::run(|host| async move {
			host.add_file("/.aws/credentials");
			assert!(!default_provider_auth_context().file_exists("~/.aws/credentials").await);
		});
	}

	#[test]
	fn env_drops_blank_values() {
		pi_host::testing::run(|host| async move {
			host.set_env("BLANK", " \t");
			host.set_env("SET", " key ");
			let ctx = default_provider_auth_context();
			assert_eq!(ctx.env("BLANK").await, None);
			assert_eq!(ctx.env("SET").await.as_deref(), Some(" key "));
			assert_eq!(ctx.env("MISSING").await, None);
		});
	}
}
