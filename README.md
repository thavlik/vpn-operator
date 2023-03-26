# vpn-operator
> Note that everything is experimental and may change at any time.

[Kubernetes](https://kubernetes.io/) operator for [VPN](https://en.wikipedia.org/wiki/Virtual_private_network) sidecars written in pure [Rust](https://www.rust-lang.org/). This operator simplifies the process of hiding your pods behind one or more VPN services. Instead of assigning the same exact VPN sidecar to every pod you want cloaked, you use the included `Mask` and `MaskProvider` Custom Resources to automate credentials distribution across any number of pods and VPN services.

This project is ideal for applications that need to provision VPN-connected pods, like [ytdl-operator](https://github.com/thavlik/ytdl-operator).

## Installation
1. Clone the repository:
```bash
git clone https://github.com/thavlik/vpn-operator
cd vpn-operator
```
2. Install the [Custom Resource](https://kubernetes.io/docs/concepts/extend-kubernetes/api-extension/custom-resources/) Definitions:
```bash
kubectl apply -f crds/
```
3. Install the vpn-operator [Helm chart](https://helm.sh/):
```bash
# Create your chart configuration file. Generally, you
# will want to place your values.yaml in a separate
# repository, but for this simple example we're putting
# it at the root.
cat <<EOF | echo "$(</dev/stdin)" > values.yaml
# Expose Prometheus metrics for the controllers and
# enable PodMonitor creation. This is what you would
# want to do if your cluster uses kube-prometheus,
# a project I highly recommend: https://github.com/prometheus-operator/kube-prometheus
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
1. Create a `MaskProvider` resource and accompanying `Secret` with your VPN credentials. Unless you set `spec.verify.skip=true` in the `MaskProvider`, the controller will dial the service with your credentials as a way to automatically test the service end-to-end. The expected structure of the credentials `Secret` corresponds to environment variables for a [gluetun](https://github.com/qdm12/gluetun) container. Refer to the [gluetun wiki](https://github.com/qdm12/gluetun/wiki) for provider-specific guides.
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
  # in the MaskProvider to disable verification with gluetun.
  # For example, with NordVPN: https://github.com/qdm12/gluetun/wiki/NordVPN
    VPN_SERVICE_PROVIDER: nordvpn
    OPENVPN_USER: myuser@mydomain.com
    OPENVPN_PASSWORD: examplePassword12345
    SERVER_REGIONS: Netherlands # optional
---
apiVersion: vpn.beebs.dev/v1
kind: MaskProvider
metadata:
  name: my-vpn
  namespace: default
spec:
  # Corresponds to the above Secret's metadata.name
  secret: my-vpn-credentials
  
  # In this example, the contractual terms with NordVPN allows up to
  # six devices to be active simultaneously, and we want to use one
  # less so we can still cloak our local machine's IP when desired.
  # This limit is a requirement. You shouldn't attempt to create an
  # obscene number of connections; most providers likely reserve the
  # right to ban your account for abuse if you try and initialize
  # too many. Always set this to a sane value for your purposes.
  maxSlots: 5

  # You can optionally specify tag(s) so that Masks have the ability
  # to select this service at the exclusion of others. This MaskProvider
  # will match the tags "default", "preferred", and "my-vpn", which
  # in effect establishes a set of use cases for this MaskProvider.
  # These values are arbitrary. This field is necessary because a
  # Mask and its desired MaskProvider(s) can be in different namespaces.
  tags:
    - default
    - preferred
    - my-vpn

  # The controller will attempt to verify that the VPN credentials
  # are correct and the service works. It will do this by injecting
  # the Secret's data as environment variables into a gluetun container
  # and probing an IP service until it returns something different from
  # the initial/unmasked IP address.
  # Note: all of these fields are optional.
  verify:
    # Set to true to bypass credentials verification. This will allow
    # the structure of the Secret to be anything you want, and the
    # MaskProvider will immediately enter the Ready phase without truly
    # knowing if the credentials are valid.
    skip: false

    # Amount of time that can elapse before verification will fail.
    timeout: 1m30s

    # You can configure periodic verification here. It's not terribly
    # necessary, but this example will dial the service once a day
    # just to keep the status up to date. This would be most useful
    # if you had a large number of services and you want to automate
    # the process of regularly verifying the credentials are valid.
    # Note that verification will create a Mask in order to reserve
    # a slot with the MaskProvider. Verification will be delayed until
    # the slot is reserved, as to not exceed `maxSlots` connections. 
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
        # MaskProvider's credentials Secret. As all containers in a Pod
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

2. Make sure the `MaskProvider` enters the `Ready` phase:
```bash
kubectl get maskprovider -Aw
```
If there is an error verifying the credentials, the phase of the `MaskProvider` will be `ErrVerifyFailed` and you can view the error details by looking at its `status.message` field:
```bash
kubectl get maskprovider -A -o yaml
```

3. Create `Mask` resources to reserve slots with the `MaskProvider`:
```yaml
apiVersion: vpn.beebs.dev/v1
kind: Mask
metadata:
  name: my-mask
  # Note the Mask's namespace can differ from an assigned MaskProvider.
  namespace: default
spec:
  # You can optionally require the Mask be assigned MaskProviders with
  # specific tags. These value correspond to a MaskProvider's spec.tags
  # and only one of them has to match.
  #providers: ["my-vpn"]
```

4. The controller will create a `MaskConsumer` resource with the same name/namespace as the `Mask` to manage provider assignment. Any `Pod`, `Job`, or whatever resource that make use of the assigned provider should carry a reference to the `MaskConsumer` (either directly in their `metadata.ownerReference` or indirectly through another owner object) so they will be deleted whenever the provider is unassigned. Wait for the `MaskConsumer`'s phase to be `Ready` before using it:
```bash
kubectl get maskconsumer -Aw
```
As with the `MaskProvider` resource, the `Mask` and `MaskConsumer` resources also have `status.message` fields that provide more verbose descriptions of any errors encountered during reconciliation.

Note: you should always verify the `MaskConsumer` is owned by the `Mask` in question before using it. Failure to do so may result in using the wrong `MaskConsumer` instance, and the number of connections to a `MaskProvider` may exceed the limit specified by its `spec.maxSlots`.

5. The `MaskConsumer`'s status object contains a reference to the VPN credentials `Secret` created for it at `status.provider.secret`. Plug these values into your sidecar containers (e.g. as environment variables into [gluetun](https://github.com/qdm12/gluetun)).

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

# Note: the resource limits are not based on any empirical
# profiling. They are just a starting point and require
# fine-tuning for future releases, but should be more than
# enough for most scenarios.
controllers:
  # The Mask and MaskProvider resources have separate Deployments.
  # This improves scaling and allows you to configure their
  # resource budgets separately.
  masks:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 64Mi
        cpu: 100m
  providers:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 64Mi
        cpu: 100m

  # The MaskReservation controller is for garbage collection.
  # It will delete any MaskReservations that point to MaskConsumers
  # that no longer exist. You never create MaskReservations, the
  # controller manages it automatically.
  reservations:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 64Mi
        cpu: 100m

  # The MaskConsumer controller is used to assign MaskProviders
  # to Masks and provide a way for any consuming resources to be
  # deleted whenever the MaskProvider is unassigned.
  consumers:
    resources:
      requests:
        memory: 32Mi
        cpu: 10m
      limits:
        memory: 64Mi
        cpu: 100m
```

## Notes
### Ownership model
Any `Pod` that uses a `Mask` should reference its `MaskConsumer` in [`metadata.ownerReferences`](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/) with `blockOwnerDeletion=true`. This way, the deletion of the `MaskConsumer` (and the associated `MaskReservation` used to reserve the slot) will be blocked until the `Pod` is deleted, and the `Pod` will automatically be garbage collected when its `MaskConsumer` is deleted.

Your `Mask` should have an owner reference to your custom resource, and your `Pod` should have owner references to your `MaskConsumer` and (optionally) the aforementioned custom resource as well. Your custom resource should be the only owner reference you create with `controller=true`, as your controller is responsible for managing the `Mask` and `Pod` resources it creates. Owner references with `controller=false` exist strictly for garbage collection purposes.

### Performance metrics
These are names and descriptions of [Prometheus](https://prometheus.io/) metrics collected by the controllers:
- **`vpno_masks_reconcile_counter`**: Number of reconciliations by the `Mask` controller.
- **`vpno_masks_action_counter`**: Number of actions taken by the `Mask` controller.
- **`vpno_masks_read_duration_seconds`**: Amount of time taken by the read phase of the `Mask` controller.
- **`vpno_masks_write_duration_seconds`**: Amount of time taken by the write phase of the `Mask` controller.
- **`vpno_providers_reconcile_counter`**: Number of reconciliations by the `MaskProvider` controller.
- **`vpno_providers_action_counter`**: Number of actions taken by the `MaskProvider` controller.
- **`vpno_providers_read_duration_seconds`**: Amount of time taken by the read phase of the `MaskProvider` controller.
- **`vpno_providers_write_duration_seconds`**: Amount of time taken by the write phase of the `MaskProvider` controller.
- **`vpno_reservations_reconcile_counter`**: Number of reconciliations by the `MaskReservation` controller.
- **`vpno_reservations_action_counter`**: Number of actions taken by the `MaskReservation` controller.
- **`vpno_reservations_read_duration_seconds`**: Amount of time taken by the read phase of the `MaskReservation` controller.
- **`vpno_reservations_write_duration_seconds`**: Amount of time taken by the write phase of the `MaskReservation` controller.
- **`vpno_consumers_reconcile_counter`**: Number of reconciliations by the `MaskConsumer` controller.
- **`vpno_consumers_action_counter`**: Number of actions taken by the `MaskConsumer` controller.
- **`vpno_consumers_read_duration_seconds`**: Amount of time taken by the read phase of the `MaskConsumer` controller.
- **`vpno_consumers_write_duration_seconds`**: Amount of time taken by the write phase of the `MaskConsumer` controller.
- **`vpno_http_requests_total`**: Number of HTTP requests made to the metrics server.
- **`vpno_http_response_size_bytes`**: Metrics server HTTP response sizes in bytes.
- **`vpno_http_request_duration_seconds`**: Metrics server HTTP request latencies in seconds.

### Scaling
While the controller code is fully capable of concurrent reconciliations, scaling is not as simple as increasing the number of replicas in the deployments. I have ideas for how to scale horizontally, so please open an issue if you encounter problems scaling vertically. Vertical scaling should be sufficient for at least a few hundred concurrent `Mask` resources.

### Custom Resource Definitions (CRDs)
The [CRDs](https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/) for [`Mask`](crds/vpn.beebs.dev_mask_crd.yaml) and [`MaskProvider`](crds/vpn.beebs.dev_maskprovider_crd.yaml) are generated by [`kube-rs/kube`](https://github.com/kube-rs/kube) and include their comments from the [surrounding code](types/src/types.rs). You can view the field descriptions with `kubectl`:
```bash
kubectl get crd masks.vpn.beebs.dev -o yaml
kubectl get crd maskproviders.vpn.beebs.dev -o yaml
kubectl get crd maskconsumers.vpn.beebs.dev -o yaml
kubectl get crd maskreservations.vpn.beebs.dev -o yaml
```

Note the `MaskReservation` resource is for internal use only by the controller. It holds a cross-namespace reference to the `MaskConsumer` and is used to ensure the `MaskConsumer` is deleted before allowing its slot to be reassigned.

### Uninstallation
For full removal of vpn-operator from your cluster:
```bash
# Remove the chart release first.
helm delete -n vpn vpn

# (Optional) delete the namespace. Assuming you have
# nothing valuable, you can skip `helm delete` and
# begin here, as this will also remove the chart.
kubectl delete namespace vpn

# Delete the Custom Resource Definitions.
kubectl delete crd masks.vpn.beebs.dev
kubectl delete crd maskproviders.vpn.beebs.dev
kubectl delete crd maskconsumers.vpn.beebs.dev
kubectl delete crd maskreservations.vpn.beebs.dev
```

### Development
Notes on the operator code itself can be found in [operator/README.md](operator/README.md).

### Tips for choosing a VPN service
Some services are more amenable for use with vpn-operator than others. Maximum number of connected devices is an important detail.

It's probably worth paying a premium to have access to a larger pool of IPs across more regions. For example, when using a `Mask` to download a video using a cloaked `Pod` (such as with [ytdl-operator](https://github.com/thavlik/ytdl-operator)), failed attempts due to constantly using banned IPs will slow overall progress more than if the service's bandwidth were reduced. Slow downloads are usually acceptable as long as the service is reliable.

## License
All code in this repository is released under [MIT](LICENSE-MIT) / [Apache 2.0](LICENSE-Apache) dual license, which is extremely permissive. Please open an issue if somehow these terms are insufficient.
