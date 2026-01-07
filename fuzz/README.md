# Running fuzz tests

Make sure `cargo version` is nightly.

```shell
cargo install cargo-afl
cargo afl config --build
AFL_NO_BUILTIN=1 cargo afl build
cargo afl fuzz -i in -o out target/debug/parser_fuzz
```