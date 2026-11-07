//! Port of `packages/ai/src/cli.ts`.
//! PORT-STATUS: reviewed
//!
//! The whole module needs the `native` feature: it reads the terminal, writes `auth.json` and exits the
//! process. `run()` stands for the module's top-level `main().catch(...)`; a binary calls it. Differences
//! from the TypeScript:
//! - There is no readline interface. `prompt` prints the question and reads one line from the process's
//!   stdin on tokio's blocking pool, so the runtime keeps serving the OAuth callback server meanwhile, and
//!   successive prompts share the standard library's stdin buffer.
//! - When stdin ends, a prompt rejects with `stdin closed`; Node's readline never answers the question.
//! - `PROVIDERS` is built once in `run()` and passed down, instead of at module load.
//! - An `auth.json` that holds valid JSON but not an object loads as `{}`.
#![cfg(feature = "native")]

use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use pi_host::abort::AbortController;
use pi_host::error::{PiError, PiResult};
use pi_host::js::json_stringify_pretty;
use serde_json::{Map, Value};

use crate::auth::oauth::openrouter::random_uuid;
use crate::auth::types::{AuthEvent, AuthPrompt, AuthPromptKind, LoginOptions, OAuthAuth, ProviderAuthInteraction};
use crate::models::Provider;
use crate::providers::all::builtin_providers;

const AUTH_FILE: &str = "auth.json";

/// Runs the CLI on a native runtime and exits with status 1 after printing the error, if any.
pub fn run() {
	pi_host::native::run(|| async {
		let providers: Vec<Provider> = builtin_providers()
			.into_iter()
			.filter(|provider| provider.auth.oauth.is_some())
			.collect();
		if let Err(error) = main(&providers).await {
			eprintln!("Error: {}", error.message);
			std::process::exit(1);
		}
	});
}

async fn prompt(question: &str) -> PiResult<String> {
	print!("{question}");
	let _ = std::io::stdout().flush();
	let read = tokio::task::spawn_blocking(|| {
		let mut line = String::new();
		std::io::stdin().read_line(&mut line).map(|bytes| (bytes, line))
	})
	.await
	.map_err(|error| PiError::new(error.to_string()))?;
	match read {
		Ok((0, _)) => Err(PiError::new("stdin closed")),
		Ok((_, line)) => Ok(line.trim_end_matches(['\n', '\r']).to_string()),
		Err(error) => Err(PiError::new(error.to_string())),
	}
}

fn load_auth() -> Map<String, Value> {
	if !Path::new(AUTH_FILE).exists() {
		return Map::new();
	}
	let Ok(bytes) = std::fs::read(AUTH_FILE) else {
		return Map::new();
	};
	match crate::utils::json_parse::json_parse(&String::from_utf8_lossy(&bytes)) {
		Ok(Value::Object(auth)) => auth,
		_ => Map::new(),
	}
}

fn save_auth(auth: &Map<String, Value>) -> PiResult<()> {
	// OAuth tokens: readable by the user only. `mode` applies when the file is created; chmod covers a file an older
	// version created, before the new tokens are in it.
	let contents = json_stringify_pretty(auth, 2);
	#[cfg(unix)]
	{
		use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

		if Path::new(AUTH_FILE).exists() {
			std::fs::set_permissions(AUTH_FILE, std::fs::Permissions::from_mode(0o600)).map_err(io_error)?;
		}
		let mut file = std::fs::OpenOptions::new()
			.write(true)
			.create(true)
			.truncate(true)
			.mode(0o600)
			.open(AUTH_FILE)
			.map_err(io_error)?;
		file.write_all(contents.as_bytes()).map_err(io_error)
	}
	#[cfg(not(unix))]
	std::fs::write(AUTH_FILE, contents).map_err(io_error)
}

fn io_error(error: std::io::Error) -> PiError {
	PiError::new(error.to_string())
}

async fn answer_prompt(auth_prompt: AuthPrompt) -> PiResult<String> {
	if let AuthPromptKind::Select { message, options } = &auth_prompt.kind {
		println!("\n{message}");
		for (index, option) in options.iter().enumerate() {
			println!("  {}. {}", index + 1, option.label);
		}
		let choice =
			parse_int(&prompt(&format!("Enter number (1-{}): ", options.len())).await?).map(|number| number - 1);
		let Some(selected) = choice
			.and_then(|choice| usize::try_from(choice).ok())
			.and_then(|choice| options.get(choice))
		else {
			return Err(PiError::new("Invalid selection"));
		};
		return Ok(selected.id.clone());
	}
	let placeholder = match auth_prompt.kind.placeholder() {
		Some(placeholder) if !placeholder.is_empty() => format!(" ({placeholder})"),
		_ => String::new(),
	};
	prompt(&format!("{}{placeholder}: ", auth_prompt.kind.message())).await
}

/// `Number.parseInt(text, 10)`; `None` stands for NaN.
fn parse_int(text: &str) -> Option<i64> {
	let text = text.trim_start_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}');
	let (negative, digits) = match text.as_bytes().first() {
		Some(b'-') => (true, &text[1..]),
		Some(b'+') => (false, &text[1..]),
		_ => (false, text),
	};
	let end = digits.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(digits.len());
	let number: i64 = digits[..end].parse().ok()?;
	Some(if negative { -number } else { number })
}

async fn login(providers: &[Provider], provider_id: &str) -> PiResult<()> {
	let Some(oauth) = providers
		.iter()
		.find(|entry| entry.id == provider_id)
		.and_then(|entry| entry.auth.oauth.as_ref())
	else {
		return Err(PiError::new(format!("Unknown provider: {provider_id}")));
	};
	// This dev CLI does not persist an installation ID; apps should reuse one across logins.
	let credential = OAuthAuth::login(
		oauth,
		ProviderAuthInteraction {
			signal: AbortController::new().signal(),
			prompt: Rc::new(|auth_prompt| Box::pin(answer_prompt(auth_prompt))),
			notify: Rc::new(|event| match event {
				AuthEvent::AuthUrl { url, instructions } => {
					println!("\nOpen this URL in your browser:\n{url}");
					if let Some(instructions) = instructions.filter(|instructions| !instructions.is_empty()) {
						println!("{instructions}");
					}
				}
				AuthEvent::DeviceCode {
					user_code,
					verification_uri,
					..
				} => {
					println!("\nOpen this URL in your browser:\n{verification_uri}");
					println!("Enter code: {user_code}");
				}
				AuthEvent::Info { message, .. } | AuthEvent::Progress { message } => println!("{message}"),
			}),
		},
		Some(LoginOptions {
			get_device_id: Some(Rc::new(random_uuid)),
			..LoginOptions::default()
		}),
	)
	.await?;
	let mut auth = load_auth();
	auth.insert(provider_id.to_string(), serde_json::to_value(&credential)?);
	save_auth(&auth)?;
	println!("\nCredentials saved to {AUTH_FILE}");
	Ok(())
}

async fn main(providers: &[Provider]) -> PiResult<()> {
	let args: Vec<String> = std::env::args_os()
		.skip(1)
		.map(|arg| arg.to_string_lossy().into_owned())
		.collect();
	let command = args.first().map(String::as_str);
	if matches!(command, None | Some("" | "help" | "--help" | "-h")) {
		let provider_list = providers
			.iter()
			.map(|provider| format!("  {:<20} {}", provider.id, provider.name))
			.collect::<Vec<_>>()
			.join("\n");
		println!(
			"Usage: npx @at-inc/pi-ai <command> [provider]\n\nCommands:\n  login [provider]  Login to an OAuth provider\n  list              List available providers\n\nProviders:\n{provider_list}"
		);
		return Ok(());
	}
	if command == Some("list") {
		for provider in providers {
			println!("{:<20} {}", provider.id, provider.name);
		}
		return Ok(());
	}
	if command == Some("login") {
		let mut provider_id = args.get(1).cloned();
		if provider_id.as_deref().is_none_or(str::is_empty) {
			for (index, provider) in providers.iter().enumerate() {
				println!("  {}. {}", index + 1, provider.name);
			}
			let index =
				parse_int(&prompt(&format!("Enter number (1-{}): ", providers.len())).await?).map(|number| number - 1);
			provider_id = index
				.and_then(|index| usize::try_from(index).ok())
				.and_then(|index| providers.get(index))
				.map(|provider| provider.id.clone());
		}
		let Some(provider_id) = provider_id.clone().filter(|provider_id| {
			!provider_id.is_empty() && providers.iter().any(|provider| &provider.id == provider_id)
		}) else {
			return Err(PiError::new(format!(
				"Unknown provider: {}",
				provider_id.unwrap_or_default()
			)));
		};
		login(providers, &provider_id).await?;
		return Ok(());
	}
	Err(PiError::new(format!(
		"Unknown command: {}",
		command.unwrap_or_default()
	)))
}
