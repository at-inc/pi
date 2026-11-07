//! Port of `packages/ai/test/validation-probes.test.ts`.
//!
//! The reviewers' probes of the schema switch (round 1) as regression tests: TypeBox-built and plain JSON
//! (MCP-shaped) tool schemas with arguments through validateToolArguments. Each case's outcome is "ok <coerced
//! arguments>", "REJECT <first error line>" or "THROW <message>". Cases marked as TypeBox-only record the documented
//! differences from TypeBox (see the CHANGELOG): Refine and BigInt.
//!
//! Rust: the TypeBox schemas are their JSON, as `JSON.stringify` wrote them from TypeBox 1.3.27 (functions and
//! symbols, which pi-schema does not read, are gone). The `it` blocks generated from tables are one test per
//! `describe` that runs every case and names the failing one; each table row is `[it name, tool name, schema,
//! arguments, expected outcome]` and the huge enum (`"HUGE_ENUM"`) is filled in by code as in the TS. Skipped: the two
//! Sol cases with an unpaired high surrogate (`"\ud800A"`), which a Rust string cannot hold. The cached-validator
//! case keeps its TS assertions but is `#[ignore]`d: the Rust cache is keyed by the schema's JSON (see
//! `utils/validation.rs`), so a schema changed after its first use is compiled anew.

use pi_ai::types::{Tool, ToolCall};
use pi_ai::utils::text::js_trim;
use pi_ai::utils::validation::validate_tool_arguments;
use pi_host::js::json_stringify;
use serde_json::{Value, json};

/// The outcome of one call, in the probes' format.
fn outcome(name: &str, schema: &Value, args: &Value) -> String {
	let tool = Tool {
		name: name.into(),
		description: String::new(),
		parameters: schema.clone(),
		..Tool::default()
	};
	let call = ToolCall {
		id: "1".into(),
		name: name.into(),
		arguments: args.as_object().expect("probe arguments are objects").clone(),
		..ToolCall::default()
	};
	match validate_tool_arguments(&tool, &call) {
		Ok(value) => format!("ok {}", json_stringify(&value)),
		Err(error) => {
			let message = error.message;
			if message.starts_with("Validation failed") {
				format!(
					"REJECT {}",
					message.split('\n').nth(1).map(js_trim).unwrap_or("undefined")
				)
			} else {
				format!("THROW {}", message.split('\n').next().unwrap_or_default())
			}
		}
	}
}

/// Runs every row of a table and reports each case whose outcome differs.
fn run_table(table: &str) {
	let mut failures = Vec::new();
	for row in table.lines().filter(|row| !row.trim().is_empty()) {
		let (key, name, mut schema, args, expected): (String, String, Value, Value, String) =
			serde_json::from_str(row).unwrap_or_else(|error| panic!("bad row {row}: {error}"));
		if schema["properties"]["e"]["enum"] == json!("HUGE_ENUM") {
			schema["properties"]["e"]["enum"] = (0..5000).map(|i| json!(format!("v{i}"))).collect();
		}
		let actual = outcome(&name, &schema, &args);
		if actual != expected {
			failures.push(format!("{key}\n  expected: {expected}\n  actual:   {actual}"));
		}
	}
	assert!(
		failures.is_empty(),
		"{} failing cases:\n{}",
		failures.len(),
		failures.join("\n")
	);
}

const REVIEWER_PROBES: &str = r###"
["cyclic {\"tree\":{\"name\":\"a\",\"children\":[{\"name\":\"b\",\"children\":[]}]}}","cyclic",{"type":"object","required":["tree"],"properties":{"tree":{"$defs":{"Node":{"type":"object","required":["name","children"],"properties":{"name":{"type":"string"},"children":{"type":"array","items":{"$ref":"Node"}}},"$id":"Node"}},"$ref":"Node"}}},{"tree":{"name":"a","children":[{"name":"b","children":[]}]}},"ok {\"tree\":{\"name\":\"a\",\"children\":[{\"name\":\"b\",\"children\":[]}]}}"]
["cyclic {\"tree\":{\"name\":1,\"children\":[]}}","cyclic",{"type":"object","required":["tree"],"properties":{"tree":{"$defs":{"Node":{"type":"object","required":["name","children"],"properties":{"name":{"type":"string"},"children":{"type":"array","items":{"$ref":"Node"}}},"$id":"Node"}},"$ref":"Node"}}},{"tree":{"name":1,"children":[]}},"ok {\"tree\":{\"name\":\"1\",\"children\":[]}}"]
["ref-with-$id {\"a\":{\"x\":\"1\"}}","ref-with-$id",{"type":"object","required":["a"],"properties":{"a":{"$ref":"A"}}},{"a":{"x":"1"}},"REJECT - a: unresolved reference A"]
["function {}","function",{"type":"object","properties":{"f":{"type":"function","parameters":[],"returnType":{"type":"void"}}}},{},"ok {}"]
["function {\"f\":\"x\"}","function",{"type":"object","properties":{"f":{"type":"function","parameters":[],"returnType":{"type":"void"}}}},{"f":"x"},"REJECT - f: expected function, got string"]
["codec {\"c\":\"12\"}","codec",{"type":"object","required":["c"],"properties":{"c":{"type":"string"}}},{"c":"12"},"ok {\"c\":\"12\"}"]
["codec {\"c\":12}","codec",{"type":"object","required":["c"],"properties":{"c":{"type":"string"}}},{"c":12},"ok {\"c\":\"12\"}"]
["undefined-optional {}","undefined-optional",{"type":"object","properties":{"u":{"type":"undefined"}}},{},"ok {}"]
["undefined-optional {\"u\":null}","undefined-optional",{"type":"object","properties":{"u":{"type":"undefined"}}},{"u":null},"ok {}"]
["bigint {\"n\":1}","bigint",{"type":"object","required":["n"],"properties":{"n":{"type":"bigint"}}},{"n":1},"REJECT - n: expected bigint, got integer"]
["bigint {\"n\":\"12\"}","bigint",{"type":"object","required":["n"],"properties":{"n":{"type":"bigint"}}},{"n":"12"},"REJECT - n: expected bigint, got string"]
["refine {\"n\":5}","refine",{"type":"object","required":["n"],"properties":{"n":{"type":"number"}}},{"n":5},"ok {\"n\":5}"]
["refine {\"n\":-5}","refine",{"type":"object","required":["n"],"properties":{"n":{"type":"number"}}},{"n":-5},"ok {\"n\":-5}"]
["template-literal {\"t\":\"id-12\"}","template-literal",{"type":"object","required":["t"],"properties":{"t":{"type":"string","pattern":"^id--?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$"}}},{"t":"id-12"},"ok {\"t\":\"id-12\"}"]
["template-literal {\"t\":\"id-x\"}","template-literal",{"type":"object","required":["t"],"properties":{"t":{"type":"string","pattern":"^id--?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$"}}},{"t":"id-x"},"REJECT - t: expected to match ^id--?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$"]
["template-literal {\"t\":3}","template-literal",{"type":"object","required":["t"],"properties":{"t":{"type":"string","pattern":"^id--?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$"}}},{"t":3},"REJECT - t: expected to match ^id--?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?$"]
["record-string {\"r\":{\"a\":\"1\"}}","record-string",{"type":"object","required":["r"],"properties":{"r":{"type":"object","patternProperties":{"^.*$":{"type":"number"}}}}},{"r":{"a":"1"}},"ok {\"r\":{\"a\":1}}"]
["record-string {\"r\":{\"a\":1,\"b\":\"x\"}}","record-string",{"type":"object","required":["r"],"properties":{"r":{"type":"object","patternProperties":{"^.*$":{"type":"number"}}}}},{"r":{"a":1,"b":"x"}},"REJECT - r.b: expected number, got string"]
["record-literal-keys {\"r\":{\"a\":1,\"b\":2}}","record-literal-keys",{"type":"object","required":["r"],"properties":{"r":{"type":"object","required":["a","b"],"properties":{"a":{"type":"number"},"b":{"type":"number"}}}}},{"r":{"a":1,"b":2}},"ok {\"r\":{\"a\":1,\"b\":2}}"]
["record-literal-keys {\"r\":{\"a\":1}}","record-literal-keys",{"type":"object","required":["r"],"properties":{"r":{"type":"object","required":["a","b"],"properties":{"a":{"type":"number"},"b":{"type":"number"}}}}},{"r":{"a":1}},"REJECT - r.b: missing"]
["record-literal-keys {\"r\":{\"a\":1,\"b\":2,\"c\":3}}","record-literal-keys",{"type":"object","required":["r"],"properties":{"r":{"type":"object","required":["a","b"],"properties":{"a":{"type":"number"},"b":{"type":"number"}}}}},{"r":{"a":1,"b":2,"c":3}},"ok {\"r\":{\"a\":1,\"b\":2,\"c\":3}}"]
["intersect {\"a\":\"1\",\"b\":\"true\"}","intersect",{"allOf":[{"type":"object","required":["a"],"properties":{"a":{"type":"number"}}},{"type":"object","required":["b"],"properties":{"b":{"type":"boolean"}}}]},{"a":"1","b":"true"},"ok {\"a\":1,\"b\":true}"]
["intersect {\"a\":1,\"b\":true,\"c\":1}","intersect",{"allOf":[{"type":"object","required":["a"],"properties":{"a":{"type":"number"}}},{"type":"object","required":["b"],"properties":{"b":{"type":"boolean"}}}]},{"a":1,"b":true,"c":1},"ok {\"a\":1,\"b\":true,\"c\":1}"]
["union-objects {\"v\":{\"k\":\"x\",\"n\":\"2\"}}","union-objects",{"type":"object","required":["v"],"properties":{"v":{"anyOf":[{"type":"object","required":["k","n"],"properties":{"k":{"type":"string","const":"x"},"n":{"type":"number"}}},{"type":"object","required":["k","s"],"properties":{"k":{"type":"string","const":"y"},"s":{"type":"string"}}}]}}},{"v":{"k":"x","n":"2"}},"ok {\"v\":{\"k\":\"x\",\"n\":2}}"]
["union-objects {\"v\":{\"k\":\"y\",\"s\":2}}","union-objects",{"type":"object","required":["v"],"properties":{"v":{"anyOf":[{"type":"object","required":["k","n"],"properties":{"k":{"type":"string","const":"x"},"n":{"type":"number"}}},{"type":"object","required":["k","s"],"properties":{"k":{"type":"string","const":"y"},"s":{"type":"string"}}}]}}},{"v":{"k":"y","s":2}},"ok {\"v\":{\"k\":\"y\",\"s\":\"2\"}}"]
["minLength-astral {\"s\":\"😀\"}","minLength-astral",{"type":"object","required":["s"],"properties":{"s":{"type":"string","minLength":2}}},{"s":"😀"},"REJECT - s: expected at least 2 characters"]
["maxLength-astral {\"s\":\"😀\"}","maxLength-astral",{"type":"object","required":["s"],"properties":{"s":{"type":"string","maxLength":1}}},{"s":"😀"},"ok {\"s\":\"😀\"}"]
["pattern-astral {\"s\":\"😀\"}","pattern-astral",{"type":"object","required":["s"],"properties":{"s":{"type":"string","pattern":"^.$"}}},{"s":"😀"},"ok {\"s\":\"😀\"}"]
["pattern-class-dash {\"s\":\"a-b\"}","pattern-class-dash",{"type":"object","required":["s"],"properties":{"s":{"type":"string","pattern":"^[\\w-]+$"}}},{"s":"a-b"},"ok {\"s\":\"a-b\"}"]
["pattern-unicode-prop {\"s\":\"äb\"}","pattern-unicode-prop",{"type":"object","required":["s"],"properties":{"s":{"type":"string","pattern":"^\\p{L}+$"}}},{"s":"äb"},"ok {\"s\":\"äb\"}"]
["pattern-unicode-prop {\"s\":\"p{L}\"}","pattern-unicode-prop",{"type":"object","required":["s"],"properties":{"s":{"type":"string","pattern":"^\\p{L}+$"}}},{"s":"p{L}"},"REJECT - s: expected to match ^\\p{L}+$"]
["multipleOf-float {\"n\":0.07}","multipleOf-float",{"type":"object","required":["n"],"properties":{"n":{"type":"number","multipleOf":0.01}}},{"n":0.07},"ok {\"n\":0.07}"]
["multipleOf-float {\"n\":1.1}","multipleOf-float",{"type":"object","required":["n"],"properties":{"n":{"type":"number","multipleOf":0.01}}},{"n":1.1},"ok {\"n\":1.1}"]
["uniqueItems-keyorder {\"a\":[{\"x\":1,\"y\":2},{\"y\":2,\"x\":1}]}","uniqueItems-keyorder",{"type":"object","required":["a"],"properties":{"a":{"type":"array","items":{},"uniqueItems":true}}},{"a":[{"x":1,"y":2},{"y":2,"x":1}]},"REJECT - a: expected unique items"]
["uniqueItems-keyorder {\"a\":[1,1]}","uniqueItems-keyorder",{"type":"object","required":["a"],"properties":{"a":{"type":"array","items":{},"uniqueItems":true}}},{"a":[1,1]},"REJECT - a: expected unique items"]
["const-object {\"c\":\"a\"}","const-object",{"type":"object","required":["c"],"properties":{"c":{"type":"string","const":"a"}}},{"c":"a"},"ok {\"c\":\"a\"}"]
["formats {\"e\":\"a@b.co\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"e":"a@b.co"},"ok {\"e\":\"a@b.co\"}"]
["formats {\"e\":\"not-an-email\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"e":"not-an-email"},"REJECT - e: expected format email"]
["formats {\"u\":\"relative/path\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"u":"relative/path"},"REJECT - u: expected format uri"]
["formats {\"u\":\"https://x.y/z\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"u":"https://x.y/z"},"ok {\"u\":\"https://x.y/z\"}"]
["formats {\"d\":\"2024-01-01T00:00:00Z\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"d":"2024-01-01T00:00:00Z"},"ok {\"d\":\"2024-01-01T00:00:00Z\"}"]
["formats {\"d\":\"2024-01-01\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"d":"2024-01-01"},"REJECT - d: expected format date-time"]
["formats {\"x\":\"anything\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"x":"anything"},"ok {\"x\":\"anything\"}"]
["formats {\"h\":\"under_score.example\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"h":"under_score.example"},"REJECT - h: expected format hostname"]
["formats {\"r\":\"[\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"r":"["},"REJECT - r: expected format regex"]
["formats {\"i\":\"00000000-0000-0000-0000-000000000000\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"i":"00000000-0000-0000-0000-000000000000"},"ok {\"i\":\"00000000-0000-0000-0000-000000000000\"}"]
["formats {\"i\":\"not\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"i":"not"},"REJECT - i: expected format uuid"]
["formats {\"ip\":\"999.1.1.1\"}","formats",{"type":"object","properties":{"e":{"type":"string","format":"email"},"u":{"type":"string","format":"uri"},"d":{"type":"string","format":"date-time"},"x":{"type":"string","format":"made-up"},"h":{"type":"string","format":"hostname"},"r":{"type":"string","format":"regex"},"i":{"type":"string","format":"uuid"},"ip":{"type":"string","format":"ipv4"}}},{"ip":"999.1.1.1"},"REJECT - ip: expected format ipv4"]
["integer-big {\"n\":1e+21}","integer-big",{"type":"object","required":["n"],"properties":{"n":{"type":"integer"}}},{"n":1e+21},"ok {\"n\":1e+21}"]
["integer-big {\"n\":9007199254740994}","integer-big",{"type":"object","required":["n"],"properties":{"n":{"type":"integer"}}},{"n":9007199254740994},"ok {\"n\":9007199254740994}"]
["additional-false {\"a\":\"x\",\"b\":1}","additional-false",{"type":"object","required":["a"],"properties":{"a":{"type":"string"}},"additionalProperties":false},{"a":"x","b":1},"REJECT - b: unknown field"]
["prototype-key {}","prototype-key",{"type":"object","properties":{"constructor":{"type":"string"},"toString":{"type":"number"}}},{},"ok {}"]
["prototype-key {\"constructor\":1}","prototype-key",{"type":"object","properties":{"constructor":{"type":"string"},"toString":{"type":"number"}}},{"constructor":1},"ok {\"constructor\":\"1\"}"]
["proto-key-value {\"__proto__\":\"x\",\"a\":\"y\"}","proto-key-value",{"type":"object","properties":{},"additionalProperties":{"type":"array","items":{"type":"string"}}},{"__proto__":"x","a":"y"},"ok {\"__proto__\":[\"x\"],\"a\":[\"y\"]}"]
["mcp-draft7-ref-definitions {\"t\":{\"path\":\"a\"}}","mcp-draft7-ref-definitions",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"t":{"$ref":"#/definitions/T"}},"required":["t"],"definitions":{"T":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},{"t":{"path":"a"}},"ok {\"t\":{\"path\":\"a\"}}"]
["mcp-draft7-ref-definitions {\"t\":{\"path\":42}}","mcp-draft7-ref-definitions",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"t":{"$ref":"#/definitions/T"}},"required":["t"],"definitions":{"T":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},{"t":{"path":42}},"ok {\"t\":{\"path\":\"42\"}}"]
["mcp-ref-unresolved {\"t\":1}","mcp-ref-unresolved",{"type":"object","properties":{"t":{"$ref":"#/definitions/Missing"}}},{"t":1},"REJECT - t: unresolved reference #/definitions/Missing"]
["mcp-ref-unresolved {}","mcp-ref-unresolved",{"type":"object","properties":{"t":{"$ref":"#/definitions/Missing"}}},{},"ok {}"]
["mcp-ref-external {\"t\":1}","mcp-ref-external",{"type":"object","properties":{"t":{"$ref":"https://example.com/schemas/t.json"}}},{"t":1},"REJECT - t: unresolved reference https://example.com/schemas/t.json"]
["mcp-nullable-openapi {\"s\":null}","mcp-nullable-openapi",{"type":"object","properties":{"s":{"type":"string","nullable":true}},"required":["s"]},{"s":null},"ok {\"s\":\"null\"}"]
["mcp-nullable-openapi {\"s\":\"x\"}","mcp-nullable-openapi",{"type":"object","properties":{"s":{"type":"string","nullable":true}},"required":["s"]},{"s":"x"},"ok {\"s\":\"x\"}"]
["mcp-draft7-tuple {\"p\":[1,\"a\"]}","mcp-draft7-tuple",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"p":{"type":"array","items":[{"type":"number"},{"type":"string"}],"additionalItems":false}}},{"p":[1,"a"]},"ok {\"p\":[1,\"a\"]}"]
["mcp-draft7-tuple {\"p\":[1,\"a\",3]}","mcp-draft7-tuple",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"p":{"type":"array","items":[{"type":"number"},{"type":"string"}],"additionalItems":false}}},{"p":[1,"a",3]},"REJECT - p.2: no value is allowed here"]
["mcp-draft7-tuple {\"p\":[\"1\",2]}","mcp-draft7-tuple",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"p":{"type":"array","items":[{"type":"number"},{"type":"string"}],"additionalItems":false}}},{"p":["1",2]},"ok {\"p\":[1,\"2\"]}"]
["mcp-draft7-ref-sibling {\"s\":\"abc\"}","mcp-draft7-ref-sibling",{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"s":{"$ref":"#/definitions/S","maxLength":1}},"definitions":{"S":{"type":"string"}}},{"s":"abc"},"REJECT - s: expected at most 1 characters"]
["mcp-draft4-exclusive-bool {\"n\":0}","mcp-draft4-exclusive-bool",{"$schema":"http://json-schema.org/draft-04/schema#","type":"object","properties":{"n":{"type":"number","minimum":0,"exclusiveMinimum":true}}},{"n":0},"ok {\"n\":0}"]
["mcp-draft4-exclusive-bool {\"n\":1}","mcp-draft4-exclusive-bool",{"$schema":"http://json-schema.org/draft-04/schema#","type":"object","properties":{"n":{"type":"number","minimum":0,"exclusiveMinimum":true}}},{"n":1},"ok {\"n\":1}"]
["mcp-id-draft4 {\"a\":\"x\"}","mcp-id-draft4",{"$schema":"http://json-schema.org/draft-04/schema#","id":"http://x/y","type":"object","properties":{"a":{"type":"string"}}},{"a":"x"},"ok {\"a\":\"x\"}"]
["mcp-bad-pattern {\"s\":\"y\"}","mcp-bad-pattern",{"type":"object","properties":{"s":{"type":"string","pattern":"(?<!x)y"}}},{"s":"y"},"ok {\"s\":\"y\"}"]
["mcp-invalid-regex {\"s\":\"y\"}","mcp-invalid-regex",{"type":"object","properties":{"s":{"type":"string","pattern":"["}}},{"s":"y"},"THROW Invalid regular expression: /[/: Unterminated character class"]
["mcp-type-integer-float-0 {\"n\":1}","mcp-type-integer-float-0",{"type":"object","properties":{"n":{"type":"integer"}}},{"n":1},"ok {\"n\":1}"]
["mcp-type-integer-float-0 {\"n\":1.5}","mcp-type-integer-float-0",{"type":"object","properties":{"n":{"type":"integer"}}},{"n":1.5},"ok {\"n\":1}"]
["mcp-format-unknown {\"s\":\"xx\"}","mcp-format-unknown",{"type":"object","properties":{"s":{"type":"string","format":"binary"}}},{"s":"xx"},"ok {\"s\":\"xx\"}"]
["mcp-required-null-string {\"q\":null}","mcp-required-null-string",{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]},{"q":null},"ok {\"q\":\"null\"}"]
["mcp-array-json-string {\"p\":\"[\\\"a\\\",\\\"b\\\"]\"}","mcp-array-json-string",{"type":"object","properties":{"p":{"type":"array","items":{"type":"string"}}},"required":["p"]},{"p":"[\"a\",\"b\"]"},"ok {\"p\":[\"[\\\"a\\\",\\\"b\\\"]\"]}"]
["mcp-integer-float {\"n\":3.7}","mcp-integer-float",{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]},{"n":3.7},"ok {\"n\":3}"]
["mcp-integer-float {\"n\":\"3.7\"}","mcp-integer-float",{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]},{"n":"3.7"},"ok {\"n\":3}"]
["mcp-empty-schema {\"a\":1}","mcp-empty-schema",{},{"a":1},"ok {\"a\":1}"]
["mcp-type-object-no-props {\"a\":1}","mcp-type-object-no-props",{"type":"object"},{"a":1},"ok {\"a\":1}"]
["mcp-boolean-true-prop {\"a\":1}","mcp-boolean-true-prop",{"type":"object","properties":{"a":true,"b":false}},{"a":1},"ok {\"a\":1}"]
["mcp-boolean-true-prop {\"b\":1}","mcp-boolean-true-prop",{"type":"object","properties":{"a":true,"b":false}},{"b":1},"REJECT - b: no value is allowed here"]
["mcp-huge-enum {\"e\":\"v4999\"}","mcp-huge-enum",{"type":"object","properties":{"e":{"type":"string","enum":"HUGE_ENUM"}}},{"e":"v4999"},"ok {\"e\":\"v4999\"}"]
["mcp-huge-enum {\"e\":\"v5000\"}","mcp-huge-enum",{"type":"object","properties":{"e":{"type":"string","enum":"HUGE_ENUM"}}},{"e":"v5000"},"REJECT - e: expected one of [\"v0\",\"v1\",\"v2\",\"v3\",\"v4\",\"v5\",\"v6\",\"v7\",\"v8\",\"v9\",\"v10\",\"v11\",\"v12\",\"v13\",\"v14\",\"v15\",\"v16\",\"v17\",\"v18\",\"v19\", ... and 4980 more]"]
["oneOf-overlap {\"v\":\"abc\"}","oneOf-overlap",{"type":"object","properties":{"v":{"oneOf":[{"type":"string"},{"type":"string","maxLength":5}]}}},{"v":"abc"},"REJECT - v: matches more than one alternative"]
["oneOf-overlap {\"v\":\"abcdefgh\"}","oneOf-overlap",{"type":"object","properties":{"v":{"oneOf":[{"type":"string"},{"type":"string","maxLength":5}]}}},{"v":"abcdefgh"},"ok {\"v\":\"abcdefgh\"}"]
["oneOf-number-integer {\"v\":2}","oneOf-number-integer",{"type":"object","properties":{"v":{"oneOf":[{"type":"number"},{"type":"integer"}]}}},{"v":2},"REJECT - v: matches more than one alternative"]
["oneOf-number-integer {\"v\":2.5}","oneOf-number-integer",{"type":"object","properties":{"v":{"oneOf":[{"type":"number"},{"type":"integer"}]}}},{"v":2.5},"ok {\"v\":2.5}"]
["oneOf-objects-open {\"v\":{\"a\":\"x\"}}","oneOf-objects-open",{"type":"object","properties":{"v":{"oneOf":[{"type":"object","properties":{"a":{"type":"string"}}},{"type":"object","properties":{"b":{"type":"string"}}}]}}},{"v":{"a":"x"}},"REJECT - v: matches more than one alternative"]
["oneOf-objects-open {\"v\":{}}","oneOf-objects-open",{"type":"object","properties":{"v":{"oneOf":[{"type":"object","properties":{"a":{"type":"string"}}},{"type":"object","properties":{"b":{"type":"string"}}}]}}},{"v":{}},"REJECT - v: matches more than one alternative"]
["if-then-else {\"kind\":\"a\"}","if-then-else",{"type":"object","properties":{"kind":{"type":"string"},"n":{"type":"number"}},"if":{"properties":{"kind":{"const":"a"}}},"then":{"required":["n"]}},{"kind":"a"},"REJECT - n: missing"]
["if-then-else {\"kind\":\"b\"}","if-then-else",{"type":"object","properties":{"kind":{"type":"string"},"n":{"type":"number"}},"if":{"properties":{"kind":{"const":"a"}}},"then":{"required":["n"]}},{"kind":"b"},"ok {\"kind\":\"b\"}"]
["dependentRequired {\"a\":\"x\"}","dependentRequired",{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"dependentRequired":{"a":["b"]}},{"a":"x"},"REJECT - b: required when a is present"]
["dependencies-d7 {\"a\":\"x\"}","dependencies-d7",{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"dependencies":{"a":["b"]}},{"a":"x"},"REJECT - b: required when a is present"]
["propertyNames {\"abc\":1}","propertyNames",{"type":"object","properties":{},"propertyNames":{"pattern":"^[a-z]+$"}},{"abc":1},"ok {\"abc\":1}"]
["propertyNames {\"ABC\":1}","propertyNames",{"type":"object","properties":{},"propertyNames":{"pattern":"^[a-z]+$"}},{"ABC":1},"REJECT - ABC: invalid property name"]
["contains {\"l\":[\"a\"]}","contains",{"type":"object","properties":{"l":{"type":"array","contains":{"type":"number"}}}},{"l":["a"]},"REJECT - l: expected at least 1 matching item"]
["contains {\"l\":[\"a\",1]}","contains",{"type":"object","properties":{"l":{"type":"array","contains":{"type":"number"}}}},{"l":["a",1]},"ok {\"l\":[\"a\",1]}"]
["not {\"s\":\"x\"}","not",{"type":"object","properties":{"s":{"not":{"type":"null"}}}},{"s":"x"},"ok {\"s\":\"x\"}"]
["not {\"s\":null}","not",{"type":"object","properties":{"s":{"not":{"type":"null"}}}},{"s":null},"ok {}"]
["minProperties {\"m\":{}}","minProperties",{"type":"object","properties":{"m":{"type":"object","minProperties":1}}},{"m":{}},"REJECT - m: expected at least 1 properties"]
["unevaluatedProperties {\"a\":\"x\"}","unevaluatedProperties",{"type":"object","properties":{"a":{"type":"string"}},"unevaluatedProperties":false},{"a":"x"},"ok {\"a\":\"x\"}"]
["unevaluatedProperties {\"a\":\"x\",\"b\":1}","unevaluatedProperties",{"type":"object","properties":{"a":{"type":"string"}},"unevaluatedProperties":false},{"a":"x","b":1},"REJECT - b: unknown field"]
["allOf-additional-false {\"a\":\"x\"}","allOf-additional-false",{"allOf":[{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false},{"type":"object","properties":{"b":{"type":"string"}}}]},{"a":"x"},"ok {\"a\":\"x\"}"]
["allOf-additional-false {\"a\":\"x\",\"b\":\"y\"}","allOf-additional-false",{"allOf":[{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false},{"type":"object","properties":{"b":{"type":"string"}}}]},{"a":"x","b":"y"},"REJECT - b: unknown field"]
["const-object {\"c\":{\"y\":2,\"x\":1}}","const-object",{"type":"object","properties":{"c":{"const":{"x":1,"y":2}}}},{"c":{"y":2,"x":1}},"ok {\"c\":{\"y\":2,\"x\":1}}"]
["enum-object {\"e\":{\"y\":2,\"x\":1}}","enum-object",{"type":"object","properties":{"e":{"enum":[{"x":1,"y":2}]}}},{"e":{"y":2,"x":1}},"ok {\"e\":{\"y\":2,\"x\":1}}"]
["type-array-multi {\"v\":true}","type-array-multi",{"type":"object","properties":{"v":{"type":["string","number"]}}},{"v":true},"ok {\"v\":\"true\"}"]
["type-array-multi {\"v\":null}","type-array-multi",{"type":"object","properties":{"v":{"type":["string","number"]}}},{"v":null},"ok {}"]
["nullable-type-array {\"v\":null}","nullable-type-array",{"type":"object","properties":{"v":{"type":["string","null"]}},"required":["v"]},{"v":null},"ok {\"v\":null}"]
["nullable-type-array {\"v\":5}","nullable-type-array",{"type":"object","properties":{"v":{"type":["string","null"]}},"required":["v"]},{"v":5},"ok {\"v\":\"5\"}"]
["exclusiveMinimum-number {\"n\":0}","exclusiveMinimum-number",{"type":"object","properties":{"n":{"type":"number","exclusiveMinimum":0}}},{"n":0},"REJECT - n: expected > 0"]
["exclusiveMinimum-number {\"n\":\"0\"}","exclusiveMinimum-number",{"type":"object","properties":{"n":{"type":"number","exclusiveMinimum":0}}},{"n":"0"},"REJECT - n: expected > 0"]
["maxItems {\"l\":[1,2]}","maxItems",{"type":"object","properties":{"l":{"type":"array","maxItems":1}}},{"l":[1,2]},"REJECT - l: expected at most 1 items"]
["maxItems {\"l\":\"x\"}","maxItems",{"type":"object","properties":{"l":{"type":"array","maxItems":1}}},{"l":"x"},"ok {\"l\":[\"x\"]}"]
["discriminator-openapi {\"p\":{\"kind\":\"a\",\"x\":\"1\"}}","discriminator-openapi",{"type":"object","properties":{"p":{"oneOf":[{"type":"object","properties":{"kind":{"type":"string","enum":["a"]},"x":{"type":"number"}},"required":["kind"]},{"type":"object","properties":{"kind":{"type":"string","enum":["b"]}},"required":["kind"]}],"discriminator":{"propertyName":"kind"}}}},{"p":{"kind":"a","x":"1"}},"ok {\"p\":{\"kind\":\"a\",\"x\":1}}"]
["discriminator-openapi {\"p\":{\"kind\":\"b\"}}","discriminator-openapi",{"type":"object","properties":{"p":{"oneOf":[{"type":"object","properties":{"kind":{"type":"string","enum":["a"]},"x":{"type":"number"}},"required":["kind"]},{"type":"object","properties":{"kind":{"type":"string","enum":["b"]}},"required":["kind"]}],"discriminator":{"propertyName":"kind"}}}},{"p":{"kind":"b"}},"ok {\"p\":{\"kind\":\"b\"}}"]
["ref-root-defs {\"a\":\"x\"}","ref-root-defs",{"$ref":"#/$defs/Root","$defs":{"Root":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}}},{"a":"x"},"ok {\"a\":\"x\"}"]
["ref-root-defs {\"a\":1}","ref-root-defs",{"$ref":"#/$defs/Root","$defs":{"Root":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}}},{"a":1},"ok {\"a\":\"1\"}"]
["ref-root-defs {}","ref-root-defs",{"$ref":"#/$defs/Root","$defs":{"Root":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}}},{},"REJECT - a: missing"]
["ref-recursive {\"root\":{\"v\":\"2\",\"c\":[{\"v\":\"3\",\"c\":[]}]}}","ref-recursive",{"$defs":{"N":{"type":"object","properties":{"c":{"type":"array","items":{"$ref":"#/$defs/N"}},"v":{"type":"integer"}}}},"type":"object","properties":{"root":{"$ref":"#/$defs/N"}}},{"root":{"v":"2","c":[{"v":"3","c":[]}]}},"ok {\"root\":{\"v\":2,\"c\":[{\"v\":3,\"c\":[]}]}}"]
["ref-percent {\"a\":\"x\"}","ref-percent",{"type":"object","properties":{"a":{"$ref":"#/$defs/a%20b"}},"$defs":{"a b":{"type":"string"}}},{"a":"x"},"ok {\"a\":\"x\"}"]
["ref-percent {\"a\":1}","ref-percent",{"type":"object","properties":{"a":{"$ref":"#/$defs/a%20b"}},"$defs":{"a b":{"type":"string"}}},{"a":1},"ok {\"a\":\"1\"}"]
["ref-tilde {\"a\":\"x\"}","ref-tilde",{"type":"object","properties":{"a":{"$ref":"#/$defs/a~1b"}},"$defs":{"a/b":{"type":"string"}}},{"a":"x"},"ok {\"a\":\"x\"}"]
["defs-with-$id {\"a\":\"x\"}","defs-with-$id",{"type":"object","properties":{"a":{"$ref":"item.json"}},"$defs":{"item":{"$id":"item.json","type":"string"}}},{"a":"x"},"ok {\"a\":\"x\"}"]
["defs-with-$id {\"a\":2}","defs-with-$id",{"type":"object","properties":{"a":{"$ref":"item.json"}},"$defs":{"item":{"$id":"item.json","type":"string"}}},{"a":2},"ok {\"a\":\"2\"}"]
["anyOf-null-default {\"v\":null}","anyOf-null-default",{"type":"object","properties":{"v":{"anyOf":[{"type":"string"},{"type":"null"}],"default":null}}},{"v":null},"ok {\"v\":null}"]
["anyOf-null-default {}","anyOf-null-default",{"type":"object","properties":{"v":{"anyOf":[{"type":"string"},{"type":"null"}],"default":null}}},{},"ok {}"]
["format-date-time-lower {\"d\":\"2024-01-01t00:00:00z\"}","format-date-time-lower",{"type":"object","properties":{"d":{"type":"string","format":"date-time"}}},{"d":"2024-01-01t00:00:00z"},"ok {\"d\":\"2024-01-01t00:00:00z\"}"]
["format-date-time-lower {\"d\":\"2024-01-01 00:00:00Z\"}","format-date-time-lower",{"type":"object","properties":{"d":{"type":"string","format":"date-time"}}},{"d":"2024-01-01 00:00:00Z"},"REJECT - d: expected format date-time"]
["format-url {\"u\":\"not a url\"}","format-url",{"type":"object","properties":{"u":{"type":"string","format":"url"}}},{"u":"not a url"},"REJECT - u: expected format url"]
["integer-as-float-1e0 {\"n\":1000}","integer-as-float-1e0",{"type":"object","properties":{"n":{"type":"integer"}}},{"n":1000},"ok {\"n\":1000}"]
["required-missing-props {}","required-missing-props",{"type":"object","required":["a"]},{},"REJECT - a: missing"]
["required-missing-props {\"a\":null}","required-missing-props",{"type":"object","required":["a"]},{"a":null},"ok {\"a\":null}"]
["string-to-number-trim {\"n\":\" 12 \"}","string-to-number-trim",{"type":"object","properties":{"n":{"type":"number"}}},{"n":" 12 "},"ok {\"n\":12}"]
["string-to-number-trim {\"n\":\"0x10\"}","string-to-number-trim",{"type":"object","properties":{"n":{"type":"number"}}},{"n":"0x10"},"ok {\"n\":16}"]
["string-to-number-trim {\"n\":\"1e3\"}","string-to-number-trim",{"type":"object","properties":{"n":{"type":"number"}}},{"n":"1e3"},"ok {\"n\":1000}"]
["string-to-number-trim {\"n\":\"\"}","string-to-number-trim",{"type":"object","properties":{"n":{"type":"number"}}},{"n":""},"ok {\"n\":0}"]
["tree valid","tree valid",{"type":"object","properties":{"t":{"$ref":"#/$defs/Tree"}},"$defs":{"Tree":{"anyOf":[{"type":"string"},{"type":"array","items":{"$ref":"#/$defs/Tree"}}]}}},{"t":["a",["b"]]},"ok {\"t\":[\"a\",[\"b\"]]}"]
["tree number","tree number",{"type":"object","properties":{"t":{"$ref":"#/$defs/Tree"}},"$defs":{"Tree":{"anyOf":[{"type":"string"},{"type":"array","items":{"$ref":"#/$defs/Tree"}}]}}},{"t":5},"ok {\"t\":\"5\"}"]
["tree [5]","tree [5]",{"type":"object","properties":{"t":{"$ref":"#/$defs/Tree"}},"$defs":{"Tree":{"anyOf":[{"type":"string"},{"type":"array","items":{"$ref":"#/$defs/Tree"}}]}}},{"t":[5]},"ok {\"t\":[\"5\"]}"]
["json valid","json valid",{"type":"object","properties":{"data":{"$ref":"#/$defs/Json"}},"required":["data"],"$defs":{"Json":{"anyOf":[{"type":"string"},{"type":"number"},{"type":"boolean"},{"type":"array","items":{"$ref":"#/$defs/Json"}},{"type":"object","additionalProperties":{"$ref":"#/$defs/Json"}}]}}},{"data":{"a":[1,"x",{"b":true}]}},"ok {\"data\":{\"a\":[1,\"x\",{\"b\":true}]}}"]
["json null","json null",{"type":"object","properties":{"data":{"$ref":"#/$defs/Json"}},"required":["data"],"$defs":{"Json":{"anyOf":[{"type":"string"},{"type":"number"},{"type":"boolean"},{"type":"array","items":{"$ref":"#/$defs/Json"}},{"type":"object","additionalProperties":{"$ref":"#/$defs/Json"}}]}}},{"data":null},"ok {\"data\":\"null\"}"]
["json nested null","json nested null",{"type":"object","properties":{"data":{"$ref":"#/$defs/Json"}},"required":["data"],"$defs":{"Json":{"anyOf":[{"type":"string"},{"type":"number"},{"type":"boolean"},{"type":"array","items":{"$ref":"#/$defs/Json"}},{"type":"object","additionalProperties":{"$ref":"#/$defs/Json"}}]}}},{"data":{"a":[1,null]}},"ok {\"data\":{\"a\":[1,\"null\"]}}"]
["filter valid","filter valid",{"type":"object","properties":{"where":{"$ref":"#/$defs/F"}},"$defs":{"F":{"oneOf":[{"type":"object","properties":{"field":{"type":"string"},"eq":{"type":"string"}},"required":["field","eq"],"additionalProperties":false},{"type":"object","properties":{"and":{"type":"array","items":{"$ref":"#/$defs/F"}}},"required":["and"],"additionalProperties":false}]}}},{"where":{"and":[{"field":"a","eq":"b"}]}},"ok {\"where\":{\"and\":[{\"field\":\"a\",\"eq\":\"b\"}]}}"]
["filter bad leaf","filter bad leaf",{"type":"object","properties":{"where":{"$ref":"#/$defs/F"}},"$defs":{"F":{"oneOf":[{"type":"object","properties":{"field":{"type":"string"},"eq":{"type":"string"}},"required":["field","eq"],"additionalProperties":false},{"type":"object","properties":{"and":{"type":"array","items":{"$ref":"#/$defs/F"}}},"required":["and"],"additionalProperties":false}]}}},{"where":{"and":[{"field":"a","eq":1,"x":2}]}},"REJECT - where.and.0.eq: expected string, got integer"]
["filter deep bad","filter deep bad",{"type":"object","properties":{"where":{"$ref":"#/$defs/F"}},"$defs":{"F":{"oneOf":[{"type":"object","properties":{"field":{"type":"string"},"eq":{"type":"string"}},"required":["field","eq"],"additionalProperties":false},{"type":"object","properties":{"and":{"type":"array","items":{"$ref":"#/$defs/F"}}},"required":["and"],"additionalProperties":false}]}}},{"where":{"and":[{"and":[{"and":[{"and":[{"and":[{"field":1}]}]}]}]}]}},"REJECT - where.and.0.and.0.and.0.and.0.and.0.eq: missing"]
"###;

const SOL_PROBES: &str = r###"
["BigInt from a JSON string","BigInt from a JSON string",{"type":"object","required":["n"],"properties":{"n":{"type":"bigint"}}},{"n":"42"},"REJECT - n: expected bigint, got string"]
["Undefined from JSON null","Undefined from JSON null",{"type":"object","required":["n"],"properties":{"n":{"type":"undefined"}}},{"n":null},"REJECT - n: expected undefined, got null"]
["additionalProperties on declared properties","additionalProperties on declared properties",{"type":"object","required":["amount","label"],"properties":{"amount":{"type":"number"},"label":{"type":"string"}},"additionalProperties":{"type":"integer"}},{"amount":3.5,"label":"7"},"ok {\"amount\":3.5,\"label\":\"7\"}"]
["property names are not patterns","property names are not patterns",{"type":"object","required":["a.b","axb"],"properties":{"a.b":{"type":"integer"},"axb":{"type":"string"}}},{"a.b":"4","axb":"5.8"},"ok {\"a.b\":4,\"axb\":\"5.8\"}"]
["Refine is not run","Refine is not run",{"type":"object","required":["mode"],"properties":{"mode":{"type":"string"}}},{"mode":"unsafe"},"ok {\"mode\":\"unsafe\"}"]
["scalar-or-array that refers to itself","scalar-or-array that refers to itself",{"type":"object","properties":{"value":{"anyOf":[{"type":"number"},{"type":"array","items":{"$ref":"#/properties/value"}}]}},"required":["value"]},{"value":"42"},"ok {\"value\":42}"]
["StringEnum with an empty member from null","StringEnum with an empty member from null",{"type":"object","required":["mode"],"properties":{"mode":{"type":"string","enum":["","x"]}}},{"mode":null},"ok {\"mode\":\"\"}"]
["Cyclic with named local definitions","Cyclic with named local definitions",{"type":"object","required":["node"],"properties":{"node":{"$defs":{"Node":{"type":"object","required":["n"],"properties":{"n":{"type":"number"},"next":{"$ref":"Node"}},"$id":"Node"}},"$ref":"Node"}}},{"node":{"n":1}},"ok {\"node\":{\"n\":1}}"]
["integer from a string","integer from a string",{"type":"object","required":["n"],"properties":{"n":{"type":"integer"}}},{"n":"42"},"ok {\"n\":42}"]
["recursive union with a value no branch converts","recursive union with a value no branch converts",{"type":"object","properties":{"t":{"$ref":"#/$defs/Tree"}},"$defs":{"Tree":{"anyOf":[{"type":"string"},{"type":"array","items":{"$ref":"#/$defs/Tree"}}]}}},{"t":{}},"REJECT - t: expected string, got object"]
"###;

mod reviewer_probes {
	use super::*;

	#[test]
	fn reviewer_probes() {
		run_table(REVIEWER_PROBES);
	}
}

mod reviewer_probes_sol {
	use super::*;

	#[test]
	fn reviewer_probes_sol() {
		run_table(SOL_PROBES);
	}

	#[test]
	fn accepts_the_last_member_of_a_10_000_value_enum() {
		let values: Vec<String> = (0..10_000).map(|i| format!("member{i}")).collect();
		let schema = json!({ "type": "object", "properties": { "choice": { "type": "string", "enum": values } }, "required": ["choice"] });
		assert_eq!(
			outcome("enum", &schema, &json!({ "choice": "member9999" })),
			r#"ok {"choice":"member9999"}"#
		);
		let first = json_stringify(&values[..20]);
		assert_eq!(
			outcome("enum", &schema, &json!({ "choice": "x" })),
			format!(
				"REJECT - choice: expected one of {}, ... and 9980 more]",
				&first[..first.len() - 1]
			)
		);
	}

	#[test]
	fn keeps_the_validator_of_a_schema_object_as_before() {
		let parameters =
			json!({ "type": "object", "required": ["n"], "properties": { "n": { "type": "number", "minimum": 0 } } });
		assert_eq!(outcome("late", &parameters, &json!({ "n": "4" })), r#"ok {"n":4}"#);
	}

	#[test]
	#[ignore = "the Rust validator cache is keyed by the schema's JSON, so a changed schema is compiled anew"]
	fn keeps_the_validator_of_a_schema_object_as_before_also_when_the_schema_is_changed_later() {
		let mut parameters =
			json!({ "type": "object", "required": ["n"], "properties": { "n": { "type": "number", "minimum": 0 } } });
		assert_eq!(outcome("late", &parameters, &json!({ "n": "4" })), r#"ok {"n":4}"#);
		parameters["properties"]["n"]["minimum"] = json!(10);
		assert_eq!(outcome("late", &parameters, &json!({ "n": "4" })), r#"ok {"n":4}"#);
	}
}

mod reviewer_probes_round_2 {
	use super::*;

	#[test]
	fn drops_an_optional_null_inside_name_property_values() {
		let item = json!({ "type": "object", "properties": { "n": { "type": "integer", "minimum": 1 } } });
		let nested = [
			("declared", json!({ "type": "object", "properties": { "entry": item } })),
			("additional", json!({ "type": "object", "additionalProperties": item })),
			(
				"pattern",
				json!({ "type": "object", "patternProperties": { "^entry$": item }, "additionalProperties": false }),
			),
		];
		for (name, schema) in nested {
			assert_eq!(
				outcome(name, &schema, &json!({ "entry": { "n": null } })),
				r#"ok {"entry":{}}"#,
				"drops an optional null inside {name} property values"
			);
		}
	}

	#[test]
	fn converts_a_keyword_branch_together_with_the_nodes_sibling_properties() {
		for keyword in ["anyOf", "oneOf"] {
			let mut schema = json!({ "type": "object" });
			schema[keyword] = json!([
				{ "type": "object", "properties": { "a": { "type": "array", "items": { "type": "integer" } } }, "required": ["a"] },
				{ "type": "object", "properties": { "b": { "type": "boolean" } }, "required": ["b"] },
			]);
			schema["properties"] = json!({ "c": { "type": "integer" } });
			schema["required"] = json!(["c"]);
			assert_eq!(
				outcome(keyword, &schema, &json!({ "a": "3", "c": "2" })),
				r#"ok {"a":[3],"c":2}"#,
				"converts a {keyword} branch together with the node's sibling properties"
			);
		}
	}

	#[test]
	fn rejects_a_deep_value_every_branch_of_a_recursive_object_union_rejects_without_exponential_work() {
		let s = json!({
			"anyOf": [
				{ "type": "object", "additionalProperties": { "$ref": "#/$defs/S" }, "required": ["a"] },
				{ "type": "object", "additionalProperties": { "$ref": "#/$defs/S" }, "required": ["b"] },
			],
		});
		let schema = json!({ "type": "object", "properties": { "s": { "$ref": "#/$defs/S" } }, "$defs": { "S": s } });
		let mut value = json!({ "x": 1 });
		for _ in 0..40 {
			value = json!({ "c": value });
		}
		assert_eq!(
			outcome("deep", &schema, &json!({ "s": value })),
			"REJECT - s.a: missing"
		);
	}
}
