# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Fixed
- On windows, keyboard hooks now hooks alt-tab / window.
- Skip keys on sanzu keyboard shortcuts
- TCP timeout with keepalive/tcp_user_timeout for client/server/proxy
- x11 client doesn't need shm
### Changed
- extern-img-source short version is '-z' for all binaries

## [0.1.4] - 2023-05-31

### Changed
- extern-img-source short version is '-k' for all binaries
- remove duplicate '-c' option for client/proxy/server
- rename '--config_path' to '--args-config' for arguments config file
### Fixed
- Remove default listen port for sanzu_proxy (resulting in vsock err)
### Removed
- '-i' short version for "--export-video-pci" for sanzu_server

## [0.1.3] - 2023-05-16

### Changed
- loop argument renamed to keep-listen
- clipboard_config argument renamed to clipboard
- printdir argument renamed to allow-print
- import_video_shm argument renamed to extern_img_source
- shm_is_xwd argument renamed to source_is_xwd
- update to ffmpeg 6.0

### Added
- Arguments can be taken from file (--config-path) or environment variables
- Add --proto to the command line
- Add --version to the command line
- Add --title to customize client window name
- Support 32 bit rgba cursors for x11 sanzu client

### Fixed
- Multiple x11 misuse
- Dependencies listings for alpine/CentOS

## [0.1.2] - 2023-03-02

### Added/Changed

- CI: Generating binary release for alma8/alma9/alpine/debian bullseye/windows

### Fixed

- Fix windows sanzu client bad error code handling
- Fix windows memory leak
