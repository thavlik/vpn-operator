#!/bin/bash
set -euo pipefail
cd $(dirname $0)/
I=0
while true; do
    echo "Test run $I"
    ./test-actual.sh
    I=$((I+1))
    sleep 1
done
