//! Port of `packages/schema/test/typebox-static.test.ts`. The TypeScript tests are type-level (`expectTypeOf` on
//! `Static` of TypeBox schemas, which has no runtime counterpart), except that the last one compiles and parses a TypeBox
//! schema; that runtime part is kept.

use pi_schema::{CompileOptions, ParseResult, compile_coercing};
use serde_json::json;

mod static_of_type_box_schemas {
	use super::*;

	#[test]
	fn types_compile_and_parse_for_type_box_schemas() {
		// Type.Object({ n: Type.Number(), s: Type.Optional(Type.String()) })
		let schema =
			json!({"type":"object","required":["n"],"properties":{"n":{"type":"number"},"s":{"type":"string"}}});
		let result = compile_coercing(&schema, &CompileOptions::default())
			.unwrap()
			.parse(&json!({ "n": "1" }))
			.unwrap();
		assert_eq!(result, ParseResult::Ok(json!({ "n": 1 })));
	}
}
