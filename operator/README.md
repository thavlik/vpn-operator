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
The rust types [are in a sister crate](../types). Building this crate will generate the Custom Resource Definition yaml in the [crds/ directory at the root of the repository](../crds).

### Testing
Tests can run locally or in a pod with admin privileges. To run the end-to-end tests, use `cargo`:
```bash
cargo test
```