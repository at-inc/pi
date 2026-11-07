//! Port of `packages/schema/test/suite.ts`: loads the vendored JSON-Schema-Test-Suite: its test files, the remote
//! documents and the official meta-schemas. The fixtures stay in `packages/schema/test/fixtures`.

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use pi_schema::{Draft, JsonSchema};
use serde::Deserialize;
use serde_json::Value;

/// One test file: its path below `tests/` and its groups.
pub struct SuiteFile {
	pub name: String,
	pub draft: Draft,
	pub groups: Vec<SuiteGroup>,
}

/// A schema with values and whether each is valid.
#[derive(Deserialize)]
pub struct SuiteGroup {
	pub description: String,
	pub schema: JsonSchema,
	pub tests: Vec<SuiteTest>,
}

/// One value of a group and whether it is valid.
#[derive(Deserialize)]
pub struct SuiteTest {
	pub description: String,
	pub data: Value,
	pub valid: bool,
}

fn fixtures() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/schema/test/fixtures")
}

fn root() -> PathBuf {
	fixtures().join("json-schema-test-suite")
}

/// Every test file of drafts 2020-12 and 7.
pub fn suite_files() -> Vec<SuiteFile> {
	let mut files = Vec::new();
	for (dir, draft) in [("draft2020-12", Draft::V2020_12), ("draft7", Draft::V7)] {
		let tests = root().join("tests");
		for path in json_files(&tests.join(dir)) {
			let relative = path
				.strip_prefix(&tests)
				.expect("below tests/")
				.to_string_lossy()
				.replace('\\', "/");
			let name = relative.strip_suffix(".json").unwrap_or(&relative).to_string();
			let groups =
				serde_json::from_str(&fs::read_to_string(&path).expect("readable fixture")).expect("valid fixture");
			files.push(SuiteFile { name, draft, groups });
		}
	}
	files
}

/// The remote documents and meta-schemas the tests reference, by URI.
pub fn suite_schemas() -> IndexMap<String, JsonSchema> {
	let mut schemas = IndexMap::new();
	let remotes = root().join("remotes");
	for path in json_files(&remotes) {
		let relative = path
			.strip_prefix(&remotes)
			.expect("below remotes/")
			.to_string_lossy()
			.replace('\\', "/");
		schemas.insert(format!("http://localhost:1234/{relative}"), read_json(&path));
	}
	let meta = fixtures().join("meta-schemas");
	schemas.insert(
		"http://json-schema.org/draft-07/schema".into(),
		read_json(&meta.join("draft-07-schema.json")),
	);
	schemas.insert(
		"https://json-schema.org/draft/2020-12/schema".into(),
		read_json(&meta.join("draft2020-12-schema.json")),
	);
	for vocabulary in [
		"core",
		"applicator",
		"unevaluated",
		"validation",
		"meta-data",
		"format-annotation",
		"content",
	] {
		schemas.insert(
			format!("https://json-schema.org/draft/2020-12/meta/{vocabulary}"),
			read_json(&meta.join(format!("draft2020-12-meta-{vocabulary}.json"))),
		);
	}
	schemas
}

fn read_json(path: &Path) -> JsonSchema {
	serde_json::from_str(&fs::read_to_string(path).expect("readable fixture")).expect("valid fixture")
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
	let mut entries: Vec<_> = fs::read_dir(dir)
		.expect("readable fixture dir")
		.map(|e| e.expect("entry").path())
		.collect();
	entries.sort();
	let mut out = Vec::new();
	for path in entries {
		if path.is_dir() {
			out.extend(json_files(&path));
		} else if path.extension().is_some_and(|e| e == "json") {
			out.push(path);
		}
	}
	out
}
