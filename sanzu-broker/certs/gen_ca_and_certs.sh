#!/bin/sh

# Generate Root ca
echo "Gen Root ca"
openssl genrsa -out rootCA.key 2048
openssl req -x509 -extensions v3_ca -newkey rsa:2048 -key rootCA.key -out rootCA.crt -days 365 -subj /C=US/ST=abc/L=abc/O=test/OU=mine/CN=CA/emailAddress=ca@ca.ca
# -config rootCA.conf

# Generate certificate csr
echo "Gen server cert"
openssl req -new -out localhost.csr -newkey rsa:2048 -nodes -sha256 -keyout localhost.key.temp -config test.conf
openssl rsa -in localhost.key.temp -out localhost.key

# Sign certificate
openssl x509 -req -days 3650 -in localhost.csr  -CA rootCA.crt -CAkey rootCA.key -CAcreateserial -out localhost.crt -extfile test.conf -extensions v3_req


# Gen client req
echo "Gen client cert"
openssl req -newkey rsa:2048 -nodes -keyout client-key1.key.temp -config client.conf -out client-req.csr
openssl rsa -in client-key1.key.temp -out client-key1.key
openssl x509 -req -in client-req.csr -days 1000 -CA rootCA.crt -CAkey rootCA.key -extfile client.conf  -extensions client_reqext   -out client-cert1.pem

