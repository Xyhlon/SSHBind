# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2025-01-26

### Added

- GitHub Actions workflow for publishing to crates.io
- GitHub Actions workflow for automated releases with release-plz
- CD/CI pipeline for continuous deployment

## [0.1.0] - 2024-04-09

### Added

- CLI daemon architecture with local socket IPC
- Background process management with semaphores
- Session healing and automatic reconnection on failure
- Monitoring task for connection health
- Improved real-world testing scenarios
- NixOS integration tests (simple, cli, performance)
- Low session bandwidth testing with iperf3
- HTTP load testing capabilities

### Fixed

- CI problems for macOS and Windows
- Parallel integration test execution
- Rust integration testing issues
- Idle CPU utilization issue with runtime yielding
- Session rebuild functionality upon connection death
- Async I/O implementation improvements

### Changed

- Restructured code to reduce function argument size
- Reworked CI pipeline with Nix caching
- Switched to async-io for better performance
- Updated all dependencies

## [0.0.3] - 2024-03-15

### Added

- TOTP-based two-factor authentication support
- Encrypted credential management via SOPS
- YAML configuration file support
- SSH jump host chain support

### Fixed

- Connection stability improvements
- Memory leak in long-running connections

## [0.0.2] - 2024-02-20

### Added

- Initial library implementation with `bind()` and `unbind()` functions
- Basic SSH connection management
- Socket binding and bidirectional data copying
- Basic authentication support (password and public key)

### Changed

- Improved error handling throughout the codebase
- Better logging with env_logger

## [0.0.1] - 2024-02-01

### Added

- Initial project structure
- Basic SSH connection functionality
- MIT License
- README documentation

[0.0.1]: https://github.com/Xyhlon/SSHBind/releases/tag/v0.0.1
[0.0.2]: https://github.com/Xyhlon/SSHBind/compare/v0.0.1...v0.0.2
[0.0.3]: https://github.com/Xyhlon/SSHBind/compare/v0.0.2...v0.0.3
[0.1.0]: https://github.com/Xyhlon/SSHBind/compare/v0.0.3...v0.1.0
[0.1.1]: https://github.com/Xyhlon/SSHBind/compare/v0.1.0...v0.1.1
[unreleased]: https://github.com/Xyhlon/SSHBind/compare/v0.1.1...HEAD
