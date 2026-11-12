import Foundation

/// Whether the alarm is armed, as returned by all endpoints of the risco-api server
struct Status: Decodable, Equatable, Sendable {
    /// Whether any partition is armed, fully or partially
    let armed: Bool
    let partitions: [Partition]
}

/// A partition (an independently armed area)
struct Partition: Decodable, Equatable, Identifiable, Sendable {
    let id: Int
    let label: String
    let state: Arming
    let ready: Bool
    let alarm: Bool

    var name: String {
        label.isEmpty ? "Partition \(id)" : label
    }
}

enum Arming: String, Decodable, Sendable {
    case disarmed
    case armed
    case partiallyArmed = "partially_armed"
    /// A state added to the server after this app was written
    case unknown

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        self = Arming(rawValue: value) ?? .unknown
    }
}

/// A command to change whether the alarm is armed
enum Command: Equatable, Sendable {
    case arm(partition: Int?)
    case partialArm(partition: Int?)
    case disarm(partition: Int?)

    /// The partition this applies to, or `nil` for all partitions
    var partition: Int? {
        switch self {
        case .arm(let partition), .partialArm(let partition), .disarm(let partition): partition
        }
    }
}

/// The address of a risco-api server
struct Endpoint: Hashable, Sendable, CustomStringConvertible {
    let host: String
    let port: Int
    /// Whether to use HTTPS, such as through `tailscale serve`, rather than plain HTTP
    let secure: Bool
    let url: URL

    /// Returns `nil` if the host is empty or the port is out of range
    init?(host: String, port: Int, secure: Bool) {
        let host = host.trimmingCharacters(in: .whitespaces)
        guard !host.isEmpty, (1...65535).contains(port) else { return nil }

        var components = URLComponents()
        components.scheme = secure ? "https" : "http"
        // IPv6 addresses need brackets in URLs
        components.host = host.contains(":") && !host.hasPrefix("[") ? "[\(host)]" : host
        components.port = port == Self.defaultPort(secure: secure) ? nil : port
        guard let url = components.url else { return nil }

        self.host = host
        self.port = port
        self.secure = secure
        self.url = url
    }

    static func defaultPort(secure: Bool) -> Int {
        secure ? 443 : 80
    }

    /// The URL, such as `http://192.168.1.10:8080` or `https://server.example.ts.net`
    var description: String {
        url.absoluteString
    }
}

/// A client for the JSON API served by risco-api
struct APIClient: Sendable {
    let endpoint: Endpoint

    private static let session = {
        let configuration = URLSessionConfiguration.ephemeral
        // A command can take several round trips to the panel, of up to 10 seconds each
        configuration.timeoutIntervalForRequest = 30
        return URLSession(configuration: configuration)
    }()

    func status() async throws -> Status {
        try await perform(URLRequest(url: endpoint.url.appending(path: "status")))
    }

    func send(_ command: Command) async throws -> Status {
        switch command {
        case .arm(let partition):
            try await post("arm", ArmRequest(partition: partition, mode: "full"))
        case .partialArm(let partition):
            try await post("arm", ArmRequest(partition: partition, mode: "partial"))
        case .disarm(let partition):
            try await post("disarm", DisarmRequest(partition: partition))
        }
    }

    private func post(_ path: String, _ body: some Encodable) async throws -> Status {
        var request = URLRequest(url: endpoint.url.appending(path: path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        return try await perform(request)
    }

    private func perform(_ request: URLRequest) async throws -> Status {
        let (data, response) = try await Self.session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let body = try? JSONDecoder().decode(ErrorResponse.self, from: data)
            throw APIError(status: status, message: body?.error, code: body?.code)
        }

        return try JSONDecoder().decode(Status.self, from: data)
    }
}

/// An error response from the server
struct APIError: LocalizedError {
    let status: Int
    let message: String?
    /// The panel's error code, if the panel refused a command
    let code: String?

    var errorDescription: String? {
        switch (status, code) {
        case (503, _):
            "The server is not connected to the alarm panel. It reconnects by itself, so try again in a moment."
        case (504, _):
            "The alarm panel did not respond in time."
        case (502, _) where message == nil:
            // Not from risco-api, which explains its errors
            "The proxy in front of the server can't reach the risco-api server behind it."
        case (_, "N12"?):
            "Not ready to arm. Check for open doors and windows."
        default:
            message ?? HTTPURLResponse.localizedString(forStatusCode: status)
        }
    }
}

// Unset optionals are left out when encoding, so requests without a partition apply to all

private struct ArmRequest: Encodable {
    let partition: Int?
    let mode: String
}

private struct DisarmRequest: Encodable {
    let partition: Int?
}

private struct ErrorResponse: Decodable {
    let error: String
    let code: String?
}
