//! Port of `packages/ai/test/json-parse.test.ts`, plus checks that `json_parse` throws V8's `JSON.parse` messages,
//! which reach the model through `argumentsError`. The expected messages were recorded from Node 24.

use pi_ai::utils::json_parse::{json_parse, parse_streaming_json, parse_tool_call_arguments};
use pi_host::js::json_stringify;
use serde_json::json;

mod parse_streaming_json {
	use super::*;

	// #10868
	#[test]
	fn keeps_invalid_u_escapes_as_literal_text_without_dropping_fields() {
		assert_eq!(
			parse_streaming_json(Some(r#"{"text":"\url{x}"}"#)),
			json!({ "text": "\\url{x}" })
		);
		assert_eq!(
			parse_streaming_json(Some(r#"{"path":"a.tex","content":"\usepackage{x}","mode":"w"}"#)),
			json!({ "path": "a.tex", "content": "\\usepackage{x}", "mode": "w" })
		);
	}

	#[test]
	fn decodes_valid_u_escapes() {
		assert_eq!(
			parse_streaming_json(Some(r#"{"text":"caf\u00e9 \url"}"#)),
			json!({ "text": "café \\url" })
		);
	}

	#[test]
	fn omits_a_u_escape_that_is_cut_off_mid_stream() {
		assert_eq!(
			parse_streaming_json(Some(r#"{"text":"ab\u00"#)),
			json!({ "text": "ab" })
		);
	}
}

// #10897
mod parse_tool_call_arguments {
	use super::*;

	#[test]
	fn parses_complete_json_and_repairs_invalid_escapes() {
		let result = parse_tool_call_arguments(Some(r#"{"kind":"seek","note":"\url"}"#));
		assert_eq!(json!(result.arguments), json!({ "kind": "seek", "note": "\\url" }));
		assert_eq!(result.arguments_error, None);
	}

	#[test]
	fn uses_empty_arguments_for_empty_text() {
		let result = parse_tool_call_arguments(Some(""));
		assert!(result.arguments.is_empty());
		assert_eq!(result.arguments_error, None);
	}

	#[test]
	fn does_not_keep_the_valid_prefix_of_malformed_json() {
		let raw = r#"{"action":"apply","actions":[{"kind":"seek","time":.5}]}"#;
		let result = parse_tool_call_arguments(Some(raw));
		assert!(result.arguments.is_empty());
		let error = result.arguments_error.unwrap();
		assert!(error.contains("\n\nReceived arguments:\n"));
		assert!(error.ends_with(raw));
		assert_eq!(
			error,
			format!(
				"Unexpected token '.', ...\"k\",\"time\":.5}}]}}\" is not valid JSON\n\nReceived arguments:\n{raw}"
			)
		);
	}

	#[test]
	fn does_not_complete_cut_off_json() {
		let raw = r#"{"kind":"seek","clipId":"clip-fl"#;
		let result = parse_tool_call_arguments(Some(raw));
		assert!(result.arguments.is_empty());
		assert_eq!(
			result.arguments_error.unwrap(),
			format!("Unterminated string in JSON at position 32 (line 1 column 33)\n\nReceived arguments:\n{raw}")
		);
	}

	#[test]
	fn rejects_json_that_is_not_an_object() {
		let result = parse_tool_call_arguments(Some("[1,2]"));
		assert!(result.arguments.is_empty());
		assert_eq!(
			result.arguments_error.as_deref(),
			Some("Tool arguments must be a JSON object\n\nReceived arguments:\n[1,2]")
		);
	}
}

mod json_parse {
	use super::*;

	#[test]
	fn throws_v8_syntax_error_messages() {
		for (input, message) in [
			("", "Unexpected end of JSON input"),
			("[", "Unexpected end of JSON input"),
			(
				"{",
				"Expected property name or '}' in JSON at position 1 (line 1 column 2)",
			),
			(
				"{\"a\":1",
				"Expected ',' or '}' after property value in JSON at position 6 (line 1 column 7)",
			),
			(
				"{\"a\" 1}",
				"Expected ':' after property name in JSON at position 5 (line 1 column 6)",
			),
			(
				"{\"a\":1,}",
				"Expected double-quoted property name in JSON at position 7 (line 1 column 8)",
			),
			("[1,]", "Unexpected token ']', \"[1,]\" is not valid JSON"),
			(
				"[1 2]",
				"Expected ',' or ']' after array element in JSON at position 3 (line 1 column 4)",
			),
			("01", "Unexpected number in JSON at position 1 (line 1 column 2)"),
			(
				"1 2",
				"Unexpected non-whitespace character after JSON at position 2 (line 1 column 3)",
			),
			(
				"-",
				"No number after minus sign in JSON at position 1 (line 1 column 2)",
			),
			(
				"1.e",
				"Unterminated fractional number in JSON at position 2 (line 1 column 3)",
			),
			(
				"1e+",
				"Exponent part is missing a number in JSON at position 3 (line 1 column 4)",
			),
			("\"abc", "Unterminated string in JSON at position 4 (line 1 column 5)"),
			(
				"\"a\\x\"",
				"Bad escaped character in JSON at position 3 (line 1 column 4)",
			),
			(
				"\"a\\u12\"",
				"Bad Unicode escape in JSON at position 6 (line 1 column 7)",
			),
			(
				"\"a\nb\"",
				"Bad control character in string literal in JSON at position 2 (line 1 column 3)",
			),
			("\"a\\", "Unexpected end of JSON input"),
			("tru", "Unexpected end of JSON input"),
			("nulL", "Unexpected token 'L', \"nulL\" is not valid JSON"),
			("nul1", "Unexpected number in JSON at position 3 (line 1 column 4)"),
			("[tr\"", "Unexpected string in JSON at position 3 (line 1 column 4)"),
			("NaN", "\"NaN\" is not valid JSON"),
			("[object Object]", "\"[object Object]\" is not valid JSON"),
			(
				"xxxxxxxxxxxxxxxxxxxxxxxxxx",
				"Unexpected token 'x', \"xxxxxxxxxx\"... is not valid JSON",
			),
			(
				"[1,2,3,4,5,6,7,8,9,10,11,12,13,14,x,1,2,3,4,5,6,7,8,9,10]",
				"Unexpected token 'x', ...\",12,13,14,x,1,2,3,4,\"... is not valid JSON",
			),
			(
				"[1,2,3,4,5,6,7,8,9,x]",
				"Unexpected token 'x', ...\"5,6,7,8,9,x]\" is not valid JSON",
			),
			(
				"[1,2,3,4,5,6,7,8,x]",
				"Unexpected token 'x', \"[1,2,3,4,5,6,7,8,x]\" is not valid JSON",
			),
			(
				"\r\n\r\n{\"a\" 1}",
				"Expected ':' after property name in JSON at position 9 (line 3 column 6)",
			),
			("\u{a0}{}", "Unexpected token '\u{a0}', \"\u{a0}{}\" is not valid JSON"),
		] {
			let error = json_parse(input).unwrap_err();
			assert_eq!(error.name, "SyntaxError");
			assert_eq!(error.message, message, "{input:?}");
		}
	}

	#[test]
	fn orders_keys_and_numbers_like_javascript() {
		let value = json_parse(r#"{"b":1,"1":2,"0":3,"4294967295":4,"4294967294":5,"01":6,"b":7}"#).unwrap();
		assert_eq!(
			json_stringify(&value),
			r#"{"0":3,"1":2,"4294967294":5,"b":7,"4294967295":4,"01":6}"#
		);
		let value = json_parse("[0.1,1.0,100,1e21,-0.0,123456789012345678901,1e400]").unwrap();
		assert_eq!(json_stringify(&value), "[0.1,1,100,1e+21,0,123456789012345680000,null]");
		assert_eq!(value[1], json!(1));
		assert_eq!(json_parse(r#"{"__proto__":1}"#).unwrap(), json!({ "__proto__": 1 }));
		assert_eq!(json_parse(r#""\u00e9\ud83d\ude00""#).unwrap(), json!("é😀"));
	}
}
