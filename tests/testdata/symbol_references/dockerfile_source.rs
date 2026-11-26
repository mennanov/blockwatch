// <block same-as="tests/testdata/symbol_references/Dockerfile#/GO_VERSION" same-as-pattern="\d+\.\d+">
const GO_VERSION: &str = "1.25";
// </block>

// <block same-as="tests/testdata/symbol_references/Dockerfile#/0" same-as-pattern="python:\d+\.\d+-slim">
const PYTHON_IMAGE: &str = "python:3.12-slim";
// </block>

// <block same-as="tests/testdata/symbol_references/Dockerfile#/1, tests/testdata/symbol_references/Dockerfile#/builder" same-as-pattern="golang:\d+\.\d+-alpine">
const GO_IMAGE: &str = "golang:1.25-alpine";
// </block>

// <block same-as="tests/testdata/symbol_references/Dockerfile#/2" same-as-pattern="scratch">
const FINAL_IMAGE: &str = "scratch";
// </block>
