//! Rust-only build script (no TS counterpart): embeds the generated model catalog.
//!
//! The TS catalogs (`src/providers/<provider>.models.ts`) import `src/providers/data/<provider>.json`, which is
//! gitignored and produced by `npm run hydrate-model-data` in `packages/ai`. This script copies those files and
//! `.manifest.json` into `OUT_DIR/model-data` so the catalog modules can `include_str!` them, and fails the build
//! with instructions when they are missing. `PI_AI_MODEL_DATA_DIR` overrides the source directory.

use std::env;
use std::fs;
use std::path::PathBuf;

const PROVIDERS: &[&str] = &[
	"amazon-bedrock",
	"ant-ling",
	"anthropic",
	"azure",
	"baseten",
	"cerebras",
	"cloudflare-ai-gateway",
	"cloudflare-workers-ai",
	"deepseek",
	"fireworks",
	"github-copilot",
	"google",
	"google-vertex",
	"groq",
	"huggingface",
	"kimi-coding",
	"meta",
	"minimax",
	"minimax-cn",
	"mistral",
	"moonshotai",
	"moonshotai-cn",
	"nvidia",
	"openai",
	"openai-codex",
	"opencode",
	"opencode-go",
	"openrouter",
	"qwen-token-plan",
	"qwen-token-plan-cn",
	"qwen-token-plan-individual",
	"radius",
	"together",
	"typesafe",
	"vercel-ai-gateway",
	"xai",
	"xiaomi",
	"xiaomi-token-plan-ams",
	"xiaomi-token-plan-cn",
	"xiaomi-token-plan-sgp",
	"zai",
	"zai-coding-cn",
];

fn main() {
	println!("cargo:rerun-if-env-changed=PI_AI_MODEL_DATA_DIR");
	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
	let source = match env::var("PI_AI_MODEL_DATA_DIR") {
		Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
		_ => manifest_dir.join("../../../packages/ai/src/providers/data"),
	};
	println!("cargo:rerun-if-changed={}", source.display());
	let target = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("model-data");
	fs::create_dir_all(&target).expect("create OUT_DIR/model-data");

	let files: Vec<String> = PROVIDERS
		.iter()
		.map(|provider| format!("{provider}.json"))
		.chain([".manifest.json".to_string()])
		.collect();
	let missing: Vec<&str> = files
		.iter()
		.filter(|file| !source.join(file).is_file())
		.map(String::as_str)
		.collect();
	if !missing.is_empty() {
		panic!(
			"pi-ai: generated model data is missing from {} ({}). Run 'npm install --ignore-scripts' at the repository root, \
			 then 'npm run hydrate-model-data' in packages/ai, or set PI_AI_MODEL_DATA_DIR to a directory with the hydrated JSON.",
			source.display(),
			missing.join(", ")
		);
	}
	for file in &files {
		let path = source.join(file);
		println!("cargo:rerun-if-changed={}", path.display());
		let text =
			fs::read_to_string(&path).unwrap_or_else(|error| panic!("pi-ai: cannot read {}: {error}", path.display()));
		if let Err(error) = serde_json::from_str::<serde_json::Value>(&text) {
			panic!(
				"pi-ai: {} is not valid JSON ({error}); re-run 'npm run hydrate-model-data' in packages/ai",
				path.display()
			);
		}
		fs::write(target.join(file), text).unwrap_or_else(|error| panic!("pi-ai: cannot write {file}: {error}"));
	}
}
