#!/bin/bash
set -euo pipefail
cd $(dirname $0)/..
export SECRET_NAME=actual-vpn-cred
export SECRET_NAMESPACE=vpn
cargo test -j 10 basic
