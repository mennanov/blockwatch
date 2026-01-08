# Running fuzz tests

Make sure `cargo version` is nightly.

Set up the environment:

```shell
cargo install cargo-afl
cargo afl config --build
```

```shell
# Build the fuzz target
AFL_NO_BUILTIN=1 cargo afl build
# Run fuzzer on 1 CPU
cargo afl fuzz -i in -o out target/debug/parser_fuzz
```

## Running multiple fuzzers in parallel

```shell
sh run.sh
```

Observe all running fuzzers:

```shell
cargo afl whatsup out/
```

# Troubleshooting

When experiencing linker issues like
`ld: warning: ignoring file '.../afl-llvm/afl-compiler-rt.o': found architecture 'x86_64', required architecture 'arm64`
when running `AFL_NO_BUILTIN=1 cargo afl build` do this:

```shell
# Ensure you have the necessary LLVM tools
brew install llvm
# Point to the homebrew LLVM if necessary (adjust version if needed)
export LLVM_CONFIG=$(brew --prefix llvm)/bin/llvm-config
# Try building again
AFL_NO_BUILTIN=1 cargo afl build
```