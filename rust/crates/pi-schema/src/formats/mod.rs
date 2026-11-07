//! Port of `packages/schema/src/formats/index.ts`.
//! PORT-STATUS: ported
//!
//! The format checks of JSON Schema 2020-12, for compile()'s `formats` option. An optional entry point: schemas
//! compiled without it treat `format` as an annotation and do not bundle these checks.

// <generated-modules>
pub mod typebox;
// </generated-modules>

use std::rc::Rc;

use indexmap::IndexMap;

use crate::types::FormatCheck;
use typebox::date::is_date;
use typebox::date_time::is_date_time;
use typebox::duration::is_duration;
use typebox::email::is_email;
use typebox::hostname::is_hostname;
use typebox::idn_email::is_idn_email;
use typebox::idn_hostname::is_idn_hostname;
use typebox::ipv4::is_ipv4;
use typebox::ipv6::is_ipv6;
use typebox::iri::is_iri;
use typebox::iri_reference::is_iri_reference;
use typebox::json_pointer::is_json_pointer;
use typebox::json_pointer_uri_fragment::is_json_pointer_uri_fragment;
use typebox::regex::is_regex;
use typebox::relative_json_pointer::is_relative_json_pointer;
use typebox::time::is_time;
use typebox::uri::is_uri;
use typebox::uri_reference::is_uri_reference;
use typebox::uri_template::is_uri_template;
use typebox::url::is_url;
use typebox::uuid::is_uuid;

/// Every format JSON Schema 2020-12 defines, by name, plus `url` and `json-pointer-uri-fragment`, which TypeBox and
/// ajv-formats also check. The checks are adapted from TypeBox 1.3.27 (MIT).
pub fn formats() -> Rc<IndexMap<String, FormatCheck>> {
	FORMATS.with(Rc::clone)
}

thread_local! {
	static FORMATS: Rc<IndexMap<String, FormatCheck>> = {
		let entries: [(&str, FormatCheck); 21] = [
			("date", Rc::new(is_date)),
			("date-time", Rc::new(is_date_time)),
			("duration", Rc::new(is_duration)),
			("email", Rc::new(is_email)),
			("hostname", Rc::new(is_hostname)),
			("idn-email", Rc::new(is_idn_email)),
			("idn-hostname", Rc::new(is_idn_hostname)),
			("ipv4", Rc::new(is_ipv4)),
			("ipv6", Rc::new(is_ipv6)),
			("iri", Rc::new(is_iri)),
			("iri-reference", Rc::new(is_iri_reference)),
			("json-pointer", Rc::new(is_json_pointer)),
			("json-pointer-uri-fragment", Rc::new(is_json_pointer_uri_fragment)),
			("regex", Rc::new(is_regex)),
			("relative-json-pointer", Rc::new(is_relative_json_pointer)),
			("time", Rc::new(|value: &str| is_time(value, None))),
			("uri", Rc::new(is_uri)),
			("uri-reference", Rc::new(is_uri_reference)),
			("uri-template", Rc::new(is_uri_template)),
			("url", Rc::new(is_url)),
			("uuid", Rc::new(is_uuid)),
		];
		Rc::new(entries.into_iter().map(|(name, check)| (name.to_string(), check)).collect())
	};
}
