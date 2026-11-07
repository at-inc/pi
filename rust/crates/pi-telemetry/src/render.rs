//! Port of `packages/telemetry/src/render.ts`.
//! PORT-STATUS: reviewed

use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};

use crate::{SpanAttributes, SpanStatus, StoredSpan};

fn format_attributes(attributes: Option<&SpanAttributes>) -> String {
	let Some(attributes) = attributes else {
		return String::new();
	};
	attributes
		.iter()
		.map(|(key, value)| match value {
			Some(value) => format!(" {key}={}", json_stringify(value)),
			None => format!(" {key}=undefined"),
		})
		.collect()
}

/// Render stored spans as a chronological forest, treating absent parents as roots.
pub fn render_trace(spans: &[StoredSpan]) -> String {
	let mut by_id: IndexMap<&str, &StoredSpan> = IndexMap::new();
	for span in spans {
		by_id.insert(span.id.as_str(), span);
	}
	let mut roots: Vec<&StoredSpan> = Vec::new();
	let mut children: IndexMap<&str, Vec<&StoredSpan>> = IndexMap::new();
	let mut sorted: Vec<&StoredSpan> = by_id.values().copied().collect();
	sorted.sort_by(|left, right| {
		(left.started_ms - right.started_ms)
			.partial_cmp(&0.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	for span in sorted {
		match span.parent_id.as_deref() {
			Some(parent_id) if by_id.contains_key(parent_id) => children.entry(parent_id).or_default().push(span),
			_ => roots.push(span),
		}
	}

	let mut lines: Vec<String> = Vec::new();
	let mut pending: Vec<(&StoredSpan, usize)> = roots.into_iter().rev().map(|span| (span, 0)).collect();
	while let Some((span, depth)) = pending.pop() {
		let indent = "  ".repeat(depth);
		let mut error = String::new();
		if let SpanStatus::Error { error: details } = &span.status {
			error = " error".to_string();
			if let Some(details) = details {
				error += &format!(" {}: {}", details.name, details.message);
			}
		}
		lines.push(format!(
			"{indent}{}{} ({}ms){error}",
			span.name,
			format_attributes(Some(&span.attributes)),
			number_to_string(span.ended_ms - span.started_ms)
		));
		for event in &span.events {
			lines.push(format!(
				"{indent}  · {}{} (+{}ms)",
				event.name,
				format_attributes(Some(&event.attributes)),
				number_to_string(event.time_ms)
			));
		}
		for link in &span.links {
			lines.push(format!(
				"{indent}  → link trace {} span {}{}",
				link.trace_id,
				link.span_id,
				format_attributes(link.attributes.as_ref())
			));
		}
		if let Some(descendants) = children.get(span.id.as_str()) {
			for descendant in descendants.iter().rev() {
				pending.push((descendant, depth + 1));
			}
		}
	}
	lines.join("\n")
}
