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

## Dependencies

To build sanzu, the following packages are required:

* `libkrb5-dev`
* `libpam0g-dev`
* `libavutil-dev`
* `libavformat-dev`
* `libavfilter-dev`
* `libavdevice-dev`
