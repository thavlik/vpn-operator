# vpn-operator binary
The goal is this readme is to document various aspects of the operator binary itself.

## Building
### Docker
From the repository root directory:
```bash
docker build \
    -t thavlik/vpn-operator:latest \
    -f operator/Dockerfile \
    .
```

### Kindest
This project uses my [custom toolchain](https://github.com/midcontinentcontrols/kindest). Build it with a single command:
```bash
# pass -v (--verbose) to print docker build logs
kindest build -v
```

## Development
The Rust types [are in a sister crate](../types). Building this crate will generate the Custom Resource Definition yaml in the [crds/ directory at the root of the repository](../crds).

## Testing
Tests can run locally or in a pod with admin privileges. To run the end-to-end tests:
```bash
cargo test
```
If you have real VPN credentials you want to use in the tests, specify the environment variables `SECRET_NAME` and `SECRET_NAMESPACE` to point to the in-cluster `Secret` resource containing the credentials. If you name your `Secret` resource `vpn/actual-vpn-cred`, you can use the convenience script at [`../scripts/test-actual.sh`](../scripts/test-actual.sh):
```bash
#!/bin/bash
set -euo pipefail
cd $(dirname $0)/..
# Setting these variables uses a real VPN service for testing.
export SECRET_NAME=actual-vpn-cred
export SECRET_NAMESPACE=vpn
cargo test -j 10 $@
```