# Wintun

`wintun.dll` is the official Wintun 0.14.1 driver library for x86_64 Windows
(signed by WireGuard LLC; SHA-256
`e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce`), as
distributed in the `wintun-bindings` crate. Subspace Relay embeds it in the
Windows app, unmodified, and uses it only through its published API to create
the tunnel adapter (see `crates/slipstream-relay/src/tunnel.rs`). Its license
is `LICENSE.txt` (WireGuard LLC's Prebuilt Binaries License).
