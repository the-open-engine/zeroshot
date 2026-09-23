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
