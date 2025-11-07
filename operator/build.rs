use std::fs;
use kube::CustomResourceExt;
use vpn_types::*;

fn main() {
    let _ = fs::create_dir("../crds");
    fs::write("../crds/vpn.beebs.dev_mask_crd.yaml", serde_yaml::to_string(&Mask::crd()).unwrap()).unwrap();
    fs::write("../crds/vpn.beebs.dev_maskconsumer_crd.yaml", serde_yaml::to_string(&MaskConsumer::crd()).unwrap()).unwrap();
    fs::write("../crds/vpn.beebs.dev_maskprovider_crd.yaml", serde_yaml::to_string(&MaskProvider::crd()).unwrap()).unwrap();
    fs::write("../crds/vpn.beebs.dev_maskreservation_crd.yaml", serde_yaml::to_string(&MaskReservation::crd()).unwrap()).unwrap();
}

