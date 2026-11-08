# risco-api

An HTTP API for Risco alarm panels: it reports whether the alarm is armed, and can arm and disarm
it. This crate has the API's JSON types, and optionally:

- a server, with the `server` feature, which keeps a connection to the panel open using the
  `risco` crate
- a client, with the `client` feature

See the crate documentation for the endpoints. The API has no authentication, so anyone who can
reach the server can disarm the alarm.

## Running the server

The server is the `risco-api-server` binary, which needs the `server` feature:

```sh
cargo run -p risco-api --features server -- <panel address> <access code> [listen address]
```

The listen address defaults to `127.0.0.1:8080`. Instead of arguments, the settings can be given
as the `RISCO_PANEL`, `RISCO_ACCESS_CODE` and `RISCO_LISTEN` environment variables.

## Installing as a systemd service

[`systemd/risco-api.service`](systemd/risco-api.service) runs the server at startup, and restarts
it if it stops. It runs as an unprivileged user, and reads its settings from
`/etc/default/risco-api`.

### Building

Building on a small device like a Raspberry Pi is slow, so cross-compile the server elsewhere. The
target depends on the device's operating system, which `dpkg --print-architecture` shows on
Debian and Raspberry Pi OS:

| Architecture | Rust target                     | For example                                |
| ------------ | ------------------------------- | ------------------------------------------ |
| `arm64`      | `aarch64-unknown-linux-gnu`     | 64-bit Raspberry Pi OS                     |
| `armhf`      | `armv7-unknown-linux-gnueabihf` | 32-bit Raspberry Pi OS on a Pi 2 or newer  |
| `amd64`      | `x86_64-unknown-linux-gnu`      | A PC                                       |

[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) cross-compiles without a Linux
toolchain:

```sh
rustup target add aarch64-unknown-linux-gnu
cargo zigbuild --release -p risco-api --features server --target aarch64-unknown-linux-gnu
```

This builds `target/aarch64-unknown-linux-gnu/release/risco-api-server`. On a device that can build
it itself, `cargo install --path risco-api --features server --root /usr/local` builds and
installs it in one go.

### Installing

Copy the server and the files in [`systemd/`](systemd) to the device, then:

```sh
sudo install -m 755 risco-api-server /usr/local/bin/
sudo install -m 600 risco-api.env /etc/default/risco-api
sudo install -m 644 risco-api.service /etc/systemd/system/
```

Edit `/etc/default/risco-api` to set the panel's address and access code; it is readable by root
only, since it holds the access code. Then start the service, and enable it to start at boot:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now risco-api
```

To check on it:

```sh
systemctl status risco-api
journalctl -u risco-api -f
curl localhost:8080/status
```

To update the server, install the new binary the same way and run
`sudo systemctl restart risco-api`.

### Stopping

The service is stopped with SIGINT rather than SIGTERM, which the server handles by closing the
connection to the panel properly: after an unclean disconnect, the panel can take a long time to
accept a new connection. If a connection attempt is in progress, stopping waits for it to finish.

### Over Tailscale

To reach the server over Tailscale with HTTPS, as the iPhone app needs, keep the default listen
address and serve it on the tailnet:

```sh
sudo tailscale serve --bg 8080
```

This needs HTTPS certificates enabled for the tailnet. Use `127.0.0.1` rather than `localhost` in
the listen address: `localhost` can resolve to the IPv6 address only, which `tailscale serve`
doesn't connect to.
