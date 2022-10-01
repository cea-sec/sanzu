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
