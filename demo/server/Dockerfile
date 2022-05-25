FROM debian:bullseye

COPY sanzu*_amd64.deb /tmp/
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
        openssl \
        socat vim-tiny \
        pulseaudio \
        pavucontrol \
        alsa-utils \
        xinit xvfb xterm procps openbox firefox-esr; \
    dpkg -i /tmp/sanzu_*.deb /tmp/sanzu-broker*deb || true; \
    apt-get -f install ; \
    rm -f /tmp/*.deb; \
    useradd -ms /bin/bash user

COPY server/sanzu.toml /etc/sanzu.toml
COPY server/sanzu_broker.toml /etc/sanzu_broker.toml
COPY server/certs/ /usr/share/doc/sanzu-broker/certs/
COPY server/run-demo-server.sh /usr/local/bin/run-demo-server.sh
COPY server/run_video_server.py  /usr/local/bin/run_video_server.py
COPY server/run_x_env.sh  /usr/local/bin/run_x_env.sh
EXPOSE 11498

USER root
WORKDIR /root/
CMD ["/usr/local/bin/run-demo-server.sh"]
