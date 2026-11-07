//! Server-sent events decoding, shared by the provider APIs and MCP's streamable HTTP transport.
//!
//! Follows the WHATWG event-stream parsing rules that the provider SDKs and `eventsource-parser` implement:
//! lines split on CRLF, LF, or CR; `:` comments ignored; `data` lines joined with `\n`; an event dispatched on a
//! blank line; a leading BOM stripped.

use bytes::Bytes;
use futures::{Stream, StreamExt};

use crate::LocalBoxStream;
use crate::error::PiResult;

/// One dispatched server-sent event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SseEvent {
	/// `event:` field; `None` means the default `message` type.
	pub event: Option<String>,
	pub data: String,
	pub id: Option<String>,
	pub retry: Option<u64>,
}

/// Incremental decoder. Feed bytes with `push`, then call `finish` at end of stream.
#[derive(Default)]
pub struct SseDecoder {
	buffer: Vec<u8>,
	started: bool,
	pending_cr: bool,
	event: Option<String>,
	data: Vec<String>,
	has_data: bool,
	id: Option<String>,
	retry: Option<u64>,
}

impl SseDecoder {
	/// A decoder with empty state.
	pub fn new() -> Self {
		Self::default()
	}

	/// Feeds a chunk and returns every event it completed.
	pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
		let mut events = Vec::new();
		for &byte in chunk {
			if self.pending_cr {
				self.pending_cr = false;
				if byte == b'\n' {
					continue;
				}
			}
			match byte {
				b'\n' => self.line(&mut events),
				b'\r' => {
					self.pending_cr = true;
					self.line(&mut events);
				}
				_ => self.buffer.push(byte),
			}
		}
		events
	}

	/// Flushes a final unterminated line. A trailing event without a blank line is dispatched, matching the
	/// provider SDKs' decoders.
	pub fn finish(&mut self) -> Vec<SseEvent> {
		let mut events = Vec::new();
		if !self.buffer.is_empty() {
			self.line(&mut events);
		}
		self.dispatch(&mut events);
		events
	}

	fn line(&mut self, events: &mut Vec<SseEvent>) {
		let mut raw = std::mem::take(&mut self.buffer);
		if !self.started {
			self.started = true;
			if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
				raw.drain(..3);
			}
		}
		let line = String::from_utf8_lossy(&raw);
		if line.is_empty() {
			self.dispatch(events);
			return;
		}
		if line.starts_with(':') {
			return;
		}
		let (field, value) = match line.find(':') {
			Some(index) => {
				let value = &line[index + 1..];
				(&line[..index], value.strip_prefix(' ').unwrap_or(value))
			}
			None => (line.as_ref(), ""),
		};
		match field {
			"event" => self.event = Some(value.to_string()),
			"data" => {
				self.data.push(value.to_string());
				self.has_data = true;
			}
			"id" if !value.contains('\0') => self.id = Some(value.to_string()),
			"retry" => {
				if let Ok(retry) = value.parse() {
					self.retry = Some(retry);
				}
			}
			_ => {}
		}
	}

	fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
		let event = self.event.take();
		let retry = self.retry.take();
		if !self.has_data && event.as_deref().is_none_or(str::is_empty) {
			self.data.clear();
			return;
		}
		self.has_data = false;
		events.push(SseEvent {
			event,
			data: std::mem::take(&mut self.data).join("\n"),
			id: self.id.clone(),
			retry,
		});
	}
}

/// Decodes a byte stream into server-sent events.
pub fn decode(body: impl Stream<Item = PiResult<Bytes>> + 'static) -> LocalBoxStream<'static, PiResult<SseEvent>> {
	let mut decoder = SseDecoder::new();
	let mut body = Box::pin(body);
	let mut finished = false;
	async_stream(move |yield_| async move {
		while !finished {
			match body.next().await {
				Some(Ok(chunk)) => {
					for event in decoder.push(&chunk) {
						yield_(Ok(event)).await;
					}
				}
				Some(Err(error)) => {
					yield_(Err(error)).await;
					finished = true;
				}
				None => {
					for event in decoder.finish() {
						yield_(Ok(event)).await;
					}
					finished = true;
				}
			}
		}
	})
}

type Yield<T> = std::rc::Rc<dyn Fn(T) -> futures::future::LocalBoxFuture<'static, ()>>;

/// Minimal generator-to-stream adapter (the counterpart of an `async function*`): `body` receives a `yield_`
/// function whose future completes once the consumer has taken the item.
pub fn async_stream<T: 'static, F, Fut>(body: F) -> LocalBoxStream<'static, T>
where
	F: FnOnce(Yield<T>) -> Fut + 'static,
	Fut: std::future::Future<Output = ()> + 'static,
{
	let (sender, receiver) = futures::channel::mpsc::channel::<T>(0);
	let yield_: Yield<T> = std::rc::Rc::new(move |item| {
		let mut sender = sender.clone();
		Box::pin(async move {
			use futures::SinkExt;
			let _ = sender.send(item).await;
		})
	});
	let producer = body(yield_);
	let producer = futures::stream::once(producer).filter_map(|()| async { None::<T> });
	futures::stream::select(receiver, producer).boxed_local()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn decodes_split_chunks_and_multiline_data() {
		let mut decoder = SseDecoder::new();
		let mut events = decoder.push(b"\xEF\xBB\xBFevent: a\r\ndata: 1\r");
		assert!(events.is_empty());
		events.extend(decoder.push(b"\ndata:2\n\n: comment\ndata: tail"));
		events.extend(decoder.finish());
		assert_eq!(events.len(), 2);
		assert_eq!(events[0].event.as_deref(), Some("a"));
		assert_eq!(events[0].data, "1\n2");
		assert_eq!(events[1].data, "tail");
	}

	#[test]
	fn stream_adapter_yields_in_order() {
		let body = futures::stream::iter(vec![
			Ok(Bytes::from_static(b"data: x\n\n")),
			Ok(Bytes::from_static(b"data: y\n\n")),
		]);
		let events: Vec<_> = futures::executor::block_on(decode(body).collect::<Vec<_>>());
		let data: Vec<_> = events.into_iter().map(|event| event.unwrap().data).collect();
		assert_eq!(data, vec!["x", "y"]);
	}
}
