pub(super) struct TailBuffer {
    bytes: Vec<u8>,
    capacity: usize,
    truncated: bool,
}

pub(super) struct TailSnapshot {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

impl TailBuffer {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            capacity,
            truncated: false,
        }
    }

    pub(super) fn append(&mut self, value: &[u8]) {
        if value.len() >= self.capacity {
            self.truncated |= !self.bytes.is_empty() || value.len() > self.capacity;
            self.bytes.clear();
            if let Some(tail) = value.get(value.len().saturating_sub(self.capacity)..) {
                self.bytes.extend_from_slice(tail);
            }
            return;
        }
        let required = self.bytes.len() + value.len();
        if required > self.capacity {
            let discard = required - self.capacity;
            self.bytes.copy_within(discard.., 0);
            self.bytes.truncate(self.bytes.len() - discard);
            self.truncated = true;
        }
        self.bytes.extend_from_slice(value);
    }

    pub(super) fn snapshot(&self) -> TailSnapshot {
        TailSnapshot {
            bytes: self.bytes.clone(),
            truncated: self.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(buffer: &TailBuffer) -> (Vec<u8>, bool) {
        let snapshot = buffer.snapshot();
        (snapshot.bytes, snapshot.truncated)
    }

    #[test]
    fn retains_only_the_latest_bytes_and_reports_actual_truncation() {
        let mut exact = TailBuffer::new(4);
        exact.append(b"abcd");
        assert_eq!(snapshot(&exact), (b"abcd".to_vec(), false));

        let mut incremental = TailBuffer::new(4);
        incremental.append(b"ab");
        incremental.append(b"cd");
        assert_eq!(snapshot(&incremental), (b"abcd".to_vec(), false));
        incremental.append(b"ef");
        assert_eq!(snapshot(&incremental), (b"cdef".to_vec(), true));
        incremental.append(b"ghij");
        assert_eq!(snapshot(&incremental), (b"ghij".to_vec(), true));

        let mut oversized = TailBuffer::new(4);
        oversized.append(b"prefix-tail");
        assert_eq!(snapshot(&oversized), (b"tail".to_vec(), true));

        let mut disabled = TailBuffer::new(0);
        disabled.append(b"");
        assert_eq!(snapshot(&disabled), (Vec::new(), false));
        disabled.append(b"discarded");
        assert_eq!(snapshot(&disabled), (Vec::new(), true));
    }
}
