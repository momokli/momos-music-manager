//! Golden contract test for the `comment-template` crate (issue #664).
//!
//! Reads `tests/fixtures/cases.json` and iterates both sections:
//!
//! * `render` — asserts `render(template, &TrackFacts) == expected`.
//! * `validate` — asserts `validate(template)` and, when `ok:false`, that the
//!   error contains every string in `error_contains`.
//!
//! The fixture is the shared Hub↔kellner contract; R(`double_brace…`) and
//! V(`{{tags}}`) sit side by side on purpose to lock the documented divergence
//! between the two predicates (see the crate-level docs).

use comment_template::{TrackFacts, render, validate};
use serde_json::Value;

fn fixture() -> Value {
    let raw = include_str!("fixtures/cases.json");
    serde_json::from_str(raw).expect("cases.json is valid JSON")
}

fn string_list(v: &Value, field: &str) -> Vec<String> {
    v.get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|t| t.as_str().expect("list item is a string").to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn facts_from(v: &Value) -> TrackFacts {
    TrackFacts {
        types: string_list(v, "types"),
        tags: string_list(v, "tags"),
        source_ids: string_list(v, "source_ids"),
        bpm: v["bpm"].as_f64(),
        key: v["key"].as_str().map(str::to_string),
        energy: v["energy"].as_f64(),
    }
}

#[test]
fn golden_render_cases() {
    let data = fixture();
    let cases = data["render"].as_array().expect("render section");
    assert!(!cases.is_empty(), "fixture has render cases");
    for case in cases {
        let name = case["name"].as_str().unwrap_or("<unnamed>");
        let template = case["template"].as_str().expect("template is a string");
        let expected = case["expected"].as_str().expect("expected is a string");
        let got = render(template, &facts_from(&case["facts"]));
        assert_eq!(got, expected, "render case {name:?}");
    }
}

#[test]
fn golden_validate_cases() {
    let data = fixture();
    let cases = data["validate"].as_array().expect("validate section");
    assert!(!cases.is_empty(), "fixture has validate cases");
    for case in cases {
        let name = case["name"].as_str().unwrap_or("<unnamed>");
        let template = case["template"].as_str().expect("template is a string");
        let ok = case["ok"].as_bool().expect("ok is a bool");
        let result = validate(template);
        assert_eq!(result.is_ok(), ok, "validate case {name:?}: {result:?}");
        if !ok {
            let err = result.unwrap_err();
            for needle in case["error_contains"]
                .as_array()
                .expect("error_contains is an array")
            {
                let needle = needle.as_str().expect("needle is a string");
                assert!(
                    err.contains(needle),
                    "validate case {name:?}: error {err:?} must contain {needle:?}"
                );
            }
        }
    }
}
