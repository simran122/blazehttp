mod support {
    pub mod vectors;
}

use std::collections::BTreeSet;

use engine_core::generated::spec::{ErrorCode, Method};
use support::vectors::{Expect, load_suite};

const SUITE: &str = "engine-core:h1_response";

#[test]
fn h1_response_suite_loads_with_requests_the_engine_knows() {
    let cases = load_suite(SUITE);
    assert!(!cases.is_empty());
    let methods: BTreeSet<&str> = (0..)
        .map_while(Method::from_wire)
        .map(Method::as_str)
        .collect();
    for (file, case) in &cases {
        assert!(!case.input.is_empty(), "{file}/{}: empty input", case.name);
        assert!(
            methods.contains(case.method.as_str()),
            "{file}/{}: unknown method {}",
            case.name,
            case.method
        );
    }
}

#[test]
fn expected_errors_are_errors_the_engine_can_produce() {
    let known: BTreeSet<String> = ErrorCode::ALL_SAMPLES
        .iter()
        .map(ToString::to_string)
        .collect();
    for (file, case) in load_suite(SUITE) {
        if let Expect::Error(error) = &case.expect {
            assert!(
                known.contains(error),
                "{file}/{}: {error} is not an ErrorCode display form",
                case.name
            );
        }
    }
}
