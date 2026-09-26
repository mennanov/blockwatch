// `[package]` is written in two places, with `[dependencies]` between them. Only the keys of
// `[package]` are compared.
// <block same-as="tests/testdata/same_as/split_table_target.toml#/package" same-as-pattern="(?P<value>[\w-]+) =">
let name = "example";
let version = "1.0";
let docs = true;
// </block>
