#!/usr/bin/env bash
# Mine N blocks (default 1) on the harness chain.
set -euo pipefail
cd "$(dirname "$0")/.."
BTC="docker compose exec -T bitcoind bitcoin-cli -regtest -rpcuser=regtest -rpcpassword=regtest"
$BTC generatetoaddress "${1:-1}" "$($BTC -rpcwallet=miner getnewaddress)"
