#!/bin/bash

set -eu


usage() {
    echo "Usage: $0 -l [ -p PROXY ] -c -d -h"
    echo "  -h : shows usage"
    echo "  -l : shows the command line for a local client instead of starting a containered one"
    echo "  -c : cleanup containers and temporary files"
    echo "  -d : debug mode"
    echo "  -p PROXY: use a HTTP/HTTPs proxy inside the container"
    exit 0;
}

cleanup() {
    docker kill sanzu-demo-server || true
    docker kill sanzu-demo-client || true
    docker rm sanzu-demo-server || true
    docker rm sanzu-demo-client || true
    docker rm sanzu-builder || true
    docker image rm sanzu-demo-server || true
    docker image rm sanzu-demo-client || true
    docker image rm sanzu-builder || true
    rm -fr /tmp/sanzu_demo.*
    rm -f *.deb
    exit 0
}

localclient=0
debug=0
proxy=

while getopts ":lhcdp:" o; do
    case "${o}" in
    l)
        localclient=1
        ;;
    d)
        debug=1
        ;;
    c)
        cleanup
        ;;
    p)
        proxy=${OPTARG}
        ;;
    h | *)
        usage
        ;;
    esac
done

shift $((OPTIND-1))



OLDPWD=$PWD
if [[ ! -f ../target/debian/sanzu-broker_0.1.0_amd64.deb ]]; then
    cd ../build && make debian
fi
cd $OLDPWD

cp ../target/debian/*.deb .

docker build -t sanzu-demo-server:latest -f server/Dockerfile .
[[ $localclient == 0 ]] && docker build -t sanzu-demo-client:latest -f client/Dockerfile .

TEMPDIR=$(mktemp -d -p /tmp/ sanzu_demo.XXXXXXXXXX)
echo "Using temporary directory ${TEMPDIR}"

DOCKER_ARGS=
if [[ ! -z $proxy ]]
then
  DOCKER_ARGS="--env http_proxy=$proxy --env https_proxy=$proxy --env HTTP_PROXY=$proxy --env HTTPS_PROXY=$proxy"
fi


docker run -d -it --rm -v ${TEMPDIR}:/sanzu_demo/ --env USER_UID=`id -u` -p 11498:11498 $DOCKER_ARGS --name sanzu-demo-server sanzu-demo-server  &
sleep 1
SANZU_SERVER_IP=$(docker inspect -f '{{range.NetworkSettings.Networks}}{{.IPAddress}}{{end}}' sanzu-demo-server)




if [[ $debug == 0 ]]
then

    if [[ $localclient == 0 ]]
    then
        docker run -it --rm -v ${TEMPDIR}:/sanzu_demo/ -u `id -u`:`id -g` \
        -v /tmp/.X11-unix:/tmp/.X11-unix \
        --env SANZU_SERVER_IP=$SANZU_SERVER_IP \
        --env DISPLAY=$DISPLAY \
        -h $HOSTNAME \
        -v $HOME/.Xauthority:/home/user/.Xauthority \
        --name sanzu-demo-client sanzu-demo-client  || true
    else
        echo "RUN client with:"
        echo "sanzu_client localhost 11498 --tls-server-name localhost  --tls-ca  ${TEMPDIR}/certs/rootCA.crt --client-cert  ${TEMPDIR}/certs/client1.crt  --client-key ${TEMPDIR}/certs/client1.key"
        echo "Press ENTER to stop the server"
        read
    fi
else
  echo "Run server with : "
  echo "RUN client with: "
  echo "docker run -it --rm -v ${TEMPDIR}:/sanzu_demo/ -u `id -u`:`id -g`  --env SANZU_SERVER_IP=$SANZU_SERVER_IP --name sanzu-demo-client sanzu-demo-client"
  echo "RUST_LOG=debug sanzu_client localhost 11498 --tls-server-name localhost  --tls-ca  ${TEMPDIR}/certs/rootCA.crt --client-cert  ${TEMPDIR}/certs/client-cert1.pem  --client-key ${TEMPDIR}/certs/client-key1.key"
  docker run -d --rm -v ${TEMPDIR}:/sanzu_demo/ -p 11498:11498 --env USER_UID=`id -u` --name sanzu-demo-server sanzu-demo-server

fi

docker kill sanzu-demo-server sanzu-demo-client || true
docker kill sanzu-demo-server sanzu-demo-server || true

rm -fr "${TEMPDIR}" *.deb

