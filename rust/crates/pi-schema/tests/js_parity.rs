//! Rust-only: JavaScript behavior the port must keep where Rust's defaults differ, from the adversarial review of the
//! port. Each expected value is what the TypeScript source gives on Node.

use pi_schema::formats::formats;
use pi_schema::shared::JsRegex;
use pi_schema::{CompileOptions, compile, compile_coercing, obj, opt, str};
use serde_json::{Value, json};

fn test(source: &str, flags: &str, value: &str) -> Result<bool, String> {
	JsRegex::new(source, flags)
		.map(|re| re.test(value))
		.map_err(|error| error.message)
}

fn compile_error(schema: Value) -> String {
	compile(&schema, &CompileOptions::default())
		.err()
		.expect("the schema does not compile")
		.message
}

fn paths(schema: Value, value: Value) -> Vec<String> {
	let validator = compile(&schema, &CompileOptions::default()).unwrap();
	validator
		.errors(&value)
		.unwrap()
		.into_iter()
		.map(|error| error.path)
		.collect()
}

#[test]
fn regex_backreferences_follow_javascript() {
	assert_eq!(test(r"^(a)\1\x30$", "u", "aa0"), Ok(true));
	assert_eq!(test(r"^\1(a)$", "", "a"), Ok(true));
	assert_eq!(test(r"^\k<n>(?<n>a)$", "u", "a"), Ok(true));
	assert_eq!(test(r"^(?=(a))?(b)\2$", "", "bb"), Ok(true));
	assert_eq!(test(r"^(?=(a))*b(b)\1$", "", "bb"), Ok(true));
	assert_eq!(test(r"^(?:(?<a>x)|(?<a>y))\k<a>$", "u", "yy"), Ok(true));
	assert_eq!(test(r"^(?:(?<a>x)|(?<a>y))\k<a>$", "u", "xy"), Ok(false));
	assert_eq!(
		test("(?<a>x)(?<a>y)", "u", "xy"),
		Err("Invalid regular expression: /(?<a>x)(?<a>y)/u: Duplicate capture group name".into())
	);
}

#[test]
fn regex_zero_width_repeats_follow_javascript() {
	assert_eq!(test("(?:)*?", "u", "a"), Ok(true));
	assert_eq!(test("^(?i:)+a$", "", "a"), Ok(true));
	assert_eq!(test("^(?:(?<=a))*b$", "u", "b"), Ok(true));
	assert_eq!(test("^(?:^)*a", "u", "a"), Ok(true));
	assert_eq!(test(r"^(?=(a))?\1b", "", "ab"), Ok(false));
	assert_eq!(test(r"^(?:(?=(a)))*\1b", "u", "ab"), Ok(false));
	assert_eq!(test(r"^(?:(?=(a)))+\1b$", "u", "ab"), Ok(true));
}

#[test]
fn regex_references_inside_or_before_their_group_match_empty() {
	assert_eq!(test(r"^(\1a)$", "u", "a"), Ok(true));
	assert_eq!(test(r"^(?<n>a\k<n>)$", "u", "a"), Ok(true));
	assert_eq!(test(r"^(?:\1(b))+$", "u", "bb"), Ok(true));
}

#[test]
fn regex_group_names_follow_javascript() {
	assert_eq!(test(r"(?<\u0061>x)\k<a>", "u", "xx"), Ok(true));
	assert_eq!(test("(?<a\u{301}>x)", "u", "x"), Ok(true));
	assert!(test("(?<a\u{b2}>x)", "u", "x").is_err());
	assert_eq!(test(r"^(?<g_e9>a)(?<é>b)\k<g_e9>$", "u", "aba"), Ok(true));
}

#[test]
fn regex_properties_follow_ecmascript_names() {
	assert!(test(r"^\p{Greek}$", "u", "α").is_err());
	assert!(test(r"^\p{LeTTer}$", "u", "a").is_err());
	assert_eq!(test(r"^\p{Script=Greek}$", "u", "α"), Ok(true));
	assert_eq!(test(r"^\p{space}$", "u", " "), Ok(true));
	assert_eq!(test(r"^\p{Script=Unknown}$", "u", "\u{378}"), Ok(true));
	assert_eq!(test(r"^\p{Cs}$", "u", "a"), Ok(false));
	assert_eq!(test(r"^\P{Script=Sidetic}$", "u", "a"), Ok(true));
}

#[test]
fn regex_quantifiers_and_lines_follow_javascript() {
	assert_eq!(test(r"^.{0,100000}$", "u", "abc"), Ok(true));
	assert_eq!(test(r"^a{0,99999999999}$", "u", "aaa"), Ok(true));
	assert_eq!(test(r"^a{4294967296}$", "u", "aaa"), Ok(false));
	assert_eq!(test(r"[]+a", "", "a"), Ok(false));
	assert_eq!(test(r"(?m:^b)", "", "a\rb"), Ok(true));
	assert_eq!(test(r"^b", "m", "a\u{2028}b"), Ok(true));
	assert_eq!(
		test(r"^(?:(a|a)+\bb|a+c)$", "", &format!("{}c", "a".repeat(18))),
		Ok(true)
	);
}

#[test]
fn regex_errors_use_v8_wording() {
	let message = |source: &str, flags: &str| test(source, flags, "").unwrap_err();
	assert_eq!(
		message("(?ii:a)", ""),
		"Invalid regular expression: /(?ii:a)/: Repeated flag in flag group"
	);
	assert_eq!(
		message("(?-:a)", ""),
		"Invalid regular expression: /(?-:a)/: Invalid flag group"
	);
	assert_eq!(
		message("(?<=a)*b", ""),
		"Invalid regular expression: /(?<=a)*b/: Invalid quantifier"
	);
	assert_eq!(
		message("a{,5}", "u"),
		"Invalid regular expression: /a{,5}/u: Incomplete quantifier"
	);
	assert_eq!(
		message(r"\c1", "u"),
		"Invalid regular expression: /\\c1/u: Invalid Unicode escape"
	);
	assert_eq!(
		message(r"\p{Greek}", "ui"),
		"Invalid regular expression: /\\p{Greek}/iu: Invalid property name"
	);
	assert_eq!(
		message(r"(?<a>x)[\k]", ""),
		"Invalid regular expression: /(?<a>x)[\\k]/: Invalid escape"
	);
	assert_eq!(
		message(r"[\1]", "u"),
		"Invalid regular expression: /[\\1]/u: Invalid decimal escape"
	);
	assert_eq!(
		message(r"[\8]", "u"),
		"Invalid regular expression: /[\\8]/u: Invalid escape"
	);
	assert_eq!(
		message(r"[\p{L]", "u"),
		"Invalid regular expression: /[\\p{L]/u: Invalid property name in character class"
	);
	assert_eq!(
		message(r"\b{2,1}", ""),
		"Invalid regular expression: /\\b{2,1}/: Nothing to repeat"
	);
	assert_eq!(
		message(r"(?<a>x)\k<a", "u"),
		"Invalid regular expression: /(?<a>x)\\k<a/u: Invalid capture group name"
	);
}

// V8 checks named references after the whole pattern parsed, and a group's name when the group closes.
#[test]
fn regex_errors_come_in_v8_order() {
	let message = |source: &str, flags: &str| test(source, flags, "").unwrap_err();
	assert_eq!(
		message(r"\k<m>(?<n>)(", "u"),
		"Invalid regular expression: /\\k<m>(?<n>)(/u: Unterminated group"
	);
	assert_eq!(
		message(r"(?<a>x)\k<b>[\k]", ""),
		"Invalid regular expression: /(?<a>x)\\k<b>[\\k]/: Invalid escape"
	);
	assert_eq!(
		message("(?<a>(?<a>x)(?x:))", "u"),
		"Invalid regular expression: /(?<a>(?<a>x)(?x:))/u: Invalid group"
	);
	assert_eq!(
		message(r"[\k](?<1>)", ""),
		"Invalid regular expression: /[\\k](?<1>)/: Invalid escape"
	);
	assert_eq!(
		message("(?<a>(?<a>x))", "u"),
		"Invalid regular expression: /(?<a>(?<a>x))/u: Duplicate capture group name"
	);
}

#[test]
fn integer_like_keys_come_first() {
	assert_eq!(
		paths(
			json!({"properties": {"b": {"type": "string"}, "1": {"type": "string"}}}),
			json!({"b": 0, "1": 0})
		),
		["/1", "/b"]
	);
	assert_eq!(
		paths(
			json!({"dependentRequired": {"b": ["x"]}, "dependencies": {"1": ["y"]}}),
			json!({"b": 0, "1": 0})
		),
		["/y", "/x"]
	);
	assert_eq!(
		compile_error(json!({"properties": {"b": 7, "1": 8}})),
		"A schema must be an object or a boolean, got 8"
	);
	let anchored = json!({"$defs": {"b": {"$anchor": "x", "type": "string"}, "1": {"$anchor": "x", "type": "number"}}, "$ref": "#x"});
	assert!(
		compile(&anchored, &CompileOptions::default())
			.unwrap()
			.check(&json!("s"))
	);
	assert_eq!(
		serde_json::to_string(&str(json!({"10": 1}))).unwrap(),
		r#"{"10":1,"type":"string"}"#
	);
	let object = obj(
		[("b", str(Value::Null).into()), ("1", opt(str(Value::Null)).into())],
		Value::Null,
	);
	assert_eq!(object["required"], json!(["b"]));
	assert_eq!(
		object["properties"].as_object().unwrap().keys().collect::<Vec<_>>(),
		["1", "b"]
	);
}

#[test]
fn large_integers_round_like_javascript_numbers() {
	assert_eq!(
		compile_error(json!({"not": 9007199254740993u64})),
		"A schema must be an object or a boolean, got 9007199254740992"
	);
	let validator = compile_coercing(&json!({"type": "number"}), &CompileOptions::default()).unwrap();
	assert_eq!(
		validator.coerce(&json!("0x200000000000011")).unwrap().as_f64(),
		Some(144115188075855900.0)
	);
}

#[test]
fn references_resolve_like_the_url_standard() {
	use pi_schema::resolver::resolve_uri;
	assert_eq!(resolve_uri("////x?y", "http://example.com/root.json"), "http://x/?y");
	assert_eq!(resolve_uri(r"/\/x", "http://a/"), "http://x/");
	assert_eq!(resolve_uri("a^b?^#^", "tag:/x/y"), "tag:/x/a%5Eb?^#^");
	assert_eq!(resolve_uri("a^b", "http://ex.com/^/c"), "http://ex.com/%5E/a%5Eb");
	assert_eq!(resolve_uri("#a", "tag:x^y"), "tag:x^y#a");
}

// Where the TypeScript runs out of stack, check() fails and errors() and parse() throw the RangeError, even when the
// recursion sits in a branch the check could pass without; coerce() treats the branch as failing, as its try does.
#[test]
fn an_endless_reference_throws_out_of_errors_and_parse() {
	let validator = compile_coercing(
		&json!({"anyOf": [{"$ref": "#"}, {"type": "number"}]}),
		&CompileOptions::default(),
	)
	.unwrap();
	assert!(!validator.check(&json!(1)));
	assert_eq!(validator.errors(&json!(1)).unwrap_err().name, "RangeError");
	assert_eq!(
		validator.parse(&json!(".5")).err().map(|error| error.name),
		Some("RangeError".into())
	);
	assert_eq!(*validator.coerce(&json!(".5")).unwrap(), json!(0.5));
	let negated = compile(
		&json!({"type": "number", "not": {"$ref": "#"}}),
		&CompileOptions::default(),
	)
	.unwrap();
	assert_eq!(negated.errors(&json!(1)).unwrap_err().name, "RangeError");
}

// A subschema that failed to compile stays in the compiler's cache as an empty indirection, as in the TypeScript, so a
// later branch that only references it compiles; calling the empty one throws the TypeScript's TypeError.
#[test]
fn a_subschema_that_failed_to_compile_stays_cached() {
	let schema = json!({
		"$ref": "#/definitions/ok",
		"definitions": {"ok": {}, "bad": {"$ref": "other.json"}},
		"anyOf": [{"$ref": "#/definitions/bad"}, {"type": "number", "properties": {"a": {"$ref": "#/definitions/bad"}}}]
	});
	let options = CompileOptions {
		draft: Some(pi_schema::Draft::V7),
		..CompileOptions::default()
	};
	let validator = compile_coercing(&schema, &options).unwrap();
	assert_eq!(*validator.coerce(&json!(".5")).unwrap(), json!(0.5));
}

// JSON.parse rounds every decimal correctly; serde_json does only with its float_roundtrip feature (without it,
// "9.727837981879871e+26" parses one unit in the last place off).
#[test]
fn long_decimals_parse_like_json_parse() {
	let schema: Value = serde_json::from_str(r#"{"exclusiveMaximum": 972783798187987123879878123.18878137}"#).unwrap();
	let value: Value = serde_json::from_str("972783798187987123879878123.18878137").unwrap();
	let short: Value = serde_json::from_str("9.727837981879871e+26").unwrap();
	assert_eq!(value.as_f64(), short.as_f64());
	let validator = compile(&schema, &CompileOptions::default()).unwrap();
	let errors = validator.errors(&value).unwrap();
	assert_eq!(errors[0].message, "expected < 9.727837981879871e+26");
}

#[test]
fn a_reference_cycle_fails_instead_of_overflowing_the_stack() {
	let validator = compile(&json!({"$ref": "#"}), &CompileOptions::default()).unwrap();
	assert!(!validator.check(&json!(1)));
	let error = validator.errors(&json!(1)).unwrap_err();
	assert_eq!(
		(error.name.as_str(), error.message.as_str()),
		("RangeError", "Maximum call stack size exceeded")
	);
}

#[test]
fn formats_follow_javascript() {
	let formats = formats();
	let format = |name: &str, value: &str| (formats.get(name).unwrap())(value);
	assert!(format("hostname", "xn--ib9b66e"));
	assert!(format("idn-hostname", "a.xn--ib9b66e"));
	assert!(format("iri-reference", "///a"));
	assert!(!format("iri-reference", "///"));
	assert!(format("idn-email", &format!("\"{}\"@b", "a".repeat(500_000))));
	assert!(!format("idn-email", "a@b-"));
}
