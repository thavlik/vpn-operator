#!/bin/bash
set -euo pipefail
cd $(dirname $0)/..
# Setting these variables uses a real VPN service for testing.
export SECRET_NAME=actual-vpn-cred
export SECRET_NAMESPACE=vpn
cargo test -j 10
