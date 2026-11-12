# Risco for iPhone

A SwiftUI app for the HTTP API served by `risco-api`: it shows whether the alarm is armed, and
arms and disarms it. It is built with a Makefile and Xcode's command line tools; there is no
Xcode project. Run `make help` for the targets.

## Requirements

- Xcode, for the iOS SDK and command line tools (`swiftc`, `simctl`, `devicectl`, `actool`).
  The Xcode app itself isn't used.
- The `risco-api` server, listening on an address the iPhone can reach:

  ```sh
  cargo run -p risco-api --features server -- <panel address> <access code> 0.0.0.0:8080
  ```

  The API has no authentication, so anyone on your network can then disarm the alarm.

## Over Tailscale

iOS only allows unencrypted HTTP to local network addresses, which Tailscale's `100.x.y.z`
addresses are not. Instead, serve the API over HTTPS with `tailscale serve`, which needs HTTPS
certificates enabled for the tailnet:

```sh
cargo run -p risco-api --features server -- <panel address> <access code> 127.0.0.1:8000
tailscale serve --bg 8000
```

Then paste the URL `tailscale serve status` shows, like `https://host.tailnet.ts.net`, into the
app's address field; it turns on HTTPS and sets the port. Use `127.0.0.1` rather than `localhost`
for the server: `localhost` can resolve to the IPv6 address only, which `tailscale serve` doesn't
connect to.

## Simulator

Running in the simulator needs an iOS simulator runtime, which can be installed with
`xcodebuild -downloadPlatform iOS`. Then:

```sh
make run-sim SIMULATOR="iPhone 17"   # see `make simulators` for the names
```

The simulator shares the Mac's network, so the app can connect to a server on `127.0.0.1`.

## iPhone

Installing on an iPhone needs a paid Apple Developer account, for an Apple Development
certificate and a development provisioning profile. To set them up on
[the developer portal](https://developer.apple.com/account/resources):

1. Run `make csr`, and upload `signing/development.csr` as a new Apple Development
   certificate. Download the certificate and run `make import-cert CERT=<downloaded .cer>`.
   The private key stays in `signing/`, which is not checked in.
2. Connect the iPhone and turn on Developer Mode, in Settings → Privacy & Security. Register it
   under Devices, with the UDID from `make devices`.
3. Register an App ID with a bundle identifier of your choice, then create an iOS App
   Development profile for it with your certificate and iPhone. Download it into `signing/`.
4. Copy `config.mk.example` to `config.mk` and fill it in.

Then `make run` builds, signs, installs and launches the app. On first launch, the app asks
for permission to access the local network, which it needs to reach the server. `codesign` may
also ask for access to the signing key in the keychain; choose Always Allow.

## Editing in VS Code

With the Swift extension installed, run `make lsp` to write `compile_commands.json`, so that
SourceKit-LSP checks the code against the iOS SDK. The `.sourcekit-lsp/config.json` at the root
of the repository tells it to look for that file in this directory.
