[![Build & test](https://github.com/cea-sec/sanzu/actions/workflows/test.yml/badge.svg)](https://github.com/cea-sec/sanzu/actions/workflows/test.yml)

# Sanzu

Sanzu is a graphical remote desktop solution. It is composed of:

- a server running on Unix or Windows which can stream a X11 or a Windows GUI environment (for now the Unix version is more advanced)
- a client running on Unix or Windows which can read this stream and interact with the GUI environment

It uses modern video codecs like h264/h265 to offer a good image quality and limit its bandwidth consumption. Video compression is done through FFmpeg which allows the use of graphic cards or full featured CPU to achieve fast video compression at low latency. It also allows the use of yuv420 or yuv44 for better graphical details.


This repository contains:
- sanzu: client / server code
- sanzu-broker: a broker for sanzu
- sanzu-common: common code
- demo: demo code to quickly build and run sanzu
- build: docker scripts to build sanzu packages for several distributions

Here is the README which explains how to run the client/server manually: [Sanzu Readme](sanzu/README.md)

Here are some examples: In this case, the remote sanzu server runs under a linux system. Example configuration:
- compression: h264_qsv (intel)
- ffmpeg target bandwidth: 2000000 bits/s
- format: nv12 (yuv420)
- preset: veryfast

Screenshots are in PNG to show original compression details.

Sanzu client running in seamless mode under windows (both windows are from the remote server)
![Alt text](misc/screenshot/sanzu_windows.png?raw=true "Sanzu client running in seamless mode under windows")

Sanzu client running in seamless mode under linux (both windows are from the remote server)
![Alt text](misc/screenshot/sanzu_linux.png?raw=true "Sanzu client running in seamless mode under linux")


## Usefull shortcuts
- Ctrl Alt Shift H: leave keyboard grab mode
- Ctrl Alt Shift C: on clipboard "trigger" mode, the client sends its clipboard value
- Ctrl Alt Shift S: toggle debug statistics on screen
