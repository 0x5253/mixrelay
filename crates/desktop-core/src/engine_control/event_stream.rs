//! Splits the engine's server-sent telemetry events out of response body chunks.

/// Largest event the parser accepts; a longer one is malformed.
pub const EVENT_LIMIT: usize = 4 * 1024 * 1024;

/// Collects events, which end with a blank line (LF, as the engine's server writes them).
#[derive(Debug, Default)]
pub struct EventParser {
    buffer: Vec<u8>,
    /// Bytes already searched for a terminator, so a long event is scanned once.
    scanned: usize,
}

impl EventParser {
    /// Appends a chunk and returns the `data` payload of every event it completes.
    /// Comment lines (keep-alives) and fields other than `data` are ignored.
    ///
    /// # Errors
    /// Returns an error, and discards buffered data, when any event exceeds the limit.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        let mut search = self.scanned.saturating_sub(1).max(start);
        while let Some(offset) = self.buffer[search..]
            .windows(2)
            .position(|pair| pair == b"\n\n")
        {
            let end = search + offset + 2;
            if end - start > EVENT_LIMIT {
                return Err(self.oversized());
            }
            let text = String::from_utf8_lossy(&self.buffer[start..end]);
            let data: Vec<&str> = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
                .collect();
            if !data.is_empty() {
                events.push(data.join("\n"));
            }
            start = end;
            search = end;
        }
        self.buffer.drain(..start);
        if self.buffer.len() > EVENT_LIMIT {
            return Err(self.oversized());
        }
        self.scanned = self.buffer.len();
        Ok(events)
    }

    fn oversized(&mut self) -> String {
        self.buffer.clear();
        self.scanned = 0;
        "telemetry event exceeds its limit".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_chunks_yield_their_data_and_skip_keep_alives() {
        let mut parser = EventParser::default();
        assert!(
            parser
                .push(b"event: meters\ndata: {\"a\"")
                .unwrap()
                .is_empty()
        );
        assert!(parser.push(b":1}\n").unwrap().is_empty());
        assert_eq!(
            parser.push(b"\n:\n\ndata: x\ndata:y\n\n").unwrap(),
            ["{\"a\":1}", "x\ny"]
        );
        assert!(parser.push(b"event: only\n\n").unwrap().is_empty());
    }

    #[test]
    fn an_oversized_event_is_discarded_even_when_complete() {
        let mut parser = EventParser::default();
        assert!(parser.push(&vec![b'a'; EVENT_LIMIT + 1]).is_err());
        assert_eq!(parser.push(b"data: next\n\n").unwrap(), ["next"]);
        let mut complete = vec![b'a'; EVENT_LIMIT];
        complete.extend_from_slice(b"\n\n");
        assert!(parser.push(&complete).is_err());
        assert_eq!(parser.push(b"data: after\n\n").unwrap(), ["after"]);
    }
}
