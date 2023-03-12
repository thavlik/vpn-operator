# vpn-operator binary
The goal is this readme is to document various aspects of the operator.

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
