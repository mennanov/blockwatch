// Indexed path into the first document.
// <block same-as="tests/testdata/symbol_references/multi_doc_yaml_target.yaml#/0/kind" same-as-pattern="[A-Z][a-z]+">
const FIRST_KIND: &str = "Service";
// </block>

// Indexed path into the second non-empty document (empty document skipped).
// <block same-as="tests/testdata/symbol_references/multi_doc_yaml_target.yaml#/1/spec/replicas" same-as-pattern="\d+">
const INDEXED_REPLICAS: usize = 2;
// </block>

// Root-level compound nested filter picking the Rollout named `api`.
// <block same-as="tests/testdata/symbol_references/multi_doc_yaml_target.yaml#/~[kind=Rollout][metadata/name=api]/spec/replicas" same-as-pattern="\d+">
const FILTERED_REPLICAS: usize = 2;
// </block>

// Root-level compound filter with `~1` escaped `/` in a value.
// <block same-as="tests/testdata/symbol_references/multi_doc_yaml_target.yaml#/~[apiVersion=argoproj.io~1v1alpha1][metadata/name=api]/kind" same-as-pattern="[A-Z][a-z]+">
const ROLLOUT_KIND: &str = "Rollout";
// </block>
