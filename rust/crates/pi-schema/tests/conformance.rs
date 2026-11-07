//! Port of `packages/schema/test/conformance.test.ts`: the official JSON-Schema-Test-Suite, drafts 2020-12 and 7:
//! every test, including optional and format tests. A test listed in KNOWN_FAILURES must fail; when one starts
//! passing, remove it from the list.
//!
//! Rust port: the TypeScript makes one `it` per suite test; here one test per draft runs them all and reports every
//! failing case by its name (`file > group > test`).

mod suite;

use std::rc::Rc;

use pi_schema::formats::formats;
use pi_schema::{CompileOptions, Draft, compile};
use suite::{suite_files, suite_schemas};

const KNOWN_FAILURES: [&str; 9] = [
	// Vocabularies switched off by a custom meta-schema are not supported.
	"draft2020-12/vocabulary > schema that uses custom metaschema with with no validation vocabulary > no validation: invalid number, but it still validates",
	"draft2020-12/optional/format-assertion > schema that uses custom metaschema with format-assertion: false > format-assertion: false: invalid string",
	"draft2020-12/optional/format-assertion > schema that uses custom metaschema with format-assertion: true > format-assertion: true: invalid string",
	// A $ref to a document of another draft is evaluated with the compile's draft.
	"draft2020-12/optional/cross-draft > refs to historic drafts are processed as historic drafts > first item not a string is valid",
	"draft7/optional/cross-draft > refs to future drafts are processed as future drafts > missing bar is invalid",
	// contentMediaType and contentEncoding are annotations.
	"draft7/optional/content > validation of string-encoded content based on media type > an invalid JSON document",
	"draft7/optional/content > validation of binary string-encoding > an invalid base64 string (% is not a valid character)",
	"draft7/optional/content > validation of binary-encoded media type documents > a validly-encoded invalid JSON document",
	"draft7/optional/content > validation of binary-encoded media type documents > an invalid base64 string that is valid JSON",
];

fn run(draft: Draft) {
	let schemas = Rc::new(suite_schemas());
	let mut failures = Vec::new();
	let mut count = 0;
	for file in suite_files().into_iter().filter(|file| file.draft == draft) {
		// Format tests assert formats; everywhere else `format` is an annotation, as draft 2020-12 specifies.
		let options = CompileOptions {
			draft: Some(file.draft),
			schemas: Some(schemas.clone()),
			formats: file.name.contains("/optional/format/").then(formats),
			..CompileOptions::default()
		};
		for group in &file.groups {
			for test in &group.tests {
				count += 1;
				let name = format!("{} > {} > {}", file.name, group.description, test.description);
				let known = KNOWN_FAILURES.contains(&name.as_str());
				let validator = match compile(&group.schema, &options) {
					Ok(validator) => validator,
					Err(error) => {
						if !known {
							failures.push(format!("{name}: compile threw {}", error.message));
						}
						continue;
					}
				};
				let passed = validator.check(&test.data) == test.valid;
				if passed == known {
					failures.push(if known {
						format!("{name}: known failure passes")
					} else {
						format!("{name}: check")
					});
				}
				// (errors agree with check)
				match validator.errors(&test.data) {
					Ok(errors) if errors.is_empty() == validator.check(&test.data) => {}
					Ok(_) => failures.push(format!("{name} (errors agree with check)")),
					Err(error) => failures.push(format!("{name} (errors agree with check): threw {}", error.message)),
				}
			}
		}
	}
	assert!(count > 0, "no suite tests found");
	assert!(
		failures.is_empty(),
		"{} of {count} suite tests failed:\n{}",
		failures.len(),
		failures.join("\n")
	);
}

#[test]
fn draft2020_12() {
	run(Draft::V2020_12);
}

#[test]
fn draft7() {
	run(Draft::V7);
}
