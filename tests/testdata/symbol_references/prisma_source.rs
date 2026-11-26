// <block same-as="tests/testdata/symbol_references/prisma_target.prisma#/db/provider" same-as-pattern="(?:= \"|^)(?P<value>[a-z]+)(?:\";|$)">
const PROVIDER: &str = "postgresql";
// </block>

// <block same-as="tests/testdata/symbol_references/prisma_target.prisma#/Color" same-as-pattern="\b(?P<value>[A-Z]+)\b">
const colors: &[&str] = &["RED", "GREEN"];
// </block>

// <block same-as="tests/testdata/symbol_references/prisma_target.prisma#/Color/GREEN" same-as-pattern="\b(?P<value>[A-Z]+)\b">
const green: &str = "GREEN";
// </block>

// <block same-as="tests/testdata/symbol_references/prisma_target.prisma#/Item/name" same-as-pattern="\b(?P<value>name)\b">
const NAME_FIELD: &str = "name";
// </block>
