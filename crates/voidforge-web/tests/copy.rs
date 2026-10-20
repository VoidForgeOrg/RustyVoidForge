//! Every `ErrorCode`'s copy is phase4-ux §6.1's, and the web's own states are §6.2's (P4.2,
//! D-059). The expected strings are `fixtures/copy.json`, extracted from dev-design's document
//! rather than restated by hand, so a swapped or edited sentence fails here.

use std::collections::BTreeMap;

use serde::Deserialize;
use voidforge_api_types::error::ErrorCode;
use voidforge_web::api::{ApiError, Unreachable};
use voidforge_web::messages::{Context, Fill, copy, for_error};

#[derive(Deserialize)]
struct Pair {
    page: String,
    command: String,
}

#[derive(Deserialize)]
struct Doc {
    codes: BTreeMap<String, Pair>,
    without_number: BTreeMap<String, String>,
    web: BTreeMap<String, Pair>,
}

fn doc() -> Doc {
    let path = format!("{}/tests/fixtures/copy.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

const FILL: Fill = Fill {
    planet: Some(4),
    target: Some(9),
};

fn filled(s: &str) -> String {
    s.replace("{p}", "4").replace("{t}", "9")
}

#[test]
fn every_code_has_its_section_6_1_copy_for_a_page_and_an_order() {
    let doc = doc();
    assert_eq!(
        doc.codes.len(),
        ErrorCode::ALL.len(),
        "the table names every code"
    );
    for code in ErrorCode::ALL {
        let want = &doc.codes[code.as_str()];
        assert_eq!(
            copy(code, Context::Page, FILL),
            filled(&want.page),
            "{code:?}"
        );
        assert_eq!(
            copy(code, Context::Command, FILL),
            filled(&want.command),
            "{code:?}"
        );
    }
}

#[test]
fn copy_with_a_number_reads_without_it_when_the_page_has_none() {
    let doc = doc();
    for code in ErrorCode::ALL {
        let page = copy(code, Context::Page, Fill::default());
        match doc.without_number.get(code.as_str()) {
            Some(want) => assert_eq!(&page, want, "{code:?}"),
            None => assert_eq!(page, doc.codes[code.as_str()].page, "{code:?}"),
        }
        assert!(
            !page.contains('{') && !page.contains('#'),
            "{code:?}: {page}"
        );
    }
}

#[test]
fn a_message_shows_the_copy_with_the_code_and_the_web_s_own_states_without_one() {
    let doc = doc();
    for code in ErrorCode::ALL {
        let m = for_error(
            &ApiError::Refused {
                code,
                status: code.status(),
                retry_after: None,
            },
            Context::Page,
            FILL,
        );
        assert_eq!(m.copy, filled(&doc.codes[code.as_str()].page));
        assert_eq!(m.code, Some(code.as_str()));
    }
    for (state, e) in [
        ("unreachable", ApiError::Unreachable(Unreachable::Connect)),
        ("unreachable", ApiError::Unreachable(Unreachable::Timeout)),
        ("unexpected", ApiError::UnexpectedStatus(302)),
        ("unexpected", ApiError::Undecodable { status: 200 }),
    ] {
        let want = &doc.web[state];
        let page = for_error(&e, Context::Page, FILL);
        let order = for_error(&e, Context::Command, FILL);
        assert_eq!((page.copy.as_str(), page.code), (want.page.as_str(), None));
        assert_eq!(
            (order.copy.as_str(), order.code),
            (want.command.as_str(), None)
        );
    }
}
