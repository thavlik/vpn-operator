#!/bin/sh
IP_SERVICE="https://api.ipify.org"
IP_FILE_PATH=/tmp/ip
SLEEP_TIME=5s
IP=$(curl -s $IP_SERVICE)
echo $IP > $IP_FILE_PATH
INITIAL_IP=$(cat $IP_FILE_PATH)
echo \"Unmasked IP address is $INITIAL_IP\"
IP=$(curl -s $IP_SERVICE)
# IP service may fail or return the same IP address.
while [ $? -ne 0 ] || [ \"$IP\" = \"$INITIAL_IP\" ]; do
    echo \"Current IP address is $IP, sleeping for $SLEEP_TIME\"
    sleep $SLEEP_TIME
    IP=$(curl -s $IP_SERVICE)
done
echo \"VPN connected. Masked IP address: $IP\"