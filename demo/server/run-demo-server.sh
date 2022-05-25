#!/bin/bash

if [[ ! -d /sanzu_demo/certs/client-cert1.pem ]]; then
  mkdir /sanzu_demo/certs/
  cd /sanzu_demo/certs/
  cp /usr/share/doc/sanzu-broker/certs/*conf .
  /usr/share/doc/sanzu-broker/certs/gen_ca_and_certs.sh
  chown $USER_UID . client1.*
fi

if [[ ! -z http_proxy ]]
then
  echo "http_proxy=$http_proxy" >> /etc/environment
  echo "https_proxy=$https_proxy" >> /etc/environment
  echo "no_proxy=$no_proxy" >> /etc/environment
fi

while true; do
  #socat TCP:127.0.0.1:11498,retry,forever UNIX-LISTEN:/sanzu_demo/server.sock &
  RUST_LOG=debug /usr/bin/sanzu_broker -p 11498 -f /etc/sanzu_broker.toml -l 0.0.0.0
  pkill socat
done
