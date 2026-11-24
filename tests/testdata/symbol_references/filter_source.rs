// The filter picks the item whose `name` is `blue`, which is the second one.
// <block same-as="tests/testdata/symbol_references/filter_target.yaml#/items/~[name=blue]/size" same-as-pattern="\d+">
const BLUE_SIZE: usize = 2;
// </block>

// The nested filter picks the item whose `metadata/name` is `api`, which is the second one.
// <block same-as="tests/testdata/symbol_references/filter_target.yaml#/items/~[metadata/name=api]/size" same-as-pattern="\d+">
const API_SIZE: usize = 2;
// </block>
