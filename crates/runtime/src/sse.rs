pub fn parse_sse_line(line: &str) -> Option<SseEvent> {
    if line.is_empty() {
        return None;
    }
    if line.starts_with(':') {
        return Some(SseEvent::Comment);
    }
    if let Some(data) = line.strip_prefix("data: ") {
        if data.trim() == "[DONE]" {
            return Some(SseEvent::Done);
        }
        return Some(SseEvent::Data(data.to_string()));
    }
    if let Some(event_type) = line.strip_prefix("event: ") {
        return Some(SseEvent::Event(event_type.trim().to_string()));
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Data(String),
    Event(String),
    Comment,
    Done,
}

pub struct SseStream<R> {
    reader: R,
    buffer: String,
}

impl<R: std::io::BufRead> SseStream<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: String::new(),
        }
    }

    pub fn next_event(&mut self) -> Option<anyhow::Result<SseEvent>> {
        loop {
            self.buffer.clear();
            match self.reader.read_line(&mut self.buffer) {
                Ok(0) => return None,
                Ok(_) => {
                    let line = self.buffer.trim_end_matches('\n').trim_end_matches('\r');
                    if let Some(event) = parse_sse_line(line) {
                        return Some(Ok(event));
                    }
                }
                Err(e) => return Some(Err(anyhow::anyhow!("SSE read error: {e}"))),
            }
        }
    }
}
