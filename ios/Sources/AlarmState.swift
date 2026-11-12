import SwiftUI

/// The state of the alarm or a partition, as shown to the user
enum AlarmState: Equatable {
    case connecting
    case unreachable
    case unknown
    case disarmed
    case partiallyArmed
    case armed
    case alarm

    /// The overall state, from the most urgent state of any partition
    init(_ status: Status?, error: String?) {
        guard let status else {
            self = error == nil ? .connecting : .unreachable
            return
        }

        let states = status.partitions.map(AlarmState.init)
        self = [.alarm, .armed, .partiallyArmed, .unknown].first(where: states.contains)
            ?? (status.armed ? .armed : .disarmed)
    }

    init(_ partition: Partition) {
        self = if partition.alarm {
            .alarm
        } else {
            switch partition.state {
            case .disarmed: .disarmed
            case .armed: .armed
            case .partiallyArmed: .partiallyArmed
            case .unknown: .unknown
            }
        }
    }

    var title: String {
        switch self {
        case .connecting: "Connecting"
        case .unreachable: "Unavailable"
        case .unknown: "Unknown"
        case .disarmed: "Disarmed"
        case .partiallyArmed: "Partially Armed"
        case .armed: "Armed"
        case .alarm: "Alarm"
        }
    }

    var symbol: String {
        switch self {
        case .connecting: "antenna.radiowaves.left.and.right"
        case .unreachable: "wifi.exclamationmark"
        case .unknown: "shield.lefthalf.filled"
        case .disarmed: "lock.open.fill"
        case .partiallyArmed: "house.fill"
        case .armed: "lock.shield.fill"
        case .alarm: "light.beacon.max.fill"
        }
    }

    var color: Color {
        switch self {
        case .connecting, .unreachable, .unknown: .gray
        case .disarmed: .green
        case .partiallyArmed: .orange
        case .armed: .indigo
        case .alarm: .red
        }
    }
}
