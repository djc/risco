import SwiftUI

/// Edits the server's address; shown as a welcome screen when no server is set up yet
struct SettingsView: View {
    @State private var host: String
    @State private var port: String
    @State private var secure: Bool
    private let onSave: (Endpoint) -> Void
    /// Set when editing an existing server, rather than setting up the first one
    private let onCancel: (() -> Void)?

    init(endpoint: Endpoint?, onSave: @escaping (Endpoint) -> Void, onCancel: (() -> Void)? = nil) {
        _host = State(initialValue: endpoint?.host ?? "")
        _port = State(initialValue: String(endpoint?.port ?? 8080))
        _secure = State(initialValue: endpoint?.secure ?? false)
        self.onSave = onSave
        self.onCancel = onCancel
    }

    private var endpoint: Endpoint? {
        Int(port).flatMap { Endpoint(host: host, port: $0, secure: secure) }
    }

    private var welcome: Bool {
        onCancel == nil
    }

    var body: some View {
        Form {
            if welcome {
                Section {
                    WelcomeHeader()
                }
                .listRowBackground(Color.clear)
            }

            Section {
                LabeledContent("Address") {
                    // HTTPS certificates are for host names, so expect one rather than an IP
                    TextField(secure ? "server.example.ts.net" : "192.168.1.10", text: $host)
                        .keyboardType(secure ? .URL : .numbersAndPunctuation)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .multilineTextAlignment(.trailing)
                        .submitLabel(.done)
                        .onSubmit(save)
                        .onChange(of: host, takeURL)
                }
                LabeledContent("Port") {
                    TextField("8080", text: $port)
                        .keyboardType(.numberPad)
                        .multilineTextAlignment(.trailing)
                }
                Toggle("HTTPS", isOn: $secure)
                    .onChange(of: secure, switchPort)
            } header: {
                Text("Server")
            } footer: {
                Text(
                    "The risco-api server must listen on an address your iPhone can reach, such "
                        + "as 0.0.0.0:8080. Turn on HTTPS to connect through a proxy such as "
                        + "tailscale serve, using its host name."
                )
            }

            if welcome {
                Section {
                    Button(action: save) {
                        Text("Connect")
                            .font(.headline)
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .disabled(endpoint == nil)
                }
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
            }
        }
        .navigationTitle(welcome ? "" : "Server")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let onCancel {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onCancel)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .disabled(endpoint == nil)
                }
            }
        }
    }

    private func save() {
        if let endpoint {
            onSave(endpoint)
        }
    }

    /// Split a URL entered as the address, like the one `tailscale serve` prints
    private func takeURL() {
        guard let url = URL(string: host),
            let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
            let urlHost = url.host(), !urlHost.isEmpty
        else { return }

        secure = scheme == "https"
        port = String(url.port ?? Endpoint.defaultPort(secure: secure))
        host = urlHost
    }

    /// Switch between the usual ports for the server itself and for an HTTPS proxy
    private func switchPort() {
        switch (secure, port) {
        case (true, "8080"): port = "443"
        case (false, "443"): port = "8080"
        default: break
        }
    }
}

private struct WelcomeHeader: View {
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "lock.shield.fill")
                .font(.system(size: 52, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 104, height: 104)
                .background(Color.indigo.gradient, in: .rect(cornerRadius: 26))
                .shadow(color: .indigo.opacity(0.4), radius: 16, y: 8)
            Text("Connect to Your Alarm")
                .font(.title2.bold())
            Text("Enter the address of the risco-api server on your network.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
    }
}
