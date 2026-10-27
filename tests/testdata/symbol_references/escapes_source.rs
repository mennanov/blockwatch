// `~1` stands for a `/` inside a key.
// <block same-as="tests/testdata/symbol_references/escapes_target.json#/dependencies/@types~1node" same-as-pattern="\d+\.\d+\.\d+">
const TYPES_NODE: &str = "20.1.0";
// </block>
// `~0` stands for a `~` inside a key.
// <block same-as="tests/testdata/symbol_references/escapes_target.json#/dependencies/a~0b" same-as-pattern="\d+">
const A_TILDE_B: usize = 1;
// </block>
// A `:` has to be percent-encoded, since a reference cannot hold both `#` and `:`.
// <block same-as="tests/testdata/symbol_references/escapes_target.json#/dependencies/a%3Ab" same-as-pattern="\d+">
const A_COLON_B: usize = 2;
// </block>
