import SwiftUI

@main
struct RiscoApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                .tint(.indigo)
        }
    }
}

/// Asks for the server's address on first launch, and shows the dashboard after that
struct ContentView: View {
    @AppStorage("host") private var host = ""
    @AppStorage("port") private var port = 8080
    @AppStorage("https") private var secure = false
    @State private var editing = false

    var body: some View {
        if let endpoint = Endpoint(host: host, port: port, secure: secure) {
            DashboardView(endpoint: endpoint) { editing = true }
                // A new server gets a fresh model
                .id(endpoint)
                .sheet(isPresented: $editing) {
                    NavigationStack {
                        SettingsView(endpoint: endpoint, onSave: save) { editing = false }
                    }
                }
        } else {
            NavigationStack {
                SettingsView(endpoint: nil, onSave: save)
            }
        }
    }

    private func save(_ endpoint: Endpoint) {
        host = endpoint.host
        port = endpoint.port
        secure = endpoint.secure
        editing = false
    }
}
