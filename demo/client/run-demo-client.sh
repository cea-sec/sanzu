#!/bin/bash
#socat TCP-LISTEN:11498,reuseaddr,fork UNIX-CONNECT:/sanzu_demo/server.sock&
RUST_LOG=info sanzu_client $SANZU_SERVER_IP 11498 --tls_server_name localhost \
    --tls_ca /sanzu_demo/certs/rootCA.crt \
    --client_cert /sanzu_demo/certs/client1.crt \
    --client_key /sanzu_demo/certs/client1.key
