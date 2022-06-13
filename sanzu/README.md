# Sanzu

## Overview

Sanzu is a graphical remote desktop solution. It is composed of:

- a server running on Unix or Windows which can stream a X11 or a Windows GUI environment (for now the Unix version is more advanced)
- a client running on Unix or Windows which can read this stream and interact with the GUI environment

It uses modern video codecs like h264/h265 to offer a good image quality and limit its bandwidth consumption. Video compression is done through FFmpeg which allows the use of graphic cards or full featured CPU to achieve fast video compression at low latency. It also allows the use of yuv420 or yuv44 for better graphical details.

You can find more information on the architecture in `doc/architecture.md`.

## Features
- Audio
- Copy/Paste
- TLS
- Kerberos
- PAM
- One way clipboard
- Seamless mode

## Options
```
./sanzu_server --help
./sanzu_client --help
./sanzu_proxy --help
```


## Getting started
- You'll need to create a X server (for example on number 100). On the server:
```
  Xvfb :100
```
  You can also customize the screen resolution (here, 1920x1080 in 24 bit/pixel):
```
  Xvfb :100 -screen 0 1920x1080x24
```
- Launch the server:
```
  DISPLAY=:100 sanzu_server -f sanzu.toml
```
  or, if the server implements GPU encoding (h264_nvenc here):
```
  DISPLAY=:100 sanzu_server -f sanzu.toml -e h264_nvenc
```
- On the server side, launch a window manager through X server
```
  DISPLAY=:100 xfwm4
```
- On the client side, launch the client (./sanzu_client <server ip> <server port>):
```
  ./sanzu_client 192.168.0.1 1122
```
By default, sound is disabled. To enable it, server and client should be launched with option "-a".

## Replacement of ssh -Y
If you have a server, let's say Rochefort, which runs a X server on the display :1234, you can access it with:
```
sanzu_client 127.0.0.1 1337 --proxycommand "ssh rochefort DISPLAY=:1234 sanzu_server --stdio"
```
In this case the connection is done throught ssh so ip and port are useless. The server must have the configuration file "/etc/sanzu.toml" present or specified with the "--config" flag.


## Client compilation
### Debian
Packages required:

```
apt install build-essential cargo libasound2-dev ffmpeg libavutil-dev libclang-dev \
    libkrb5-dev libx264-dev libx264-dev libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libxdamage-dev libxext-dev x264 xcb libavformat-dev \
    libavfilter-dev libavdevice-dev
```

### Archlinux
Required packages:

```
pacman -S rust libxtst libxext ffmpeg libxdamage libxfixes pkgconf clang \
    gcc llvm
```

### CentOs
Required packages:

```
yum install alsa-lib-devel ffmpeg-devel compat-libxcb
```

### Windows cross compilation
Allow cargo to cross compile:
```
export PKG_CONFIG_ALLOW_CROSS=1
```
Build with pc-windows-gnu toolchain:
```
build --release  --target "x86_64-pc-windows-gnu"
```

## Testing with TLS
Generate CA / server / client certificates (for test purposes, you can use gen_ca_and_certs.sh)
Add the following example configuration to sanzu.toml:
```
[tls]
server_name = "localhost"
ca_file = "/home/user/certs/rootCA.crt"
auth_cert = "/home/user/certs/localhost.crt"
auth_key = "/home/user/certs/localhost.key"
# allowed_client_domains = ["domain.local"]
```
Run the server:
```
   sanzu_server -f sanzu.toml -p 1122 -l 127.0.0.1
```

And the client:
```
   sanzu_client 127.0.0.1 1144 -a  -t ./certs/rootCA.crt -n localhost
```


## Server configuration file
### video
#### max_fps
This option limits the maximum FPS outputted by the server. This can be useful to limit video throughput in case of fast client & server.
#### max_stall_img
On the server side, if no motion is detected, the encoder is stopped after `max_stall_img` frames count. As most video encoders are progressive, the encoding of a fixed image will enhance the client quality with time. A too low value will result in premature pause in the video encoding process and the video client will be stalled to a bad image quality. A too high value will make the encoder continually consume graphic resources.
#### ffmpeg_options_cmd
Optional: command line to execute each time an encoder is created. It gives a chance to do some tweaks at run time, for example change the process affinity. The output of the command is passed to FFmpeg encoder options. Thus, the command can also tweak the encoder, for example the physical GPU on which the encoder is executed.
#### control_path
Path of a control socket. This socket reacts to client connection by restarting the current encoder. This is used to hot restart video encoder, for example to do some dynamic graphic cards load balancing.
### audio
#### sample_rate
The default sample rate at which the server will capture the sound.
#### max_buffer_ms
The size (in ms) of the sound buffer. The value must be large enough to overcome network lags and avoid sound shuttering. The value must not be too large to avoid desynchronization between image and sound.
### export_video_pci
Used when server is in virtual machine, to export raw frames through shared memory instead of network. Used when the video encoding is done out of the virtual machine.
### ffmpeg
#### globals
FFmpeg video codec options used for all encoders.
#### codec_name
FFmpeg options used for the libx264 codec_name.
Those options can be retrieved from the FFmpeg command line:
`ffmpeg -h encoder=code_cname`

## Known issues
- If connection is flappy, keyboard events might be sent with a delay. Consequence is identical to sticky keys, with input repetition.
