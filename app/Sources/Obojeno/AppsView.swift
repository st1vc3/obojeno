import SwiftUI

/// The apps obojeno themes, as a settings group: a switch each, and what is
/// left to do for the ones that need a one-time setup.
struct AppsSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        SectionTitle("Apps")
        SettingsGroup {
            ForEach(Array(model.targets.enumerated()), id: \.element.id) { index, target in
                if index > 0 { RowDivider() }
                TargetRow(
                    target: target,
                    isOn: Binding(get: { target.enabled }, set: { model.setTarget(target, enabled: $0) })
                ) { option, value in
                    model.setTarget(target, enabled: true, options: [option: value])
                }
            }
        }
        Caption(
            "obojeno writes each app's colors and reloads it. Some apps need a line added to their config once, which obojeno can do for you."
        )
    }
}

/// One app: its name, a switch, and its state when that needs attention.
struct TargetRow: View {
    @Environment(AppModel.self) private var model
    let target: TargetInfo
    let isOn: Binding<Bool>
    /// Values chosen but not saved yet (on the first run).
    var chosen: [String: String] = [:]
    /// Gets a value for the option the target needs (a folder).
    let choose: @MainActor (String, String) -> Void

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(target.title)
                if let status {
                    Text(status.text)
                        .font(.caption)
                        .foregroundStyle(status.attention ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                }
            }
            Spacer(minLength: 8)
            if let missing, target.installed {
                Button("Choose…") {
                    model.chooseFolder(for: target, option: missing) { choose(missing, $0) }
                }
                .buttonStyle(CapsuleButtonStyle())
            } else if target.snippet != nil || target.setUp == false {
                SetupButton(target: target, attention: status?.attention == true)
            }
            Toggle(target.title, isOn: isOn)
                .toggleStyle(.switch)
                .labelsHidden()
                .disabled(!isOn.wrappedValue && (!target.installed || missing != nil))
        }
        .frame(minHeight: 44)
        .padding(.vertical, status == nil ? 0 : 4)
        .opacity(target.installed || isOn.wrappedValue ? 1 : 0.55)
    }

    /// The option still without a value.
    private var missing: String? {
        target.missing.flatMap { chosen[$0] == nil ? $0 : nil }
    }

    private var status: (text: String, attention: Bool)? {
        if !target.installed { return ("Not found on this Mac", false) }
        if let missing { return ("Choose its \(missing) folder to theme it", true) }
        if let value = target.missing.flatMap({ chosen[$0] }) { return (value, false) }
        if isOn.wrappedValue, target.setUp == false { return ("Needs a one-time setup", true) }
        return nil
    }
}

/// Shows how to set an app up, with its snippet to copy.
private struct SetupButton: View {
    @Environment(AppModel.self) private var model
    let target: TargetInfo
    let attention: Bool
    @ViewState private var showing = false

    var body: some View {
        Button {
            showing.toggle()
        } label: {
            Image(systemName: attention ? "exclamationmark.circle.fill" : "info.circle")
                .foregroundStyle(attention ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
        }
        .buttonStyle(.plain)
        .help("How to set up \(target.title)")
        .popover(isPresented: $showing, arrowEdge: .trailing) {
            SetupSteps(target: target)
                .padding(14)
                .frame(width: 340)
        }
    }
}

/// An app's setup text and its snippet, with a Copy button, and one that
/// adds the snippet when obojeno can.
struct SetupSteps: View {
    @Environment(AppModel.self) private var model
    let target: TargetInfo
    /// On the first run's last page: says where obojeno adds the snippet
    /// once the user confirms, instead of offering to add it now.
    var confirming = false
    @ViewState private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(target.title).font(.headline)
            Text(description)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            if let snippet = target.snippet {
                Text(snippet)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.black.opacity(0.3), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                if let added {
                    Text(added)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack {
                    Spacer()
                    if !confirming, target.canSetUp {
                        Button("Add for Me") { model.setUp(target) }
                            .buttonStyle(CapsuleButtonStyle())
                            .help("Add this to \(target.setupFile ?? "the config"), keeping a copy of the file")
                    }
                    Button(copied ? "Copied" : "Copy") {
                        model.copy(snippet)
                        copied = true
                    }
                    .buttonStyle(CapsuleButtonStyle())
                }
            }
        }
    }

    private var description: String {
        if confirming, target.canSetUp, let file = target.setupFile {
            return "obojeno adds this to \(file):"
        }
        return target.setup
    }

    /// Where the snippet went, and the copy of the file from before.
    private var added: String? {
        guard let result = model.setupResults[target.name], let file = result.file else { return nil }
        guard let backup = result.backup else { return "Added to \(file)." }
        return "Added to \(file). The file from before is \(backup)."
    }
}

/// The first run: choose the apps to theme, then see what is left to set up.
struct WelcomeView: View {
    @Environment(AppModel.self) private var model
    @ViewState private var chosen: Set<String>?
    /// Folders chosen for targets that need one, by target.
    @ViewState private var options: [String: [String: String]] = [:]
    @ViewState private var showingSteps = false

    var body: some View {
        let found = model.targets.filter(\.installed)
        let selection = chosen ?? Set(found.filter { $0.missing == nil }.map(\.name))
        // The buttons stay in view below whatever scrolls.
        VStack(alignment: .leading, spacing: 14) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if showingSteps {
                        steps(for: selection)
                    } else {
                        choose(found: found, selection: selection)
                    }
                }
                // Clear of the fade once scrolled to the end.
                .padding(.bottom, 16)
            }
            .scrollIndicators(.never)
            // What scrolls fades out above the buttons instead of ending at
            // a hard edge.
            .mask {
                VStack(spacing: 0) {
                    Rectangle()
                    LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom)
                        .frame(height: 16)
                }
            }
            HStack {
                if showingSteps {
                    stepsButtons(for: selection)
                } else {
                    chooseButtons(for: selection)
                }
            }
        }
    }

    @ViewBuilder private func choose(found: [TargetInfo], selection: Set<String>) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Welcome to obojeno").font(.title2.weight(.bold)).legibleOnGlass()
            Text(
                "Pick a wallpaper and your apps take on its colors. These are the apps found on this Mac; choose the ones to theme."
            )
            .font(.callout)
            .foregroundStyle(.white.opacity(0.85))
            .legibleOnGlass()
            .fixedSize(horizontal: false, vertical: true)
        }
        if found.isEmpty {
            Caption("No apps obojeno knows were found. You can add them later in Settings.")
        } else {
            SettingsGroup {
                ForEach(Array(found.enumerated()), id: \.element.id) { index, target in
                    if index > 0 { RowDivider() }
                    TargetRow(
                        target: target,
                        isOn: Binding(
                            get: { selection.contains(target.name) },
                            set: { on in
                                var next = selection
                                if on { next.insert(target.name) } else { next.remove(target.name) }
                                chosen = next
                            }),
                        chosen: options[target.name] ?? [:]
                    ) { option, value in
                        options[target.name, default: [:]][option] = value
                        chosen = selection.union([target.name])
                    }
                }
            }
        }
        let missing = model.targets.filter { !$0.installed }.map(\.title)
        if !missing.isEmpty {
            Caption("Not found: \(missing.joined(separator: ", ")). You can change all of this later in Settings.")
        }
    }

    @ViewBuilder private func chooseButtons(for selection: Set<String>) -> some View {
        Spacer()
        Button(needsSteps(selection).isEmpty ? "Start" : "Continue") {
            if needsSteps(selection).isEmpty {
                model.finishWelcome(enabling: selection, options: options, linking: [])
            } else {
                withAnimation(.snappy) { showingSteps = true }
            }
        }
        .buttonStyle(CapsuleButtonStyle())
        .keyboardShortcut(.defaultAction)
    }

    @ViewBuilder private func steps(for selection: Set<String>) -> some View {
        let targets = needsSteps(selection)
        let linking = linking(selection)
        VStack(alignment: .leading, spacing: 6) {
            Text("Almost there").font(.title2.weight(.bold)).legibleOnGlass()
            Text(intro(targets: targets.count, linking: linking.count))
                .font(.callout)
                .foregroundStyle(.white.opacity(0.85))
                .legibleOnGlass()
                .fixedSize(horizontal: false, vertical: true)
        }
        ForEach(targets) { target in
            SetupSteps(target: target, confirming: true)
                .padding(14)
                .glassEffect(.regular, in: .rect(cornerRadius: 16))
        }
    }

    @ViewBuilder private func stepsButtons(for selection: Set<String>) -> some View {
        let linking = linking(selection)
        Button("Back") { withAnimation(.snappy) { showingSteps = false } }
            .buttonStyle(CapsuleButtonStyle())
        Spacer()
        if !linking.isEmpty {
            Button("Skip") { model.finishWelcome(enabling: selection, options: options, linking: []) }
                .buttonStyle(CapsuleButtonStyle())
                .help("Start without changing these files")
        }
        // No Return shortcut: the Return that pressed Continue would press
        // this too, skipping the confirmation before files change.
        Button(linking.isEmpty ? "Start" : "Add and Start") {
            model.finishWelcome(enabling: selection, options: options, linking: linking)
        }
        .buttonStyle(CapsuleButtonStyle())
    }

    private func intro(targets: Int, linking: Int) -> String {
        let need = "These apps need a line in their config once, so they read obojeno's colors."
        let backup = "and keeps each file from before next to it, ending in .obojeno-backup."
        if linking == 0 { return need }
        if linking == targets { return "\(need) obojeno adds it, \(backup)" }
        return "\(need) obojeno adds it where it can write, \(backup) Add the others yourself."
    }

    /// The chosen apps whose setup obojeno does on confirmation.
    private func linking(_ selection: Set<String>) -> [String] {
        needsSteps(selection).filter(\.canSetUp).map(\.name)
    }

    /// Chosen apps known not to be set up, or that may need it; the ones
    /// obojeno sets up first.
    private func needsSteps(_ selection: Set<String>) -> [TargetInfo] {
        let targets = model.targets.filter { selection.contains($0.name) && $0.snippet != nil && $0.setUp != true }
        return targets.filter(\.canSetUp) + targets.filter { !$0.canSetUp }
    }
}
