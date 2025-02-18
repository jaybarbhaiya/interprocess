# Interprocess
[![Crates.io](https://img.shields.io/crates/v/interprocess)](https://crates.io/crates/interprocess "Interprocess on Crates.io")
[![Docs.rs](https://img.shields.io/badge/documentation-docs.rs-informational)](https://docs.rs/interprocess "interprocess on Docs.rs")
[![Build Status](https://github.com/kotauskas/interprocess/workflows/Checks%20and%20tests/badge.svg)](https://github.com/kotauskas/interprocess/actions "GitHub Actions page for Interprocess")
![maintenance-status](https://img.shields.io/badge/maintenance-actively%20developed-brightgreen)

[![Rust version: 1.66+](https://img.shields.io/badge/rust%20version-1.66+-orange)](https://blog.rust-lang.org/2022/12/15/Rust-1.66.0.html)

Interprocess communication toolkit for Rust programs. The crate aims to expose as many platform-specific features as possible while maintaining a uniform interface for all platforms.

## Features
### Interprocess communication primitives
`interprocess` provides both OS-specific interfaces for IPC and cross-platform abstractions for them.

#### Cross-platform IPC APIs
- **Local sockets** – similar to TCP sockets, but use filesystem or namespaced paths instead of ports on `localhost`, depending on the OS, bypassing the network stack entirely; implemented using named pipes on Windows and Unix domain sockets on Unix

#### Platform-specific, but present on both Unix-like systems and Windows
- **Unnamed pipes** – anonymous file-like objects for communicating privately in one direction, most commonly used to communicate between a child process and its parent

#### Unix-only
- **FIFO files** – special type of file which is similar to unnamed pipes but exists on the filesystem, often referred to as "named pipes" but completely different from Windows named pipes
- **Unix domain sockets** – a type of socket which is built around the standard networking APIs but uses filesystem paths instead of ports on `localhost`, optionally using a spearate namespace on Linux akin to Windows named pipes

#### Windows-only
- **Named pipes** – closely resembles Unix domain sockets, uses a separate namespace instead of on-drive paths

### Asynchronous I/O
Currently, only Tokio for local sockets, Unix domain sockets and Windows named pipes is supported. Support for `async-std` is planned.

## Feature gates
- **`tokio`**, *off* by default – enables support for Tokio-powered efficient asynchronous IPC.

## License
This crate, along with all community contributions made to it, is dual-licensed under the terms of either the [MIT license] or the [Apache 2.0 license].

[MIT license]: https://choosealicense.com/licenses/mit/ " "
[Apache 2.0 license]: https://choosealicense.com/licenses/apache-2.0/ " "
