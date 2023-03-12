# vpn-operator
Kubernetes operator for VPN sidecars written in pure rust. 

## Installation
1. Apply the Custom Resource Definitions:
```bash
kubectl apply -f crds/
```
2. Install the vpn-operator helm chart:
```bash
# Create your chart configuration file.
echo "prometheus: false" > values.yaml

# Install the chart into the `vpn` namespace.
RELEASE_NAME=vpn
CHART_PATH=chart/
helm install \
  --namespace=vpn \
  --create-namespace \
  $RELEASE_NAME \
  $CHART_PATH \
  -f values.yaml
```

## Usage
1. Create a `Provider` resource and secret with your VPN credentials:
```bash
cat <<EOF | kubectl apply -f -
apiVersion: v1
kind: Secret
metadata:
  name: my-vpn-credentials
spec:
  stringData:
  # Environment variables for glueten, or however you
  # choose to connect to your VPN, go here.
  # Refer to https://github.com/qdm12/gluetun
    VPN_NAME: ""
    VPN_USERNAME: ""
    VPN_PASSWORD: ""
---
apiVersion: vpn.beebs.dev/v1
kind: Provider
metadata:
  name: my-vpn
spec:
  # In this example, this VPN account only allows
  # five devices to be active simultaneously.
  maxSlots: 5

  # Corresponds to the above Secret resource.
  secret: my-vpn-credentials
EOF
```

2. Make sure the `Provider` enters the `Active` phase:
```bash
kubectl get provider -w my-vpn
```

3. Create `Mask` resources to reserve slots with the `Provider`:
```bash
cat <<EOF | kubectl apply -f -
apiVersion: vpn.beebs.dev/v1
kind: Mask
metadata:
  name: my-mask
spec:
```

4. Wait for the `Mask`'s phase to be `Active` before using it:
```bash
kubectl get mask -w my-mask
```

5. The `Mask` contains a reference to the VPN credentials Secret created for it at `status.provider.secret`. Plug these values into your VPN containers.

### Notes
- If no slots are available, the phase of the `Mask` will be `Waiting`.
- Your application is responsible for monitoring the status of your container's `Mask` and killing the pod if the provider is changed or unassigned.

## License
All code in this repository is released under [MIT](LICENSE-MIT) / [Apache 2.0](LICENSE-Apache) dual license, which is extremely permissive. Please open an issue if somehow these terms are insufficient.
