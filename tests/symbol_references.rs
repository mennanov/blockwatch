//! Symbol references (`file#/path`) end to end: how a reference spells a path, and that every file
//! type with symbols is wired to a parser that derives them. The tests observe resolution through
//! `same-as`, since a block agrees with its target only when the reference resolved to the right
//! value.
//!
//! Each file type gets one minimal test. What a language derives is pinned by its own unit tests,
//! which call its parser directly and so cannot tell whether an extension reaches that parser.

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;

#[test]
fn symbol_paths_with_escapes_resolve() {
    // Each block in the fixture spells its path with a different escape.
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/escapes_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn reference_list_mixing_symbols_and_blocks_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/mixed_list_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn json_target_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/json_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn jsonc_target_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/jsonc_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn toml_target_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/toml_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn yaml_target_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/yaml_source.rs");
    cmd.output().unwrap().assert().success();
}

#[test]
fn yml_target_resolves() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/symbol_references/yml_source.rs");
    cmd.output().unwrap().assert().success();
}
