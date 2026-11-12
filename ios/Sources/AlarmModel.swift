import Foundation
import Observation

/// The alarm's status as last reported by the server, and the commands sent to it
@MainActor
@Observable
final class AlarmModel {
    let client: APIClient
    private(set) var status: Status?
    private(set) var updated: Date?
    /// Why the status could not be refreshed
    private(set) var connectionError: String?
    /// The command being sent, if any
    private(set) var pending: Command?
    /// The last command sent, until a status has been fetched after it; no new commands are
    /// sent before then, so they're based on what the command changed
    private(set) var awaiting: Command?
    /// Why the last command failed, until it is acknowledged
    var failure: Failure?
    /// Counts of sent commands, to trigger haptic feedback
    private(set) var succeeded = 0
    private(set) var failed = 0

    /// Incremented for each command, so a refresh started before it doesn't overwrite its result
    private var generation = 0

    init(endpoint: Endpoint) {
        client = APIClient(endpoint: endpoint)
    }

    /// Refresh the status every few seconds, until cancelled
    func poll() async {
        while !Task.isCancelled {
            await refresh()
            try? await Task.sleep(for: .seconds(5))
        }
    }

    func refresh() async {
        guard pending == nil else { return }
        let generation = generation
        do {
            let status = try await client.status()
            if generation == self.generation {
                apply(status)
                awaiting = nil
            }
        } catch {
            if generation == self.generation {
                connectionError = describe(error)
            }
        }
    }

    func send(_ command: Command) async {
        guard awaiting == nil else { return }
        pending = command
        awaiting = command
        generation += 1

        do {
            apply(try await client.send(command))
            succeeded += 1
        } catch {
            let action = switch command {
            case .arm, .partialArm: "Arm"
            case .disarm: "Disarm"
            }
            failure = Failure(title: "Couldn't \(action.lowercased())", message: describe(error))
            failed += 1
        }

        // Even after an error, the command may have taken effect
        pending = nil
        await refresh()
    }

    private func apply(_ status: Status) {
        self.status = status
        updated = .now
        connectionError = nil
    }

    private func describe(_ error: Error) -> String {
        switch error {
        case let error as URLError where Self.unreachable.contains(error.code):
            "Can't reach the server at \(client.endpoint). Check that it is running, and that "
                + "Alarm is allowed to access the local network in Settings."
        case let error as URLError where error.code == .appTransportSecurityRequiresSecureConnection:
            "iOS only allows unencrypted connections to local network addresses, which "
                + "\(client.endpoint.host) is not. Connect over HTTPS instead."
        case let error as URLError where Self.insecure.contains(error.code):
            "Couldn't make a secure connection to \(client.endpoint). Check that it serves HTTPS "
                + "with a certificate for \(client.endpoint.host)."
        case is DecodingError:
            "The server sent a response this app doesn't understand."
        default:
            error.localizedDescription
        }
    }

    /// Errors meaning the server could not be reached, rather than that the request was refused
    private static let unreachable: Set<URLError.Code> = [
        .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost,
        .notConnectedToInternet, .timedOut,
    ]

    /// Errors meaning HTTPS failed, such as when the server only speaks HTTP
    private static let insecure: Set<URLError.Code> = [
        .secureConnectionFailed, .serverCertificateHasBadDate, .serverCertificateUntrusted,
        .serverCertificateHasUnknownRoot, .serverCertificateNotYetValid,
    ]
}

struct Failure: Equatable {
    let title: String
    let message: String
}
