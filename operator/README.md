# vpn-operator binary
The goal of this readme is to document various aspects of the operator binary itself. Much effort has gone into the code comments, so you should refer to the source code for details on specific snippets.

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

## Generating CRDs
Building this crate will generate the Custom Resource Definition yaml in the [crds/ directory at the root of the repository](../crds). The Rust types are located in a [sister crate](../types).

## Testing
Tests can run locally or in a pod with admin privileges. To run the end-to-end tests with the default kubectl context:
```bash
# Override KUBECONFIG env to use a different kubectl config file.
#export KUBECONFIG="$HOME/.kube/config"
cargo test
```
It is possible to run the tests with arbitrary VPN credentials. To test any VPN provider, specify the environment variables `SECRET_NAME` and `SECRET_NAMESPACE`. Their values must point to the in-cluster `Secret` resource that contains the credentials. If you create the `Secret` resource `vpn/actual-vpn-cred`, you can use the convenience script at [`../scripts/test-actual.sh`](../scripts/test-actual.sh):
```bash
#!/bin/bash
set -euo pipefail
cd $(dirname $0)/..
# Setting these variables uses a real VPN service for testing.
export SECRET_NAME=actual-vpn-cred
export SECRET_NAMESPACE=vpn
cargo test $@
```