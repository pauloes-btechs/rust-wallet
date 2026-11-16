#!/usr/bin/env bash
# Bring the regtest harness to a known state and export credentials.
#
#   harness/regtest/scripts/bootstrap.sh          # up + fund + channel
#   source harness/regtest/.env.regtest           # RW_* variables for tests/CLI
#
# Result: alice has ~1 BTC on-chain and a 1,000,000 sat channel to bob
# (100,000 pushed to bob so both sides can pay). REGTEST ONLY.
set -euo pipefail
cd "$(dirname "$0")/.."
DC="docker compose"
BTC="$DC exec -T bitcoind bitcoin-cli -regtest -rpcuser=regtest -rpcpassword=regtest"
ln() { local who=$1; shift; $DC exec -T "lnd-$who" lncli --network=regtest "$@"; }
mine() { $BTC generatetoaddress "$1" "$MINER" > /dev/null; }
retry() { local n=0; until "$@" > /dev/null 2>&1; do n=$((n+1)); [ $n -ge 90 ] && { echo "timeout: $*" >&2; return 1; }; sleep 2; done; }

$DC up -d
retry $BTC getblockchaininfo
$BTC -named createwallet wallet_name=miner load_on_startup=true > /dev/null 2>&1 || $BTC loadwallet miner > /dev/null 2>&1 || true
MINER=$($BTC getnewaddress)
[ "$($BTC getblockcount)" -lt 101 ] && mine 101

for who in alice bob; do retry ln "$who" getinfo; done
ALICE_PK=$(ln alice getinfo | python3 -c 'import sys,json;print(json.load(sys.stdin)["identity_pubkey"])')
BOB_PK=$(ln bob getinfo | python3 -c 'import sys,json;print(json.load(sys.stdin)["identity_pubkey"])')

ALICE_ADDR=$(ln alice newaddress p2tr | python3 -c 'import sys,json;print(json.load(sys.stdin)["address"])')
$BTC sendtoaddress "$ALICE_ADDR" 1 > /dev/null
BOB_ADDR=$(ln bob newaddress p2wkh | python3 -c 'import sys,json;print(json.load(sys.stdin)["address"])')
$BTC sendtoaddress "$BOB_ADDR" 0.5 > /dev/null
mine 6
retry sh -c "$DC exec -T lnd-alice lncli --network=regtest walletbalance | grep -q '\"confirmed_balance\": \"[1-9]'"

ln alice connect "$BOB_PK@lnd-bob:9735" > /dev/null 2>&1 || true
if ! ln alice listchannels | grep -q "$BOB_PK"; then
  retry sh -c "$DC exec -T lnd-alice lncli --network=regtest getinfo | grep -q '\"synced_to_chain\": true'"
  ln alice openchannel --node_key "$BOB_PK" --local_amt 1000000 --push_amt 100000 > /dev/null
  mine 6
fi
retry sh -c "$DC exec -T lnd-alice lncli --network=regtest listchannels --active_only | grep -q '$BOB_PK'"
# "Active" is not "routable": the router learns the link's bandwidth a moment
# later. Wait until a real route to bob exists before declaring ready.
retry sh -c "$DC exec -T lnd-alice lncli --network=regtest queryroutes --dest '$BOB_PK' --amt 20000 | grep -q '\"total_amt\"'"

# Credentials: copied out of the containers so they are readable by the host user.
mkdir -p creds/alice creds/bob creds/litd
for who in alice bob; do
  $DC cp "lnd-$who:/root/.lnd/tls.cert" "creds/$who/tls.cert"
  $DC cp "lnd-$who:/root/.lnd/data/chain/bitcoin/regtest/admin.macaroon" "creds/$who/admin.macaroon"
done
if retry $DC exec -T litd test -f /root/.lnd/data/chain/bitcoin/regtest/admin.macaroon; then
  $DC cp "litd:/root/.lnd/tls.cert" creds/litd/tls.cert
  $DC cp "litd:/root/.lnd/data/chain/bitcoin/regtest/admin.macaroon" creds/litd/admin.macaroon
fi

H=$(pwd)
cat > .env.regtest <<ENV
export RW_ALICE_ENDPOINT=127.0.0.1:10109
export RW_ALICE_CERT=$H/creds/alice/tls.cert
export RW_ALICE_MACAROON=$H/creds/alice/admin.macaroon
export RW_BOB_ENDPOINT=127.0.0.1:10110
export RW_BOB_CERT=$H/creds/bob/tls.cert
export RW_BOB_MACAROON=$H/creds/bob/admin.macaroon
export RW_BOB_PUBKEY=$BOB_PK
export RW_ALICE_PUBKEY=$ALICE_PK
export RW_LITD_ENDPOINT=127.0.0.1:10111
export RW_LITD_CERT=$H/creds/litd/tls.cert
export RW_LITD_MACAROON=$H/creds/litd/admin.macaroon
ENV
echo "regtest ready: alice=$ALICE_PK bob=$BOB_PK (source harness/regtest/.env.regtest)"
