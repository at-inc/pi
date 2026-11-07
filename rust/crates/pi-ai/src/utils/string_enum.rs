//! Port of `packages/ai/src/utils/string-enum.ts`.
//! PORT-STATUS: ported
//!
//! The schema is a JSON value; the TS `Schema<T[number]>` type parameter has no Rust counterpart.

use serde_json::{Map, Value};

/// `StringEnum` options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StringEnumOptions {
	pub description: Option<String>,
	pub default: Option<String>,
}

/// Creates a string enum schema compatible with Google's API and other providers
/// that don't support anyOf/const patterns: `{ type: "string", enum: [...] }`.
pub fn string_enum(values: &[&str], options: Option<&StringEnumOptions>) -> Value {
	let mut schema = Map::new();
	schema.insert("type".into(), "string".into());
	schema.insert("enum".into(), values.iter().map(|value| Value::from(*value)).collect());
	if let Some(description) = options.and_then(|options| options.description.as_ref())
		&& !description.is_empty()
	{
		schema.insert("description".into(), description.as_str().into());
	}
	if let Some(default) = options.and_then(|options| options.default.as_ref())
		&& !default.is_empty()
	{
		schema.insert("default".into(), default.as_str().into());
	}
	Value::Object(schema)
}

#[cfg(test)]
mod tests {
	use super::*;
	use pi_host::js::json_stringify;

	#[test]
	fn writes_type_enum_and_truthy_options() {
		let options = StringEnumOptions {
			description: Some("The operation to perform".into()),
			default: Some(String::new()),
		};
		assert_eq!(
			json_stringify(&string_enum(&["add", "subtract"], Some(&options))),
			r#"{"type":"string","enum":["add","subtract"],"description":"The operation to perform"}"#
		);
	}
}
