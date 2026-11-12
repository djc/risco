import SwiftUI

/// Shows whether the alarm is armed, with buttons to change it
struct DashboardView: View {
    @State private var model: AlarmModel
    @Environment(\.scenePhase) private var scenePhase
    private let onChangeServer: () -> Void

    init(endpoint: Endpoint, onChangeServer: @escaping () -> Void) {
        _model = State(initialValue: AlarmModel(endpoint: endpoint))
        self.onChangeServer = onChangeServer
    }

    var body: some View {
        let state = AlarmState(model.status, error: model.connectionError)
        NavigationStack {
            ScrollView {
                VStack(spacing: 28) {
                    StatusHero(state: state, subtitle: subtitle(for: state))

                    if state == .unreachable {
                        Button("Change Server", systemImage: "server.rack", action: onChangeServer)
                            .buttonStyle(.bordered)
                    } else if let error = model.connectionError {
                        // Showing an earlier status, which may be outdated
                        ErrorBanner(message: error)
                    }

                    if let status = model.status {
                        // Disarming also silences an alarm
                        let armed = status.armed || status.partitions.contains(where: \.alarm)
                        CommandButton(
                            armed: armed,
                            busy: model.awaiting != nil && model.awaiting?.partition == nil,
                            send: send
                        )
                        .disabled(model.awaiting != nil)
                    }

                    if let partitions = model.status?.partitions, !partitions.isEmpty {
                        PartitionList(partitions: partitions, awaiting: model.awaiting, send: send)
                            .disabled(model.awaiting != nil)
                    }

                    Footer(endpoint: model.client.endpoint, updated: model.updated)
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 24)
                .animation(.smooth, value: model.status)
                .animation(.smooth, value: model.connectionError)
            }
            .background { Backdrop(color: state.color) }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Server", systemImage: "gearshape", action: onChangeServer)
                }
            }
            .refreshable { await model.refresh() }
        }
        .task { await model.poll() }
        .onChange(of: scenePhase) {
            if scenePhase == .active {
                Task { await model.refresh() }
            }
        }
        .alert(
            model.failure?.title ?? "",
            isPresented: Binding(
                get: { model.failure != nil },
                set: { if !$0 { model.failure = nil } }
            ),
            presenting: model.failure
        ) { _ in
            Button("OK") {}
        } message: { failure in
            Text(failure.message)
        }
        .sensoryFeedback(.success, trigger: model.succeeded)
        .sensoryFeedback(.error, trigger: model.failed)
    }

    private func send(_ command: Command) {
        Task { await model.send(command) }
    }

    private func subtitle(for state: AlarmState) -> String {
        let partitions = model.status?.partitions ?? []
        let armed = partitions.filter { $0.state != .disarmed }.count
        let some = "\(armed) of \(partitions.count) partitions armed"
        return switch state {
        case .connecting: "Connecting to \(model.client.endpoint)…"
        case .unreachable: model.connectionError ?? ""
        case .unknown: "The server reported a state this app doesn't know"
        case .disarmed: partitions.allSatisfy(\.ready) ? "Ready to arm" : "Not ready to arm"
        case .partiallyArmed: armed < partitions.count ? some : "Stay mode"
        case .armed: armed < partitions.count ? some : "Away mode"
        case .alarm:
            "Alarm in " + partitions.filter(\.alarm).map(\.name).formatted(.list(type: .and))
        }
    }
}

/// The large status indicator at the top
private struct StatusHero: View {
    let state: AlarmState
    let subtitle: String

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                Circle()
                    .fill(state.color.gradient)
                    .shadow(color: state.color.opacity(0.45), radius: 28, y: 12)
                Circle()
                    .strokeBorder(.white.opacity(0.25), lineWidth: 6)
                Image(systemName: state.symbol)
                    .font(.system(size: 68, weight: .semibold))
                    .foregroundStyle(.white)
                    .contentTransition(.symbolEffect(.replace))
                    .symbolEffect(.pulse, isActive: state == .alarm)
                    .symbolEffect(.variableColor.iterative, isActive: state == .connecting)
            }
            .frame(width: 172, height: 172)

            VStack(spacing: 6) {
                Text(state.title)
                    .font(.largeTitle.bold())
                    .contentTransition(.opacity)
                Text(subtitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(.top, 20)
        .animation(.smooth, value: state)
        .accessibilityElement(children: .combine)
    }
}

/// The button that arms all partitions, or disarms them if any is armed
private struct CommandButton: View {
    let armed: Bool
    /// Whether to show progress instead of the icon
    let busy: Bool
    let send: (Command) -> Void
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        let (title, symbol, color, command): (String, String, Color, Command) =
            armed
            ? ("Disarm", "lock.open.fill", .green, .disarm(partition: nil))
            : ("Arm", "lock.fill", .indigo, .arm(partition: nil))
        Button {
            send(command)
        } label: {
            HStack(spacing: 12) {
                ZStack {
                    Image(systemName: symbol)
                        .contentTransition(.symbolEffect(.replace))
                        .opacity(busy ? 0 : 1)
                    if busy {
                        ProgressView()
                            .tint(.white)
                    }
                }
                Text(title)
                    .contentTransition(.opacity)
            }
            .font(.title3.weight(.semibold))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 22)
            .foregroundStyle(.white)
            .background(color.gradient, in: .rect(cornerRadius: 22))
            .shadow(color: color.opacity(0.35), radius: 10, y: 5)
            .opacity(enabled || busy ? 1 : 0.55)
        }
        .buttonStyle(PressStyle())
    }
}

private struct PressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.95 : 1)
            .animation(.snappy(duration: 0.2), value: configuration.isPressed)
    }
}

private struct PartitionList: View {
    let partitions: [Partition]
    let awaiting: Command?
    let send: (Command) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Partitions")
                .font(.headline)
                .foregroundStyle(.secondary)
                .padding(.leading, 4)
            ForEach(partitions) { partition in
                PartitionRow(
                    partition: partition,
                    busy: awaiting != nil && awaiting?.partition == partition.id,
                    send: send
                )
            }
        }
    }
}

private struct PartitionRow: View {
    let partition: Partition
    let busy: Bool
    let send: (Command) -> Void

    var body: some View {
        let state = AlarmState(partition)
        HStack(spacing: 14) {
            Image(systemName: state.symbol)
                .font(.body.weight(.semibold))
                .foregroundStyle(state.color)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 44, height: 44)
                .background(state.color.opacity(0.15), in: .circle)

            VStack(alignment: .leading, spacing: 2) {
                Text(partition.name)
                    .font(.headline)
                Text(detail(state))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if busy {
                ProgressView()
                    .frame(width: 44, height: 44)
            } else {
                Menu {
                    Button("Arm", systemImage: "lock.fill") {
                        send(.arm(partition: partition.id))
                    }
                    .disabled(state == .armed)
                    Button("Disarm", systemImage: "lock.open.fill") {
                        send(.disarm(partition: partition.id))
                    }
                    .disabled(state == .disarmed)
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .font(.title2)
                        .foregroundStyle(.secondary)
                        .frame(width: 44, height: 44)
                }
                .accessibilityLabel("Change \(partition.name)")
            }
        }
        .padding(12)
        .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 22))
    }

    private func detail(_ state: AlarmState) -> String {
        guard state == .disarmed else { return state.title }
        return partition.ready ? "Disarmed · Ready" : "Disarmed · Not ready"
    }
}

private struct ErrorBanner: View {
    let message: String

    var body: some View {
        Label {
            Text(message)
        } icon: {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
        }
        .font(.subheadline)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 18))
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}

private struct Footer: View {
    let endpoint: Endpoint
    let updated: Date?

    var body: some View {
        VStack(spacing: 2) {
            Text(endpoint.description)
            if let updated {
                Text("Updated \(Text(updated, style: .relative)) ago")
            }
        }
        .font(.footnote)
        .foregroundStyle(.tertiary)
        .padding(.top, 8)
    }
}

/// A wash of the state's color behind the content
private struct Backdrop: View {
    let color: Color

    var body: some View {
        LinearGradient(
            colors: [color.opacity(0.32), color.opacity(0.08), .clear],
            startPoint: .top,
            endPoint: .center
        )
        .background(Color(.systemGroupedBackground))
        .ignoresSafeArea()
        .animation(.smooth(duration: 0.6), value: color)
    }
}
