//! Port of `packages/ai/src/utils/headers.ts`.
//! PORT-STATUS: ported
//!
//! `headersToRecord` takes the header map of a `pi_host::HttpResponse` and iterates it the way a `Headers` object
//! does: lowercase names in sorted order.

use indexmap::IndexMap;

use crate::types::ProviderHeaders;

pub fn headers_to_record(headers: &IndexMap<String, String>) -> IndexMap<String, String> {
	let mut result: Vec<(String, String)> = headers
		.iter()
		.map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
		.collect();
	result.sort_by(|(a, _), (b, _)| a.cmp(b));
	result.into_iter().collect()
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
	Some(merged.into_values().collect())
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
	}
}
