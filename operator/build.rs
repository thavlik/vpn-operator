use std::fs;
use kube::CustomResourceExt;
use vpn_types::*;

fn main() {
    fs::write("../crds/vpn.beebs.dev_provider_crd.yaml", serde_yaml::to_string(&Provider::crd()).unwrap()).unwrap();
    fs::write("../crds/vpn.beebs.dev_mask_crd.yaml", serde_yaml::to_string(&Mask::crd()).unwrap()).unwrap();
}

