# blockwatch
[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)

Linter that tracks changes between dependent blocks of code.

## How

Declare the blocks in the comments of the source file.

Validate by running `git diff --patch | blockwatch`:

### Example

Whenever some block is modified then all its affected blocks (possibly in different files) should also be updated.

`main.rs`:

```rust
// <block affects="README.md:supported-languages">
const SUPPORTED_LANGUAGES = ["rust", "java", "python"];
// </block>
```

`README.md`:

```markdown
## Supported languages

[//]: # (<block name="supported-languages">)

- Java
- Rust

[//]: # (</block>)

```

If the block in `main.rs` is modified (e.g. added `python` to the list) then the following command will produce an
error:

```shell
git diff --patch | blockwatch
```

## Supported languages

[//]: # (<block name="supported-languages">)

- Java
- Markdown
- Rust

[//]: # (</block>)

## More examples

### Blocks may reference blocks in the same file

```rust
// <block name="foo" affects=":bar, :buzz">
fn main() {
    println!("Blocks can affect multiple other blocks in declared in the same file");
    println!("Just omit the file name in the 'affects' attribute");
}
// </block>

// <block name="bar">
// Some other piece of code.
// </block>

// <block name="buzz">
// One more.
// </block>
```

### Blocks may reference each other

```rust
// <block name="foo" affects=":bar">
fn foo() {
    println!("Hello");
}
// </block>

// <block name="bar" affects=":foo">
fn bar() {
    println!("Hi!");
}
// </block>
```

### Blocks can be nested

```rust
// <block name="entire-file">
fn foo() {
    println!("Hello");
}

// <block name="small-block">
fn bar() {
    println!("Hi!");
}
// </block>
// </block>
```

## Known limitations

- deleted blocks are ignored
- blocks declared in unsupported languages are ignored
- multiple blocks can't be declared in a single line: `<block><block>will not work</block/</block>`

## Install

TODO

## Contributing

### Adding a language support

TODO

## Run tests

```shell
cargo test
```