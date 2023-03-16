# vpn-operator
Kubernetes operator for VPN sidecars written in pure [Rust](https://www.rust-lang.org/).

## Motivation
This operator simplifies the process of hiding your pods behind one or more VPN services. Instead of assigning the same exact VPN sidecar to every pod you want cloaked, you use the included `Provider` and `Mask` Custom Resources to automate credentials distribution across any number of pods and VPN services.

## Installation
1. Clone the repository:
```bash
git clone https://github.com/thavlik/vpn-operator
cd vpn-operator
```
2. Apply the Custom Resource Definitions:
```bash
kubectl apply -f crds/
```
3. Install the vpn-operator [Helm chart](https://helm.sh/):
```bash
# Create your chart configuration file.
cat <<EOF | echo "$(</dev/stdin)" > values.yaml
# In this example, we're exposing Prometheus metrics
# for the controllers and enabling PodMonitor creation.
# This is what you would want to do if your cluster
# uses kube-prometheus, a project I highly recommend:
# https://github.com/prometheus-operator/kube-prometheus
prometheus:
  expose: true
  podMonitors: true
EOF

# Install the chart into the `vpn` namespace. Refer to
# chart/values.yaml (or the section below) for details
# on how to configure chart installation.
RELEASE_NAME=vpn
CHART_PATH=chart/
helm install \
  $RELEASE_NAME \
  $CHART_PATH \
  --namespace=vpn \
  --create-namespace \
  -f values.yaml
```

## Usage
1. Create a `Provider` resource and credentials `Secret` with your VPN credentials. Unless you set `spec.verify.skip=true` in the `Provider`, the controller will dial the service with your credentials as a way to automatically test the service end-to-end for you. The expected structure of the credentials `Secret` corresponds to environment variables for a [gluetun](https://github.com/qdm12/gluetun) container. Refer to the [gluetun wiki](https://github.com/qdm12/gluetun/wiki) for provider-specific guides.
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-vpn-credentials
  namespace: default
spec:
  stringData:
  # Environment variables for gluetun, or however you
  # choose to connect to your VPN. Set spec.verify.skip=true
  # in the Provider to disable verification with gluetun.
  # For example, with NordVPN: https://github.com/qdm12/gluetun/wiki/NordVPN
    VPN_SERVICE_PROVIDER: nordvpn
    OPENVPN_USER: myuser@mydomain.com
    OPENVPN_PASSWORD: examplePassword12345
    SERVER_REGIONS: Netherlands # optional
---
apiVersion: vpn.beebs.dev/v1
kind: Provider
metadata:
  name: my-vpn
  namespace: default
spec:
  # In this example, the contractual terms with NordVPN allows up to
  # six devices to be active simultaneously. This field is mandatory.
  # You shouldn't attempt to create an obscene number of connections.
  # Always set it to sane value for your purposes. 
  maxSlots: 6

  # You can optionally specify tag(s) so that Masks have the ability
  # to select this service at the exclusion of others. This Provider
  # will match the tags "default", "preferred", and "my-vpn", which
  # in effect establishes a set of use cases for this Provider.
  # These values can be anything. This field is necessary because a
  # Mask and its desired Provider(s) can be in different namespaces.
  tags:
    - default
    - preferred
    - my-vpn

  # Corresponds to the above Secret's metadata.name
  secret: my-vpn-credentials

  # The controller will attempt to verify that the VPN credentials
  # are correct and the service works. It will do this by injecting
  # the Secret's data as environment variables into a gluetun container
  # and probing an IP service until it returns something different from
  # the initial/unmasked IP address.
  # Note: all of these fields are optional.
  verify:
    # Set to true to bypass credentials verification. This will allow
    # the structure of the Secret to by anything you want, but the
    # Provider will immediately enter the Ready phase without truly
    # knowing if the credentials are ready to use.
    skip: false

    # Amount of time that can elapse before verification is failed.
    timeout: 1m30s

    # You can configure periodic verification here. It's not terribly
    # necessary, but this example will dial the service once a day
    # just to keep the status up to date. This would be most useful
    # if you had a large number of services and you want to automate
    # the process of regularly verifying the credentials are valid.
    # Note that verification will create a Mask in order to reserve
    # a slot with the Provider. Verification will be delayed until
    # a slot is reserved as to avoid exceeding the connection limit. 
    interval: 24h

    # The following enables customization of the verification Pod
    # resource. All of these values are optional, and they are merged
    # onto the default generated.
    overrides:
      pod: # Overrides for the Pod resource.
        metadata:
          labels:
            mylabel: myvalue
      # Overrides for the containers are specified separately.
      # This way you can omit them from the pod override.
      containers:
        # Overrides for the init Container. This container fetches
        # the unmasked IP address from an external service and writes
        # it to /shared/ip for the other containers.
        init:
          image: curlimages/curl:7.88.1
        # Overrides for the VPN Container. This container connects
        # to the VPN service using environment variables from the
        # Provider's credentials Secret. As all containers in a pod
        # share the same network, it will connect all containers
        # to the VPN.
        vpn:
          image: qmcgaw/gluetun:latest
          imagePullPolicy: Always
        # Overrides for the probe Container. This container is
        # responsible for probing the IP service and exiting with
        # code zero when it differs from the initial IP.
        probe:
          image: curlimages/curl:7.88.1
```

2. Make sure the `Provider` enters the `Ready` phase:
```bash
kubectl get provider -Aw
```

If there is an error verifying the credentials, you can view the error details by looking at the `Provider`'s `status.message` field:
```bash
kubectl get provider -A -o yaml
```

3. Create `Mask` resources to reserve slots with the `Provider`:
```yaml
apiVersion: vpn.beebs.dev/v1
kind: Mask
metadata:
  name: my-mask
  namespace: default
spec:
  # You can optionally require the Mask be assigned Providers with
  # specific tags. These value correspond to a Provider's spec.tags
  #providers: ["my-vpn"]
```

4. Wait for the `Mask`'s phase to be `Ready` before using it:
```bash
kubectl get mask -Aw
```
As with the `Provider` resource, the `Mask` also has a `status.message` field that provides a more verbose description of any errors encountered during reconciliation.

5. The `Mask`'s status object contains a reference to the VPN credentials `Secret` created for it at `status.provider.secret`. Plug these values into your sidecar containers (e.g. as environment variables with [gluetun](https://github.com/qdm12/gluetun)).

## Chart configuration (values.yaml)
```yaml
# Container image to use for the controllers.
image: thavlik/vpn-operator:latest

# Pull policy for the controller image.
imagePullPolicy: Always

# Prometheus metrics configuration. See kube-prometheus:
# https://github.com/prometheus-operator/kube-prometheus
prometheus:
  # Run the metrics server with the controllers. This will
  # report on the actions taken as well as how how much
  # time elapses between their read/write phases.
  # All keys are prefixed with 'vpno_'
  expose: true

  # Create PodMonitor resources for the controllers.
  # This value may be false while `expose` is true if you
  # want to scrape the controller pods using another method.
  podMonitors: true

# The Mask and Provider resources have separate Deployments.
# This improves scaling and allows you to configure their
# resource budgets separately.
# Note: the current values are not based on any empirical
# profiling. They are just a starting point and require
# fine-tuning for future releases, but should be more than
# enough for most scenarios.
controllers:
  masks:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 128Mi
        cpu: 100m
  providers:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 128Mi
        cpu: 100m
```

## Notes
### Provider Phase
These are the enum values for the `Provider` resource's `status.phase` field, which summarizes its current state:
- **`Pending`**: The resource first appeared to the controller.
- **`Verifying`**: The credentials are being verified with a [gluetun](https://github.com/qdm12/gluetun) pod.
- **`Verified`**: Verification is complete. The `Provider` will become `Ready` or `Active` upon the next reconciliation.
- **`Ready`**: The service is ready to be used.
- **`Active`**: The service is in use by one or more `Mask` resources.
- **`ErrVerifyFailed`**: The credentials verification process failed.
- **`ErrSecretNotFound`**: The `Secret` resource referenced by `spec.secret` is missing.

### Mask Phase
These are the enum values for the `Mask` resource's `status.phase` field, which summarizes its current state:
- **`Pending`**: The resource first appeared to the controller.
- **`Waiting`**: The resource is waiting for a slot with a `Provider` to become available.
- **`Ready`**: The resource's VPN service credentials are ready to be used. 
- **`Active`**: The resource's VPN service credentials are in use by a `Pod`.
- **`ErrNoProviders`**: No suitable `Provider` resources were found.

### Status message
In addition to `phase`, the status objects of the `Mask` and `Provider` custom resources have a `message` field that is a more verbose description of why the resource is in the current phase. For example, you can view the status object of a `Mask` with `kubectl`:
```bash
kubectl get mask your-mask-name -o jsonpath='{.status}'
```
This information is useful for debugging and should be propogated to the status of any custom resource that depends on a `Mask`.

### Ownership model
Any `Pod` that uses a `Mask` should have a reference to it in [`metadata.ownerReferences`](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/) with `blockOwnerDeletion=true`. This way, the deletion of the `Mask` will be blocked until the `Pod` is deleted, and the `Pod` will automatically be garbage collected when its `Mask` is deleted. The controller also uses this relationship to determine whether a `Mask` is in the `Ready` (not in use) or the `Active` (in use) phase.

Your `Mask` should have an owner reference to your custom resource, and your `Pod` should have owner references to both your `Mask` and the aforementioned custom resource. Your custom resource should be the only owner reference you create with `controller=true`, as your controller is responsible for reconciling your child `Mask` and `Pod` resources. The owner references with `controller=false` exist strictly for garbage collection purposes and, as described above, to determine if the `Mask` is `Ready` or `Active`.

### Garbage collection
Your application is responsible for monitoring the status of your `Mask` and killing the pod if the provider is changed or unassigned. Failing to do so may result in creating more connections than afforded by a `Provider`'s `spec.maxSlots`.

Note: Some VPN services like SurfShark reserve the right to ban you for abusing their generous "unlimited devices" policies. In such cases, it's recommended to use a relatively low `spec.maxSlots` for the `Provider`.

### Scaling
While the controller code is fully capable of concurrent reconciliations, scaling is not as simple as increasing the number of replicas in the deployments. I have ideas for how to scale horizontally, so please open an issue if you encounter problems scaling vertically.

### Custom Resource Definitions (CRDs)
The [CRDs](https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/) for [Mask](crds/vpn.beebs.dev_mask_crd.yaml) and [Provider](crds/vpn.beebs.dev_provider_crd.yaml) are generated by [kube-rs](https://github.com/kube-rs/kube) and include their comments from the [surrounding code](types/src/types.rs). You can view the field descriptions with `kubectl`:
```bash
kubectl get crd providers.vpn.beebs.dev -o yaml
kubectl get crd masks.vpn.beebs.dev -o yaml
```

### Development
Notes on the operator code itself can be found in [operator/README.md](operator/README.md).

### Choosing a VPN service
Some services are more amenable for use with vpn-operator than others. Maximum number of connected devices is an important contractual detail.

It's probably worth paying a premium to have access to a larger pool of IPs across more regions. For example, when using a `Mask` to download a video using a cloaked `Pod` (such as with [ytdl-operator](https://github.com/thavlik/ytdl-operator)), failed attempts due to constantly using banned IPs will slow overall progress more than if the service's bandwidth were reduced. Slow downloads are usually acceptable as long as the service is reliable.

## License
All code in this repository is released under [MIT](LICENSE-MIT) / [Apache 2.0](LICENSE-Apache) dual license, which is extremely permissive. Please open an issue if somehow these terms are insufficient.
