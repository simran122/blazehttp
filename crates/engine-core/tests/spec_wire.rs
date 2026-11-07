use engine_core::generated::spec::{
    ErrorCode, H1_LIMITS, Method, ProtocolDetail, QueueScope, TimeoutDetail,
};

#[test]
fn every_wire_code_round_trips() {
    assert_ne!(ErrorCode::ALL_SAMPLES.len(), 0);
    for &sample in ErrorCode::ALL_SAMPLES {
        let (word, arg) = sample.to_wire();
        assert_eq!(ErrorCode::from_wire(word, arg), Some(sample), "{sample}");
    }
}

#[test]
fn wire_words_are_unique_per_sample() {
    let mut words: Vec<u32> = ErrorCode::ALL_SAMPLES
        .iter()
        .map(|e| e.to_wire().0)
        .collect();
    let total = words.len();
    words.sort_unstable();
    words.dedup();
    assert_eq!(words.len(), total);
}

#[test]
fn queue_full_keeps_bound() {
    let full = ErrorCode::QueueFull {
        scope: QueueScope::Bytes,
        bound: u32::MAX,
    };
    let (word, arg) = full.to_wire();
    assert_eq!(arg, u32::MAX);
    assert_eq!(ErrorCode::from_wire(word, arg), Some(full));
}

#[test]
fn display_uses_code_slash_detail() {
    let shown = |e: ErrorCode| e.to_string();
    assert_eq!(
        shown(ErrorCode::Protocol(ProtocolDetail::MalformedResponse)),
        "PROTOCOL/malformed-response"
    );
    assert_eq!(
        shown(ErrorCode::Timeout(TimeoutDetail::SlowBody)),
        "TIMEOUT/slow-body"
    );
    assert_eq!(
        shown(ErrorCode::QueueFull {
            scope: QueueScope::Global,
            bound: 3
        }),
        "QUEUE_FULL/global"
    );
    assert_eq!(
        shown(ErrorCode::TooManyRedirects { count: 21 }),
        "TOO_MANY_REDIRECTS"
    );
}

#[test]
fn unknown_words_are_rejected() {
    assert_eq!(ErrorCode::from_wire(255, 0), None);
    assert_eq!(
        ErrorCode::from_wire(99 << 8, 0),
        None,
        "no TIMEOUT detail 99"
    );
    assert_eq!(
        ErrorCode::from_wire(8 | (1 << 8), 5),
        None,
        "TOO_MANY_REDIRECTS has no detail"
    );
}

#[test]
fn methods_round_trip_and_reject_unknown() {
    for id in 0..8 {
        let method = Method::from_wire(id).expect("known method");
        assert_eq!(method.to_wire(), id);
        assert_ne!(method.as_str(), "");
    }
    assert_eq!(Method::from_wire(8), None);
    assert_eq!(Method::Get.as_str(), "GET");
}

#[test]
fn h1_limits_match_design() {
    assert_eq!(H1_LIMITS.max_head_bytes, 65_536);
    assert_eq!(H1_LIMITS.max_status_line_bytes, 8_192);
    assert_eq!(H1_LIMITS.max_header_fields, 100);
    assert_eq!(H1_LIMITS.max_interim_responses, 10);
    assert_eq!(H1_LIMITS.max_request_target_bytes, 16_384);
    assert_eq!(H1_LIMITS.max_request_fields_bytes, 65_536);
    assert_eq!(H1_LIMITS.max_copied_body_bytes, 65_536);
}
