#!/bin/sh

setxkbmap fr
. /etc/environment

if [ ! -f "${HOME}/.surfrc" ] ; then
  export http_proxy=$http_proxy
  export https_proxy=$https_proxy
  export no_proxy=$no_proxy
  export HTTP_PROXY=$http_proxy
  export HTTPS_PROXY=$https_proxy
  export NO_PROXY=$no_proxy
  pulseaudio -D
  set -e
  openbox&
  firefox-esr

  exit

fi

. "${HOME}/.surfrc"
