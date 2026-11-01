import RatchetKit
import SwiftUI

/// Settings > General > Advanced (development): the AMI ids and the OpenTofu binary and module directory overrides, for
/// working on Ratchet itself. Collapsed by default. Checked while typing, saved as a whole by ratchetd.
struct AdvancedSettingsSection: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var saved = AdvancedSettings()
    @State private var draft = AdvancedSettings()
    @State private var loaded = false
    @State private var failure: String?
    @State private var busy = false

    private var problems: [AdvancedField: String] { SettingsCheck.problems(in: draft) }

    private var isDirty: Bool {
        AdvancedField.allCases.contains {
            draft.value(of: $0).trimmingCharacters(in: .whitespacesAndNewlines) != saved.value(of: $0)
        }
    }

    var body: some View {
        @Bindable var router = router
        Section {
            DisclosureGroup("Advanced (development)", isExpanded: $router.advancedExpanded) {
                Text("Overrides for working on Ratchet itself. Normally the machine images are found by tag for this app version and the OpenTofu module ships inside the app.")
                    .font(.caption).foregroundStyle(.secondary)
                ForEach(AdvancedField.allCases) { field in
                    VStack(alignment: .leading, spacing: 2) {
                        TextField(field.label, text: binding(field), prompt: Text(draft.placeholder(for: field)))
                            .font(.body.monospaced())
                            .autocorrectionDisabled()
                        Text(problems[field] ?? field.hint)
                            .font(.caption)
                            .foregroundStyle(problems[field] == nil ? Color.secondary : Color.red)
                    }
                }
                if let failure { Text(failure).font(.caption).foregroundStyle(.red) }
                HStack {
                    Button("Save") { save() }
                        .disabled(!isDirty || !problems.isEmpty || busy)
                    if isDirty && problems.isEmpty { Text("Unsaved changes").font(.caption).foregroundStyle(.secondary) }
                }
            }
        }
        .task(id: router.advancedExpanded) {
            guard router.advancedExpanded, !loaded else { return }
            await load()
        }
    }

    private func binding(_ field: AdvancedField) -> Binding<String> {
        Binding(get: { draft.value(of: field) }, set: { draft.set($0, for: field) })
    }

    private func load() async {
        do {
            let settings = try await store.client.advancedSettings()
            saved = settings
            draft = settings
            loaded = true
            failure = nil
        } catch {
            failure = error.localizedDescription
        }
    }

    private func save() {
        busy = true
        Task {
            defer { busy = false }
            do {
                let settings = try await store.client.saveAdvancedSettings(draft)
                saved = settings
                draft = settings
                failure = nil
            } catch {
                failure = error.localizedDescription
            }
        }
    }
}
