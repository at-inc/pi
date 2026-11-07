//! Port of `packages/schema/test/coerce-typebox.test.ts`: TypeBox 1.3.27's Value.Convert tests
//! (test/typebox/runtime/value/convert, MIT, (c) Haydn Paterson), run against pass 2 of the coercion alone.
//!
//! The TypeScript builds the schemas with TypeBox. Rust has no TypeBox, so each schema, input and result here is the
//! JSON that the TypeScript test produced at run time (TypeBox schemas are JSON Schema); every assertion of the original
//! holds for those results, so a Rust result equal to them passes the original's assertions. Cases whose schema, input
//! or result has no JSON form (`undefined`, `bigint`, `Date`, `Uint8Array`, `NaN`, `-0`) are ignored, as are the cases
//! the TypeScript skips. Numbers compare as JavaScript numbers.

use pi_schema::checks::deep_equal;
use pi_schema::coerce::Coercer;
use pi_schema::compile;
use pi_schema::compiler::Compiler;
use pi_schema::formats::formats;
use pi_schema::types::{CompileOptions, Node};
use serde_json::Value;

fn options() -> CompileOptions {
	CompileOptions {
		formats: Some(formats()),
		..CompileOptions::default()
	}
}

fn parse(json: &str) -> Value {
	serde_json::from_str(json).expect("the test's JSON is valid")
}

/// Pass 2 alone, as TypeBox's Value.Convert(schema, value), checked against the TypeScript's result.
fn convert(schema: &str, value: &str, expected: &str) {
	let schema = parse(schema);
	let node = Node::from_value(&schema);
	let coercer = Coercer::new(Compiler::new(node.clone(), &options(), false), node);
	let value = parse(value);
	let actual = coercer.convert_only(&value).expect("convert_only").into_owned();
	let expected = parse(expected);
	assert!(
		deep_equal(&actual, &expected),
		"converted {value} to {actual}, expected {expected}"
	);
}

/// Value.Check(schema, value), checked against the TypeScript's result.
fn check(schema: &str, value: &str, expected: bool) {
	let validator = compile(&parse(schema), &options()).expect("compile");
	assert_eq!(validator.check(&parse(value)), expected, "check of {value}");
}

/// Value.Convert.Any
mod value_convert_any {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{}"#, r#"null"#, r#"null"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_2() {}

	#[test]
	fn should_convert_3() {
		convert(r#"{}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{}"#, r#"42"#, r#"42"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{}"#, r#"{"x":1}"#, r#"{"x":1}"#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{}"#, r#"[1,2,3]"#, r#"[1,2,3]"#);
	}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn should_convert_8() {}
}

/// Value.Convert.Array
mod value_convert_array {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"array","items":{"type":"number"}}"#,
			r#"[1,3.14,"1","3.14",true,false,"true","false","hello"]"#,
			r#"[1,3.14,1,3.14,1,0,1,0,"hello"]"#,
		);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"type":"array","items":{"type":"boolean"}}"#,
			r#"[1,3.14,"1","3.14",true,false,"true","false","hello"]"#,
			r#"[true,3.14,true,"3.14",true,false,true,false,"hello"]"#,
		);
	}

	#[test]
	fn should_convert_3() {
		convert(
			r#"{"type":"array","items":{"type":"string"}}"#,
			r#"[1,3.14,"1","3.14",true,false,"true","false","hello"]"#,
			r#"["1","3.14","1","3.14","true","false","true","false","hello"]"#,
		);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"array","items":{"type":"number"}}"#, r#""1""#, r#"[1]"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{"type":"array","items":{"type":"number"}}"#, r#"[]"#, r#"[]"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"array","items":{"type":"number"}}"#, r#"42"#, r#"[42]"#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"array","items":{"type":"boolean"}}"#, r#"true"#, r#"[true]"#);
	}

	#[test]
	fn should_convert_8() {
		convert(r#"{"type":"array","items":{"type":"null"}}"#, r#"null"#, r#"[null]"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_9() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn should_convert_10() {}

	#[test]
	fn should_convert_11() {
		convert(
			r#"{"type":"array","items":{"type":"object","properties":{}}}"#,
			r#"{}"#,
			r#"[{}]"#,
		);
	}

	#[test]
	fn should_convert_12() {
		convert(
			r#"{"type":"array","items":{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}}}"#,
			r#"{"x":1}"#,
			r#"[{"x":1}]"#,
		);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"array","items":{"type":"number"}}"#, r#""42""#, r#"[42]"#);
	}

	#[test]
	fn should_convert_14() {
		convert(
			r#"{"type":"array","items":{"type":"boolean"}}"#,
			r#""true""#,
			r#"[true]"#,
		);
	}

	#[test]
	fn should_convert_15() {
		convert(r#"{"type":"array","items":{"type":"boolean"}}"#, r#"0"#, r#"[false]"#);
	}

	#[test]
	fn should_convert_16() {
		convert(
			r#"{"type":"array","items":{"type":"number"}}"#,
			r#""hello""#,
			r#"["hello"]"#,
		);
	}

	#[test]
	fn should_convert_17() {
		convert(r#"{"type":"array","items":{"type":"number"}}"#, r#"[1]"#, r#"[1]"#);
	}

	#[test]
	fn should_convert_18() {
		convert(
			r#"{"type":"array","items":{"type":"array","items":{"type":"number"}}}"#,
			r#"[1,2,3]"#,
			r#"[[1],[2],[3]]"#,
		);
		check(
			r#"{"type":"array","items":{"type":"array","items":{"type":"number"}}}"#,
			r#"[[1],[2],[3]]"#,
			true,
		);
	}

	#[test]
	fn should_convert_19() {
		convert(
			r#"{"type":"array","items":{"type":"number"}}"#,
			r#"{"0":1,"length":1}"#,
			r#"[{"0":1,"length":1}]"#,
		);
	}

	#[test]
	fn should_convert_20() {
		convert(
			r#"{"type":"array","items":{"type":"array","items":{"type":"number"}}}"#,
			r#"42"#,
			r#"[[42]]"#,
		);
		check(
			r#"{"type":"array","items":{"type":"array","items":{"type":"number"}}}"#,
			r#"[[42]]"#,
			true,
		);
	}

	#[test]
	fn should_convert_21() {
		convert(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#"1"#,
			r#"[1]"#,
		);
		check(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#"[1]"#,
			true,
		);
	}

	#[test]
	fn should_convert_22() {
		convert(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#""hello""#,
			r#"["hello"]"#,
		);
		check(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#"["hello"]"#,
			true,
		);
	}

	#[test]
	fn should_convert_23() {
		convert(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#"[1,"hello",true,"3.14"]"#,
			r#"[1,"hello",1,"3.14"]"#,
		);
		check(
			r#"{"type":"array","items":{"anyOf":[{"type":"number"},{"type":"string"}]}}"#,
			r#"[1,"hello",1,"3.14"]"#,
			true,
		);
	}

	#[test]
	fn should_convert_24() {
		convert(
			r#"{"anyOf":[{"type":"array","items":{"type":"number"}},{"type":"string"}]}"#,
			r#""1""#,
			r#""1""#,
		);
		check(
			r#"{"anyOf":[{"type":"array","items":{"type":"number"}},{"type":"string"}]}"#,
			r#""1""#,
			true,
		);
	}

	#[test]
	fn should_convert_25() {
		convert(
			r#"{"type":"array","items":{"anyOf":[{"type":"array","items":{"type":"number"}},{"type":"string"}]}}"#,
			r#"42"#,
			r#"[[42]]"#,
		);
		check(
			r#"{"type":"array","items":{"anyOf":[{"type":"array","items":{"type":"number"}},{"type":"string"}]}}"#,
			r#"[[42]]"#,
			true,
		);
	}

	#[test]
	fn should_convert_26() {
		convert(
			r#"{"type":"array","items":{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}}"#,
			r#"{"x":1,"y":2}"#,
			r#"[{"x":1,"y":2}]"#,
		);
		check(
			r#"{"type":"array","items":{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}}"#,
			r#"[{"x":1,"y":2}]"#,
			true,
		);
	}

	#[test]
	fn should_convert_27() {
		convert(
			r#"{"type":"array","items":{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}}"#,
			r#"[{"x":"1","y":"2"},{"x":"3","y":"4"}]"#,
			r#"[{"x":1,"y":2},{"x":3,"y":4}]"#,
		);
		check(
			r#"{"type":"array","items":{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}}"#,
			r#"[{"x":1,"y":2},{"x":3,"y":4}]"#,
			true,
		);
	}

	#[test]
	fn should_convert_28() {
		convert(
			r#"{"allOf":[{"type":"array","items":{"type":"number"}},{"type":"array","items":{"type":"number"}}]}"#,
			r#"42"#,
			r#"[42]"#,
		);
		check(
			r#"{"allOf":[{"type":"array","items":{"type":"number"}},{"type":"array","items":{"type":"number"}}]}"#,
			r#"[42]"#,
			true,
		);
	}

	#[test]
	fn should_convert_29() {
		convert(
			r#"{"allOf":[{"type":"array","items":{"type":"number"}},{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}}]}"#,
			r#"42"#,
			r#"[42]"#,
		);
		check(
			r#"{"allOf":[{"type":"array","items":{"type":"number"}},{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}}]}"#,
			r#"[42]"#,
			false,
		);
	}
}

/// Value.Convert.Boolean
mod value_convert_boolean {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"boolean"}"#, r#""1""#, r#"true"#);
	}

	#[test]
	fn should_convert_2() {
		convert(r#"{"type":"boolean"}"#, r#""3.14""#, r#""3.14""#);
	}

	#[test]
	fn should_convert_3() {
		convert(r#"{"type":"boolean"}"#, r#""true""#, r#"true"#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"boolean"}"#, r#""false""#, r#"false"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{"type":"boolean"}"#, r#"1"#, r#"true"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"boolean"}"#, r#"3.14"#, r#"3.14"#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"boolean"}"#, r#"1.1"#, r#"1.1"#);
	}

	#[test]
	fn should_convert_8() {
		convert(r#"{"type":"boolean"}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_9() {
		convert(r#"{"type":"boolean"}"#, r#"false"#, r#"false"#);
	}

	#[test]
	fn should_convert_10() {
		convert(r#"{"type":"boolean"}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_11() {
		convert(r#"{"type":"boolean"}"#, r#""true""#, r#"true"#);
	}

	#[test]
	fn should_convert_12() {
		convert(r#"{"type":"boolean"}"#, r#""TRUE""#, r#"true"#);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"boolean"}"#, r#""false""#, r#"false"#);
	}

	#[test]
	fn should_convert_14() {
		convert(r#"{"type":"boolean"}"#, r#""0""#, r#"false"#);
	}

	#[test]
	fn should_convert_15() {
		convert(r#"{"type":"boolean"}"#, r#""1""#, r#"true"#);
	}

	#[test]
	fn should_convert_16() {
		convert(r#"{"type":"boolean","default":true}"#, r#""0""#, r#"false"#);
	}

	#[test]
	fn should_convert_17() {
		convert(r#"{"type":"boolean","default":false}"#, r#""1""#, r#"true"#);
	}

	#[test]
	fn should_convert_18() {
		convert(r#"{"type":"boolean","default":true}"#, r#""2""#, r#""2""#);
	}

	#[test]
	fn should_convert_19() {
		convert(r#"{"type":"boolean"}"#, r#"0"#, r#"false"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_20() {}

	#[test]
	fn should_convert_21() {
		convert(r#"{"type":"boolean"}"#, r#"1"#, r#"true"#);
	}

	#[test]
	fn should_convert_22() {
		convert(r#"{"type":"boolean"}"#, r#"2"#, r#"2"#);
	}

	#[test]
	fn should_convert_23() {
		convert(r#"{"type":"boolean","default":true}"#, r#"0"#, r#"false"#);
	}

	#[test]
	fn should_convert_24() {
		convert(r#"{"type":"boolean","default":false}"#, r#"1"#, r#"true"#);
	}

	#[test]
	fn should_convert_25() {
		convert(r#"{"type":"boolean","default":true}"#, r#"2"#, r#"2"#);
	}

	#[test]
	fn should_convert_26() {
		convert(r#"{"type":"boolean"}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_27() {
		convert(r#"{"type":"boolean"}"#, r#"false"#, r#"false"#);
	}

	#[test]
	fn should_convert_28() {
		convert(r#"{"type":"boolean"}"#, r#"{}"#, r#"{}"#);
	}

	#[test]
	fn should_convert_29() {
		convert(r#"{"type":"boolean"}"#, r#"[]"#, r#"[]"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_17_again() {}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_18_again() {}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_19_again() {}

	#[test]
	fn should_convert_20_again() {
		convert(r#"{"type":"boolean"}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_21_again() {
		convert(r#"{"type":"boolean"}"#, r#"false"#, r#"false"#);
	}

	#[test]
	fn should_convert_22_again() {
		convert(r#"{"type":"boolean"}"#, r#"null"#, r#"false"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_23_again() {}
}

/// Value.Convert.Enum
mod value_convert_enum {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"enum":[1,2,"hello"]}}}"#,
			r#"{"x":"1"}"#,
			r#"{"x":1}"#,
		);
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"enum":[1,2,"hello"]}}}"#,
			r#"{"x":"2"}"#,
			r#"{"x":2}"#,
		);
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"enum":[1,2,"hello"]}}}"#,
			r#"{"x":"hello"}"#,
			r#"{"x":"hello"}"#,
		);
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"enum":[1,2,"hello"]}}}"#,
			r#"{"x":"world"}"#,
			r#"{"x":"world"}"#,
		);
	}
}

/// Value.Convert.Integer
mod value_convert_integer {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"integer"}"#, r#""3.14""#, r#"3"#);
	}

	#[test]
	fn should_convert_2() {
		convert(r#"{"type":"integer"}"#, r#""42""#, r#"42"#);
	}

	#[test]
	fn should_convert_3() {
		convert(r#"{"type":"integer"}"#, r#"true"#, r#"1"#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"integer"}"#, r#"false"#, r#"0"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{"type":"integer"}"#, r#"3.14"#, r#"3"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"integer"}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"integer"}"#, r#""3.14""#, r#"3"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_8() {}

	#[test]
	fn should_convert_9() {
		convert(r#"{"type":"integer"}"#, r#""-100""#, r#"-100"#);
	}

	#[test]
	fn should_convert_10() {
		convert(r#"{"type":"integer"}"#, r#"42"#, r#"42"#);
	}

	#[test]
	fn should_convert_11() {
		convert(r#"{"type":"integer"}"#, r#"true"#, r#"1"#);
	}

	#[test]
	fn should_convert_12() {
		convert(r#"{"type":"integer"}"#, r#"false"#, r#"0"#);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"integer"}"#, r#"{}"#, r#"{}"#);
	}

	#[test]
	fn should_convert_14() {
		convert(r#"{"type":"integer"}"#, r#"[]"#, r#"[]"#);
	}

	#[test]
	fn should_convert_15() {
		convert(r#"{"type":"integer"}"#, r#"1738213389080"#, r#"1738213389080"#);
	}

	#[test]
	fn should_convert_16() {
		convert(r#"{"type":"integer"}"#, r#"1738213389080.5554"#, r#"1738213389080"#);
	}

	#[test]
	fn should_convert_17() {
		convert(r#"{"type":"integer"}"#, r#""1738213389080""#, r#"1738213389080"#);
	}

	#[test]
	fn should_convert_18() {
		convert(r#"{"type":"integer"}"#, r#""1738213389080.555""#, r#"1738213389080"#);
	}
}

/// Value.Convert.Intersect
mod value_convert_intersect {
	use super::*;

	#[test]
	fn should_convert_intersected_objects() {
		convert(
			r#"{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}"#,
			r#"{"x":"1","y":"2"}"#,
			r#"{"x":1,"y":2}"#,
		);
	}

	#[test]
	fn should_complex_intersect_1() {
		convert(
			r#"{"allOf":[{"type":"number"},{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}"#,
			r#"{"x":"1","y":"2"}"#,
			r#"{"x":1,"y":2}"#,
		);
	}

	#[test]
	fn should_complex_intersect_2() {
		convert(
			r#"{"allOf":[{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}},{"type":"number"}]}"#,
			r#"{"x":"3","y":"4"}"#,
			r#"{"x":3,"y":4}"#,
		);
	}

	#[test]
	fn should_complex_intersect_3() {
		convert(
			r#"{"allOf":[{"type":"number"},{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}},{"type":"object","required":["y"],"properties":{"y":{"type":"number"}}}]}"#,
			r#""123""#,
			r#"123"#,
		);
	}
}

/// Value.Convert.Literal
mod value_convert_literal {
	use super::*;

	#[test]
	fn literal_string_should_convert_1() {
		convert(r#"{"type":"string","const":"1"}"#, r#"1"#, r#""1""#);
	}

	#[test]
	fn literal_string_should_convert_2() {
		convert(r#"{"type":"string","const":"1"}"#, r#"2"#, r#"2"#);
	}

	#[test]
	fn literal_string_should_convert_3() {
		convert(r#"{"type":"string","const":"true"}"#, r#"true"#, r#""true""#);
	}

	#[test]
	fn literal_number_should_convert_1() {
		convert(r#"{"type":"number","const":3.14}"#, r#""3.14""#, r#"3.14"#);
	}

	#[test]
	fn literal_number_should_convert_2() {
		convert(r#"{"type":"number","const":3.14}"#, r#""3.15""#, r#""3.15""#);
	}

	#[test]
	fn literal_number_should_convert_3() {
		convert(r#"{"type":"number","const":1}"#, r#"true"#, r#"1"#);
	}

	#[test]
	fn literal_number_should_convert_4() {
		convert(r#"{"type":"number","const":0}"#, r#"false"#, r#"0"#);
	}

	#[test]
	fn literal_number_should_convert_5() {
		convert(r#"{"type":"number","const":2}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn literal_boolean_should_convert_1() {
		convert(r#"{"type":"boolean","const":true}"#, r#"3.14"#, r#"3.14"#);
	}

	#[test]
	fn literal_boolean_should_convert_2() {
		convert(r#"{"type":"boolean","const":true}"#, r#"1"#, r#"true"#);
	}

	#[test]
	fn literal_boolean_should_convert_3() {
		convert(r#"{"type":"boolean","const":true}"#, r#""true""#, r#"true"#);
	}

	#[test]
	fn literal_boolean_should_convert_4() {
		convert(r#"{"type":"boolean","const":false}"#, r#""false""#, r#"false"#);
	}

	#[test]
	fn literal_boolean_should_convert_5() {
		convert(r#"{"type":"boolean","const":true}"#, r#""1""#, r#"true"#);
	}

	#[test]
	fn literal_boolean_should_convert_6() {
		convert(r#"{"type":"boolean","const":false}"#, r#""0""#, r#"false"#);
	}

	#[test]
	fn literal_boolean_should_convert_7() {
		convert(r#"{"type":"boolean","const":false}"#, r#""2""#, r#""2""#);
	}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_1() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_2() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_3() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_4() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_5() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_6() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_7() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_1_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_2_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_3_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_4_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_5_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_6_again() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn literal_big_int_should_convert_7_again() {}
}

/// Value.Convert.Never
mod value_convert_never {
	use super::*;

	#[test]
	fn should_not_1() {
		convert(r#"{"not":{}}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_not_2() {
		convert(r#"{"not":{}}"#, r#"42"#, r#"42"#);
	}

	#[test]
	fn should_not_3() {
		convert(r#"{"not":{}}"#, r#""true""#, r#""true""#);
	}
}

/// Value.Convert.Null
mod value_convert_null {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"null"}"#, r#""null""#, r#"null"#);
	}

	#[test]
	fn should_convert_2() {
		convert(r#"{"type":"null"}"#, r#""NULL""#, r#"null"#);
	}

	#[test]
	fn should_convert_3() {
		convert(r#"{"type":"null"}"#, r#""nil""#, r#""nil""#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"null"}"#, r#""undefined""#, r#"null"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{"type":"null"}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"null"}"#, r#"null"#, r#"null"#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"null"}"#, r#"0"#, r#"null"#);
	}

	#[test]
	fn should_convert_8() {
		convert(r#"{"type":"null"}"#, r#"false"#, r#"null"#);
	}

	#[test]
	fn should_convert_9() {
		convert(r#"{"type":"null"}"#, r#""""#, r#"null"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_10() {}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_11() {}

	#[test]
	fn should_convert_12() {
		convert(r#"{"type":"null"}"#, r#"false"#, r#"null"#);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"null"}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_14() {
		convert(r#"{"type":"null"}"#, r#"0"#, r#"null"#);
	}

	#[test]
	fn should_convert_15() {
		convert(r#"{"type":"null"}"#, r#"1"#, r#"1"#);
	}

	#[test]
	fn should_convert_16() {
		convert(r#"{"type":"null"}"#, r#"null"#, r#"null"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_17() {}

	#[test]
	#[ignore = "skipped in the TypeScript: the schema or the value has no JSON form"]
	fn should_convert_18() {}
}

/// Value.Convert.Number
mod value_convert_number {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"number"}"#, r#""3.14""#, r#"3.14"#);
	}

	#[test]
	fn should_convert_2() {
		convert(r#"{"type":"number"}"#, r#""42""#, r#"42"#);
	}

	#[test]
	fn should_convert_3() {
		convert(r#"{"type":"number"}"#, r#"true"#, r#"1"#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"number"}"#, r#"false"#, r#"0"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{"type":"number"}"#, r#"3.14"#, r#"3.14"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"number"}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"number"}"#, r#""3.14""#, r#"3.14"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_8() {}

	#[test]
	fn should_convert_9() {
		convert(r#"{"type":"number"}"#, r#""-100""#, r#"-100"#);
	}

	#[test]
	fn should_convert_10() {
		convert(r#"{"type":"number"}"#, r#"42"#, r#"42"#);
	}

	#[test]
	fn should_convert_11() {
		convert(r#"{"type":"number"}"#, r#"true"#, r#"1"#);
	}

	#[test]
	fn should_convert_12() {
		convert(r#"{"type":"number"}"#, r#"false"#, r#"0"#);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"number"}"#, r#"{}"#, r#"{}"#);
	}

	#[test]
	fn should_convert_14() {
		convert(r#"{"type":"number"}"#, r#"[]"#, r#"[]"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_10_again() {}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_11_again() {}

	#[test]
	fn should_convert_12_again() {
		convert(r#"{"type":"number"}"#, r#"null"#, r#"0"#);
	}

	#[test]
	fn should_convert_13_again() {
		convert(r#"{"type":"number"}"#, r#""false""#, r#"0"#);
	}

	#[test]
	fn should_convert_14_again() {
		convert(r#"{"type":"number"}"#, r#""true""#, r#"1"#);
	}

	#[test]
	fn should_convert_15() {
		convert(r#"{"type":"number"}"#, r#""123""#, r#"123"#);
	}

	#[test]
	fn should_convert_16() {
		convert(r#"{"type":"number"}"#, r#""123.456""#, r#"123.456"#);
	}

	#[test]
	fn should_convert_17() {
		convert(r#"{"type":"number"}"#, r#""123.456""#, r#"123.456"#);
	}

	#[test]
	fn should_convert_18() {
		convert(r#"{"type":"number"}"#, r#""123n""#, r#"123"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_19() {}

	#[test]
	fn should_convert_20() {
		convert(
			r#"{"type":"number"}"#,
			r#""90071992547409910n""#,
			r#""90071992547409910n""#,
		);
	}
}

/// Value.Convert.Object
mod value_convert_object {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"object","required":["x","y","z"],"properties":{"x":{"type":"number"},"y":{"type":"boolean"},"z":{"type":"boolean"}}}"#,
			r#"{"x":"42","y":"true","z":"hello"}"#,
			r#"{"x":42,"y":true,"z":"hello"}"#,
		);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"type":"object","required":["x","y"],"properties":{"x":{"type":"number"},"y":{"type":"boolean"}}}"#,
			r#"{"x":"42","y":"true","z":"hello"}"#,
			r#"{"x":42,"y":true,"z":"hello"}"#,
		);
	}

	#[test]
	fn should_convert_3() {
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"type":"number"}}}"#,
			r#"{}"#,
			r#"{}"#,
		);
	}

	#[test]
	fn should_convert_4() {
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"type":"number"}},"additionalProperties":{"type":"null"}}"#,
			r#"{"x":"1","y":0}"#,
			r#"{"x":1,"y":null}"#,
		);
	}

	#[test]
	fn should_convert_5() {
		convert(
			r#"{"type":"object","properties":{"x":{"type":"number"}}}"#,
			r#"{"x":1}"#,
			r#"{"x":1}"#,
		);
	}

	#[test]
	fn should_convert_6() {
		convert(
			r#"{"type":"object","properties":{"x":{"type":"number"}}}"#,
			r#"{"x":"1"}"#,
			r#"{"x":1}"#,
		);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_7() {}
}

/// Value.Convert.Record
mod value_convert_record {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"object","patternProperties":{"^.*$":{"type":"number"}}}"#,
			r#"{"x":"42","y":"24","z":"hello"}"#,
			r#"{"x":42,"y":24,"z":"hello"}"#,
		);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"anyOf":[{"type":"null"},{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}}]}"#,
			r#"{}"#,
			r#"{}"#,
		);
	}

	#[test]
	fn should_convert_3() {
		convert(
			r#"{"anyOf":[{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}},{"type":"null"}]}"#,
			r#"{}"#,
			r#"{}"#,
		);
	}

	#[test]
	fn should_convert_4() {
		convert(
			r#"{"anyOf":[{"type":"null"},{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}}]}"#,
			r#"null"#,
			r#"null"#,
		);
	}

	#[test]
	fn should_convert_5() {
		convert(
			r#"{"anyOf":[{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}},{"type":"null"}]}"#,
			r#"null"#,
			r#"null"#,
		);
	}

	#[test]
	fn should_convert_6() {
		convert(
			r#"{"anyOf":[{"type":"null"},{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}}]}"#,
			r#""NULL""#,
			r#"null"#,
		);
	}

	#[test]
	fn should_convert_7() {
		convert(
			r#"{"anyOf":[{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{}}},{"type":"null"}]}"#,
			r#""NULL""#,
			r#"null"#,
		);
	}

	#[test]
	fn should_convert_8() {
		convert(
			r#"{"type":"object","patternProperties":{"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$":{"type":"number"}},"additionalProperties":{"type":"null"}}"#,
			r#"{"0":"1","x":0}"#,
			r#"{"0":1,"x":null}"#,
		);
	}
}

/// Value.Convert.String
mod value_convert_string {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"string"}"#, r#"3.14"#, r#""3.14""#);
	}

	#[test]
	fn should_convert_2() {
		convert(r#"{"type":"string"}"#, r#"3"#, r#""3""#);
	}

	#[test]
	fn should_convert_3() {
		convert(r#"{"type":"string"}"#, r#"true"#, r#""true""#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{"type":"string"}"#, r#"false"#, r#""false""#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_5() {}

	#[test]
	fn should_convert_6() {
		convert(r#"{"type":"string"}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{"type":"string"}"#, r#"42"#, r#""42""#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_8() {}

	#[test]
	fn should_convert_9() {
		convert(r#"{"type":"string"}"#, r#"true"#, r#""true""#);
	}

	#[test]
	fn should_convert_10() {
		convert(r#"{"type":"string"}"#, r#"false"#, r#""false""#);
	}

	#[test]
	fn should_convert_11() {
		convert(r#"{"type":"string"}"#, r#"{}"#, r#"{}"#);
	}

	#[test]
	fn should_convert_12() {
		convert(r#"{"type":"string"}"#, r#"[]"#, r#"[]"#);
	}

	#[test]
	fn should_convert_13() {
		convert(r#"{"type":"string"}"#, r#"null"#, r#""null""#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_14() {}
}

/// Value.Convert.TemplateLiteral
mod value_convert_template_literal {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{"type":"string","pattern":"^(1|2|3)$"}"#, r#"1"#, r#""1""#);
		convert(r#"{"type":"string","pattern":"^(1|2|3)$"}"#, r#"2"#, r#""2""#);
		convert(r#"{"type":"string","pattern":"^(1|2|3)$"}"#, r#"3"#, r#""3""#);
		convert(r#"{"type":"string","pattern":"^(1|2|3)$"}"#, r#"4"#, r#""4""#);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"type":"object","required":["test"],"properties":{"test":{"type":"string","pattern":"^-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?(a|b|c)$"}}}"#,
			r#"{"test":"123b"}"#,
			r#"{"test":"123b"}"#,
		);
	}
}

/// Value.Convert.Tuple
mod value_convert_tuple {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"["1","true"]"#,
			r#"[1,1]"#,
		);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"["1"]"#,
			r#"[1]"#,
		);
	}

	#[test]
	fn should_convert_3() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"["1","2","3"]"#,
			r#"[1,2,"3"]"#,
		);
	}

	#[test]
	fn should_convert_4() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#""not an array""#,
			r#""not an array""#,
		);
	}

	#[test]
	fn should_convert_5() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"42"#,
			r#"42"#,
		);
	}

	#[test]
	fn should_convert_6() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"null"#,
			r#"null"#,
		);
	}

	#[test]
	fn should_convert_7() {
		convert(
			r#"{"type":"array","additionalItems":false,"items":[{"type":"number"},{"type":"number"}],"minItems":2}"#,
			r#"{"0":"1","1":"2"}"#,
			r#"{"0":"1","1":"2"}"#,
		);
	}
}

/// Value.Convert.Union
mod value_convert_union {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"anyOf":[{"type":"number"},{"type":"null"}]}}}"#,
			r#"{"x":"42"}"#,
			r#"{"x":42}"#,
		);
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"anyOf":[{"type":"number"},{"type":"null"}]}}}"#,
			r#"{"x":"null"}"#,
			r#"{"x":null}"#,
		);
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"anyOf":[{"type":"number"},{"type":"null"}]}}}"#,
			r#"{"x":"hello"}"#,
			r#"{"x":"hello"}"#,
		);
	}

	#[test]
	fn should_convert_2() {
		convert(
			r#"{"type":"object","required":["x"],"properties":{"x":{"anyOf":[{"type":"boolean"},{"type":"number"}]}}}"#,
			r#"{"x":"1"}"#,
			r#"{"x":true}"#,
		);
	}

	#[test]
	fn should_convert_3() {
		convert(
			r#"{"allOf":[{"anyOf":[{"type":"object","required":["a"],"properties":{"a":{"type":"number"}}},{"type":"object","required":["b"],"properties":{"b":{"type":"number"}}}]},{"type":"object","required":["c"],"properties":{"c":{"type":"number"}}}]}"#,
			r#"{"a":"1","c":"2"}"#,
			r#"{"a":1,"c":2}"#,
		);
		convert(
			r#"{"allOf":[{"anyOf":[{"type":"object","required":["a"],"properties":{"a":{"type":"number"}}},{"type":"object","required":["b"],"properties":{"b":{"type":"number"}}}]},{"type":"object","required":["c"],"properties":{"c":{"type":"number"}}}]}"#,
			r#"{"b":"1","c":"2"}"#,
			r#"{"b":1,"c":2}"#,
		);
		convert(
			r#"{"allOf":[{"anyOf":[{"type":"object","required":["a"],"properties":{"a":{"type":"number"}}},{"type":"object","required":["b"],"properties":{"b":{"type":"number"}}}]},{"type":"object","required":["c"],"properties":{"c":{"type":"number"}}}]}"#,
			r#"{"a":"1","b":"2","c":"3"}"#,
			r#"{"a":1,"b":"2","c":3}"#,
		);
	}

	#[test]
	fn should_guard_against_array_conversion_in_object() {
		convert(
			r#"{"anyOf":[{"type":"object","required":["type","values"],"properties":{"type":{"type":"string","const":"A"},"values":{"anyOf":[{"type":"string"},{"type":"number"}]}}},{"type":"object","required":["type","values"],"properties":{"type":{"type":"string","const":"B"},"values":{"type":"string"}}}]}"#,
			r#"[{"type":"A","values":1}]"#,
			r#"[{"type":"A","values":1}]"#,
		);
	}

	#[test]
	fn should_guard_against_array_conversion_in_record() {
		convert(
			r#"{"anyOf":[{"type":"object","patternProperties":{"^values$":{"anyOf":[{"type":"string"},{"type":"number"}]}}},{"type":"object","patternProperties":{"^type$":{"anyOf":[{"type":"string"},{"type":"number"}]}}}]}"#,
			r#"[{"type":"A","values":1}]"#,
			r#"[{"type":"A","values":1}]"#,
		);
	}
}

/// Value.Convert.Unknown
mod value_convert_unknown {
	use super::*;

	#[test]
	fn should_convert_1() {
		convert(r#"{}"#, r#"null"#, r#"null"#);
	}

	#[test]
	#[ignore = "the schema, the value or the result has no JSON form"]
	fn should_convert_2() {}

	#[test]
	fn should_convert_3() {
		convert(r#"{}"#, r#""hello""#, r#""hello""#);
	}

	#[test]
	fn should_convert_4() {
		convert(r#"{}"#, r#"42"#, r#"42"#);
	}

	#[test]
	fn should_convert_5() {
		convert(r#"{}"#, r#"true"#, r#"true"#);
	}

	#[test]
	fn should_convert_6() {
		convert(r#"{}"#, r#"{"x":1}"#, r#"{"x":1}"#);
	}

	#[test]
	fn should_convert_7() {
		convert(r#"{}"#, r#"[1,2,3]"#, r#"[1,2,3]"#);
	}
}
