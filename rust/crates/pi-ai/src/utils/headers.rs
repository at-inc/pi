//! Port of `packages/ai/src/utils/headers.ts`.
//! PORT-STATUS: reviewed
//!
//! `headersToRecord` takes the header map of a `pi_host::HttpResponse` and reads it the way a `Headers` object built
//! from it does: names lowercase and sorted, values trimmed of HTTP whitespace, values of names that differ only in
//! case joined with `", "`, and only the last `set-cookie` kept (the record keeps the last of its separate entries).

use indexmap::IndexMap;

use super::text::js_entries;
use crate::types::ProviderHeaders;

pub fn headers_to_record(headers: &IndexMap<String, String>) -> IndexMap<String, String> {
	let mut result: IndexMap<String, String> = IndexMap::new();
	for (key, value) in headers {
		let key = key.to_ascii_lowercase();
		let value = value.trim_matches([' ', '\t', '\r', '\n']);
		match result.get_mut(&key) {
			Some(combined) if key != "set-cookie" => {
				combined.push_str(", ");
				combined.push_str(value);
			}
			_ => {
				result.insert(key, value.to_string());
			}
		}
	}
	result.sort_keys();
	result
}

pub fn provider_headers_to_record(header_sources: &[Option<&ProviderHeaders>]) -> Option<IndexMap<String, String>> {
	let mut merged: IndexMap<String, (String, String)> = IndexMap::new();
	for source in header_sources.iter().flatten() {
		for (name, value) in source.iter() {
			let normalized_name = name.to_lowercase();
			merged.shift_remove(&normalized_name);
			if let Some(value) = value {
				merged.insert(normalized_name, (name.clone(), value.clone()));
			}
		}
	}
	if merged.is_empty() {
		return None;
	}
	// `Object.fromEntries` builds a JS object, which lists array-index names first.
	let record: IndexMap<String, String> = merged.into_values().collect();
	Some(
		js_entries(&record)
			.into_iter()
			.map(|(name, value)| (name.clone(), value.clone()))
			.collect(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn iterates_like_headers_and_merges_provider_headers() {
		let headers = IndexMap::from([
			("x-b".to_string(), "2".to_string()),
			("X-A".to_string(), "1".to_string()),
		]);
		assert_eq!(
			headers_to_record(&headers).into_iter().collect::<Vec<_>>(),
			[
				("x-a".to_string(), "1".to_string()),
				("x-b".to_string(), "2".to_string())
			]
		);

		let model = ProviderHeaders::from([
			("X-Key".into(), Some("model".into())),
			("Drop".into(), Some("x".into())),
		]);
		let request = ProviderHeaders::from([("x-key".into(), Some("request".into())), ("drop".into(), None)]);
		assert_eq!(
			provider_headers_to_record(&[Some(&model), None, Some(&request)]),
			Some(IndexMap::from([("x-key".to_string(), "request".to_string())]))
		);
		assert_eq!(provider_headers_to_record(&[Some(&ProviderHeaders::new())]), None);
		let numeric = ProviderHeaders::from([("b".into(), Some("1".into())), ("123".into(), Some("2".into()))]);
		assert_eq!(
			provider_headers_to_record(&[Some(&numeric)])
				.unwrap()
				.into_iter()
				.collect::<Vec<_>>(),
			[("123".to_string(), "2".to_string()), ("b".to_string(), "1".to_string())]
		);
	}

	#[test]
	fn combines_case_variant_names_and_trims_values_like_headers() {
		let headers = IndexMap::from([
			("X-A".to_string(), "1".to_string()),
			("x-a".to_string(), " 2\t".to_string()),
			("Set-Cookie".to_string(), "a=1".to_string()),
			("set-cookie".to_string(), "b=2".to_string()),
			("b".to_string(), "  padded  ".to_string()),
		]);
		assert_eq!(
			headers_to_record(&headers).into_iter().collect::<Vec<_>>(),
			[
				("b".to_string(), "padded".to_string()),
				("set-cookie".to_string(), "b=2".to_string()),
				("x-a".to_string(), "1, 2".to_string()),
			]
		);
	}
}
