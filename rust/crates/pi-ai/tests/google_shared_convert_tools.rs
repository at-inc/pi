//! Port of `packages/ai/test/google-shared-convert-tools.test.ts`.
//!
//! `toEqual` compares JSON values, whose object equality ignores key order like `toEqual`. `it.each` cases run in a
//! loop. Rust values are never shared, so "does not mutate the original" holds by construction; the checks stay.

use pi_ai::api::google_shared::{
	convert_tools, resolve_google_function_calling_mode, supports_google_strict_tool_sampling,
};
use pi_ai::types::{ConstrainedSamplingConfig, ConstrainedSamplingStrictness, Tool, ToolConstrainedSampling};
use pi_host::js::json_stringify;
use serde_json::{Map, Value, json};

fn make_tool(parameters: Value) -> Tool {
	Tool {
		name: "test_tool".into(),
		description: "A test tool".into(),
		parameters,
		..Tool::default()
	}
}

/// `result?.[0]?.functionDeclarations?.[0]`.
fn first_declaration(tools: &[Tool], use_parameters: Option<bool>) -> Value {
	convert_tools(tools, use_parameters, None).unwrap().unwrap()[0]["functionDeclarations"][0].clone()
}

mod google_shared_convert_tools {
	use super::*;

	#[test]
	fn strips_json_schema_meta_keys_from_parameters_when_use_parameters_true() {
		let tools = [make_tool(json!({
			"$schema": "http://json-schema.org/draft-07/schema#",
			"$id": "urn:bash-tool",
			"$comment": "A bash tool for demonstration",
			"$defs": { "commandDef": { "type": "string" } },
			"definitions": { "legacyDef": { "type": "number" } },
			"type": "object",
			"properties": { "command": { "type": "string" } },
			"required": ["command"],
		}))];

		let decl = first_declaration(&tools, Some(true));
		assert_eq!(
			decl["parameters"],
			json!({
				"type": "object",
				"properties": { "command": { "type": "string" } },
				"required": ["command"],
			})
		);
		for key in ["$schema", "$id", "$comment", "$defs", "definitions"] {
			assert!(decl["parameters"].get(key).is_none(), "{key}");
		}
	}

	#[test]
	fn recursively_strips_nested_json_schema_meta_keys() {
		let tools = [make_tool(json!({
			"$schema": "http://json-schema.org/draft-07/schema#",
			"type": "object",
			"properties": {
				"deep": {
					"$schema": "http://json-schema.org/draft-07/schema#",
					"$id": "urn:nested",
					"type": "string",
				},
			},
		}))];

		let decl = first_declaration(&tools, Some(true));
		assert_eq!(
			decl["parameters"],
			json!({ "type": "object", "properties": { "deep": { "type": "string" } } })
		);
	}

	#[test]
	fn preserves_ref_while_stripping_meta_keys() {
		let tools = [make_tool(json!({
			"$schema": "http://json-schema.org/draft-07/schema#",
			"type": "object",
			"properties": { "refProp": { "$ref": "#/$defs/someDef", "type": "string" } },
		}))];

		let decl = first_declaration(&tools, Some(true));
		assert_eq!(
			decl["parameters"],
			json!({
				"type": "object",
				"properties": { "refProp": { "$ref": "#/$defs/someDef", "type": "string" } },
			})
		);
	}

	#[test]
	fn does_not_mutate_the_original_tool_parameters_object() {
		let original_parameters = json!({
			"$schema": "http://json-schema.org/draft-07/schema#",
			"type": "object",
			"properties": { "command": { "type": "string" } },
			"required": ["command"],
		});
		let tools = [make_tool(original_parameters.clone())];

		convert_tools(&tools, Some(true), None).unwrap();

		assert_eq!(tools[0].parameters, original_parameters);
	}

	#[test]
	fn preserves_schema_in_parameters_json_schema_when_use_parameters_false() {
		let parameters = json!({
			"$schema": "http://json-schema.org/draft-07/schema#",
			"type": "object",
			"properties": { "command": { "type": "string" } },
			"required": ["command"],
		});
		let decl = first_declaration(&[make_tool(parameters.clone())], Some(false));
		assert_eq!(decl["parametersJsonSchema"], parameters);
	}

	#[test]
	fn handles_tools_without_schema_gracefully() {
		let parameters = json!({
			"type": "object",
			"properties": { "path": { "type": "string" } },
			"required": ["path"],
		});
		let decl = first_declaration(&[make_tool(parameters.clone())], Some(true));
		assert_eq!(decl["parameters"], parameters);
	}

	#[test]
	fn uses_validated_function_calling_for_strict_tools_on_gemini_3() {
		let mut tool = make_tool(json!({ "type": "object", "properties": {} }));
		tool.constrained_sampling = Some(ToolConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
			strict: ConstrainedSamplingStrictness::Require,
		}));

		assert!(supports_google_strict_tool_sampling("gemini-3.1-pro-preview"));
		assert!(!supports_google_strict_tool_sampling("gemini-2.5-pro"));
		assert_eq!(
			resolve_google_function_calling_mode(std::slice::from_ref(&tool), None, true)
				.unwrap()
				.map(|mode| mode.as_str()),
			Some("VALIDATED")
		);
		let error = resolve_google_function_calling_mode(&[tool], None, false).unwrap_err();
		assert!(
			error
				.message
				.contains("Tool \"test_tool\" requires JSON-schema constrained sampling"),
			"{}",
			error.message
		);
	}

	#[test]
	fn returns_undefined_for_empty_tool_list() {
		assert_eq!(convert_tools(&[], None, None).unwrap(), None);
		assert_eq!(convert_tools(&[], Some(true), None).unwrap(), None);
	}
}

// Regression coverage for #11308.
mod google_shared_convert_tools_const_handling {
	use super::*;

	#[test]
	fn preserves_literal_data_under_key() {
		for key in ["default", "examples", "enum", "const", "x-custom"] {
			let literal = json!({ "const": "literal-data", "properties": { "nested": { "const": 1 } } });
			let mut value = Map::new();
			value.insert("type".into(), json!("object"));
			value.insert(
				key.into(),
				if key == "enum" || key == "examples" {
					json!([literal])
				} else {
					literal
				},
			);
			let parameters = json!({ "type": "object", "properties": { "value": value } });
			let original = parameters.clone();

			let tools = [make_tool(parameters)];
			assert_eq!(
				first_declaration(&tools, None)["parametersJsonSchema"],
				original,
				"{key}"
			);
			assert_eq!(tools[0].parameters, original, "{key}");
		}
	}

	#[test]
	fn preserves_names_while_rewriting_schemas_in_key() {
		for key in [
			"properties",
			"patternProperties",
			"$defs",
			"definitions",
			"dependentSchemas",
			"dependencies",
		] {
			let mut parameters = Map::new();
			parameters.insert("type".into(), json!("object"));
			parameters.insert(
				key.into(),
				json!({
					"__proto__": { "const": "x" },
					"const": { "const": "y" },
					"properties": { "const": "z" },
				}),
			);
			let parameters = Value::Object(parameters);
			let original = parameters.clone();
			let mut expected = Map::new();
			expected.insert("type".into(), json!("object"));
			expected.insert(
				key.into(),
				json!({
					"__proto__": { "enum": ["x"] },
					"const": { "enum": ["y"] },
					"properties": { "enum": ["z"] },
				}),
			);
			let expected = Value::Object(expected);

			let tools = [make_tool(parameters)];
			let result = first_declaration(&tools, None)["parametersJsonSchema"].clone();
			assert_eq!(result, expected, "{key}");
			assert_eq!(json_stringify(&result), json_stringify(&expected), "{key}");
			assert_eq!(tools[0].parameters, original, "{key}");
		}
	}

	#[test]
	fn rewrites_only_schema_bearing_keywords() {
		let parameters = json!({
			"type": "object",
			"additionalProperties": { "const": "x" },
			"unevaluatedProperties": false,
			"propertyNames": { "const": "name" },
			"allOf": [{ "if": { "const": "x" }, "then": { "const": "y" }, "else": { "not": { "const": "z" } } }],
			"properties": {
				"list": {
					"type": "array",
					"items": [{ "const": 1 }],
					"prefixItems": [{ "const": 2 }],
					"additionalItems": { "const": 3 },
					"contains": { "const": 4 },
					"unevaluatedItems": { "const": 5 },
				},
			},
			"dependencies": { "list": ["other"] },
		});

		assert_eq!(
			first_declaration(&[make_tool(parameters)], None)["parametersJsonSchema"],
			json!({
				"type": "object",
				"additionalProperties": { "enum": ["x"] },
				"unevaluatedProperties": false,
				"propertyNames": { "enum": ["name"] },
				"allOf": [{ "if": { "enum": ["x"] }, "then": { "enum": ["y"] }, "else": { "not": { "enum": ["z"] } } }],
				"properties": {
					"list": {
						"type": "array",
						"items": [{ "enum": [1] }],
						"prefixItems": [{ "enum": [2] }],
						"additionalItems": { "enum": [3] },
						"contains": { "enum": [4] },
						"unevaluatedItems": { "enum": [5] },
					},
				},
				"dependencies": { "list": ["other"] },
			})
		);
	}

	#[test]
	fn rewrites_string_and_number_const_values_to_single_value_enums_in_parameters_json_schema() {
		let parameters = json!({
			"type": "object",
			"properties": {
				"origin": { "type": "string", "const": "zq-src-7" },
				"mode": {
					"anyOf": [
						{ "type": "string", "const": "a" },
						{ "type": "string", "const": "b" },
					],
				},
				"level": { "oneOf": [{ "const": 1 }, { "const": 2 }] },
				"flag": { "type": "boolean", "const": true },
				"list": { "type": "array", "items": { "const": "x" } },
				"const": { "type": "string" },
			},
			"required": ["origin"],
		});
		let tools = [make_tool(parameters)];

		let decl = first_declaration(&tools, None);

		assert_eq!(
			decl["parametersJsonSchema"],
			json!({
				"type": "object",
				"properties": {
					"origin": { "type": "string", "enum": ["zq-src-7"] },
					"mode": {
						"anyOf": [
							{ "type": "string", "enum": ["a"] },
							{ "type": "string", "enum": ["b"] },
						],
					},
					"level": { "oneOf": [{ "enum": [1] }, { "enum": [2] }] },
					"flag": { "type": "boolean", "const": true },
					"list": { "type": "array", "items": { "enum": ["x"] } },
					"const": { "type": "string" },
				},
				"required": ["origin"],
			})
		);
		assert_eq!(
			tools[0].parameters["properties"]["origin"],
			json!({ "type": "string", "const": "zq-src-7" })
		);
	}
}
