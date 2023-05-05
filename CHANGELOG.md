# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- loop argument renamed to keep-listen
- clipboard_config argument renamed to clipboard
- printdir argument renamed to allow-print
- import_video_shm argument renamed to extern_img_source
- shm_is_xwd argument renamed to source_is_xwd

### Added
- Arguments can be taken from file (--config-path) or environment variables
- Add --proto to the command line
- Add --version to the command line

## [0.1.2] - 2023-03-02

### Added/Changed

- CI: Generating binary release for alma8/alma9/alpine/debian bullseye/windows

### Fixed

- Fix windows sanzu client bad error code handling
- Fix windows memory leak
