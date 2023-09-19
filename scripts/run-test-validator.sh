#! /bin/bash

function cleanup() {
    kill -TERM "$SOLANA_TEST_VALIDATOR_PID"
    exit
}

trap cleanup SIGINT

DIR="$(dirname "${BASH_SOURCE[0]}")"
BUILD_DIR="$DIR/../target"
LEDGER_DIR="$DIR/../test-ledger"

FUNDING_PROGRAM="--bpf-program Fnd1yWeU4ajtCbzuDLsZq3cuoUiroJCYRoUi2y6PVZfy $BUILD_DIR/deploy/funding.so"

solana-test-validator -r --compute-unit-limit 1400000 --ledger $LEDGER_DIR $FUNDING_PROGRAM & SOLANA_TEST_VALIDATOR_PID=$!

wait $SOLANA_TEST_VALIDATOR_PID
