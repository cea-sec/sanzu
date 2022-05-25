FROM debian:bullseye

COPY sanzu-client*_amd64.deb /tmp/
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        libasound2 \
        libxcb-shape0 \
        libavutil56 \
        libavcodec58 \
        libasound2 \
        libxcb-render0 \
        libxcb-xfixes0 \
        libdbus-1-3 \
        procps vim-tiny socat; \
    dpkg -i /tmp/sanzu*deb || true; \
    apt-get -f install ; \
    rm -f /tmp/*.deb; \
    useradd -ms /bin/bash user

COPY client/sanzu.toml /etc/
COPY client/run-demo-client.sh /usr/local/bin/run-demo-client.sh

USER user
WORKDIR /home/user
CMD ["/usr/local/bin/run-demo-client.sh"]
