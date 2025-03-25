# SSHBind

[![Build Status](https://github.com/Xyhlon/SSHBind/actions/workflows/ci.yml/badge.svg)](https://github.com/Xyhlon/SSHBind/actions/workflows/ci.yml)
[![SSHBind on crates.io](https://img.shields.io/crates/v/sshbind.svg)](https://crates.io/crates/sshbind)
[![SSHBind on docs.rs](https://docs.rs/sshbind/badge.svg)](https://docs.rs/sshbind/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://www.tldrlegal.com/license/mit-license)

SSHBind is a Rust library that enables developers to programmatically bind services
located behind multiple SSH connections to a local socket. This facilitates secure and
seamless access to remote services, even those that are otherwise unreachable.

## Features

- **Multiple Jump Host Support**: Navigate through a series of SSH jump hosts to reach
  the target service.
- **Local Socket Binding**: Expose remote services on local Unix sockets, making them
  accessible as if they were running locally.
- **Encrypted Credential Management**: Utilize SOPS-encrypted YAML files for secure and
  reproducible environment configurations.
- **Programmatic Two-Factor Authentication**: Support for TOTP-based 2FA, with plans to
  include user agent support in future releases.
- **Automatic Reconnection**: Seamlessly handle connection interruptions by
  automatically reconnecting to the service.

## Prerequisites

To build and use SSHBind, ensure the following dependencies are installed:

- **OpenSSL**: Provides the necessary cryptographic functions.
- **SOPS**: Used for decrypting the credentials file at runtime.
- **Rust Toolchain**: Required for building the library.

With Nix, building the project is straightforward:

```sh
nix build
```

Alternatively, using Cargo:

```sh
cargo build
```

## Configuration

SSHBind requires an encrypted YAML file containing the credentials for each host. This
file should be encrypted using SOPS to ensure security.

**Sample `secrets.yaml` Structure:**

```yaml
host:
  username: your_username
  password: your_password
  totp_key: optional_base32_totp_key
```

- `host`: The hostname or IP address of the target machine.
- `username`: The SSH username.
- `password`: The SSH password.
- `totp_key`: (Optional) The base-32 encoded key for TOTP 2FA.

Ensure this file is encrypted with SOPS before use.

## Usage

As a library, SSHBind exposes two primary functions:

- `bind`: Establishes the binding of a remote service to a local socket.
- `unbind`: Removes the existing binding.

**Example Usage:**

```rust
use sshbind::{bind, unbind};

fn main() {
    let local_addr = "127.0.0.1:8000";
    let jump_hosts = vec!["jump1:22".to_string(), "jump2:22".to_string()];
    let remote_addr = "remote.service:80";
    let sopsfile = "secrets.yaml";

    // Bind the remote service to the local address
    bind(local_addr, jump_hosts, remote_addr, sopsfile);

    // ... use the bound service ...

    // Unbind when done
    unbind(local_addr);
}
```

For detailed API documentation and advanced usage, refer to the
[official documentation](https://docs.rs/sshbind).

## License

SSHBind is licensed under the MIT License. For more details, see the
[LICENSE](./LICENSE) file.

Dependencies of the code are subject to additional licenses as noted in the
[THIRD_PARTY_LICENSES](./THIRD_PARTY_LICENSES) folder.

## Contributing

Contributions are welcome! To contribute:

1. Fork the repository.
1. Create a new branch.
1. Make your changes.
1. Submit a pull request.

Please ensure your code adheres to the project's coding standards and includes
appropriate tests.
