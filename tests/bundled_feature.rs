//! Verifies the `bundled` (feature-gated, embedded grammar) path end to end.
//!
//! Only runs when the `grammar-json` feature is enabled:
//!
//! ```sh
//! cargo test --test bundled_feature --features grammar-json
//! ```

#[cfg(feature = "grammar-json")]
#[test]
fn bundled_json_round_trips_and_labels() {
    use jaune::{SyntaxSet, TokenizerOp};

    let set = SyntaxSet::bundled();
    assert!(!set.is_empty(), "bundled set should contain the json grammar");

    let def = set.find_by_name("json").expect("json grammar bundled");
    let scope = def.scope;
    let input = "{\"a\": [1, true]}";
    let ops: Vec<_> = set.tokenizer(input, scope).unwrap().collect();

    // Round-trips.
    let reconstructed: String = ops
        .iter()
        .map(|op| match op {
            TokenizerOp::Content(c) => *c,
            TokenizerOp::Newline => "\n",
            _ => "",
        })
        .collect();
    assert_eq!(reconstructed, input);

    // Labels a number.
    let labelled_number = ops.iter().any(|op| match op {
        TokenizerOp::Push(s) => s.to_string().contains("constant.numeric"),
        _ => false,
    });
    assert!(labelled_number, "bundled json should label numeric constants");
}
