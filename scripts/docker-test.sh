#!/bin/bash
set -euo pipefail
source $HOME/.nordvpn
echo "Connecting to NordVPN with the following credentials:"
echo "OPENVPN_USER=$OPENVPN_USER"
echo "OPENVPN_PASSWORD=$OPENVPN_PASSWORD"
sleep 5s
docker run \
    -it \
    --rm \
    --cap-add=NET_ADMIN \
    -e VPN_SERVICE_PROVIDER=nordvpn \
    -e "OPENVPN_USER=$OPENVPN_USER" \
    -e "OPENVPN_PASSWORD=$OPENVPN_PASSWORD" \
    -e SERVER_REGIONS=Netherlands \
    qmcgaw/gluetun
