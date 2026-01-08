#! /bin/sh
# Get number of CPU cores
CORES=$(nproc)

AFL_NO_BUILTIN=1 cargo afl build

# Launch 1 Master
screen -dmS master cargo afl fuzz -i in -o out -M fuzzer1 target/debug/parser_fuzz

# Launch Slaves for the rest
for i in $(seq 2 "$CORES"); do
    screen -dmS "slave$i" cargo afl fuzz -i in -o out -S "fuzzer$i" target/debug/parser_fuzz
done