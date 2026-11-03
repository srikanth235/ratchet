import Observation
import RatchetKit
import SwiftUI

/// What a Connectors row is doing besides showing itself.
enum ConnectorRowMode: Equatable {
    case rename, edit, ssoHint
}

/// The answer of "Check connection", shown under the row.
struct ConnectorNote: Equatable {
    var ok: Bool
    var text: String
}

/// Opens the Add connector sheet on a kind (and, for snapshots, in a state).
struct ConnectorSheetRequest: Identifiable, Equatable {
    let id = UUID()
    var kind: ConnectorKind
    /// GitHub: "Use an access token instead" is open.
    var showsToken = false
    /// AWS: the profile already picked.
    var pick: String?
    /// Agents: the harness whose key is being added (the Agents page's "Add Key…").
    var harness: Harness?
}

/// What the Connectors page is doing besides showing the store's connectors: which rows are being renamed, edited or explained,
/// the last "Check connection" answers, and a request to open the Add connector sheet. Its own object, not the router's: the
/// window's `Router` knows where the window is, not what a page is in the middle of. Snapshot mode and the debug bridge set it.
@Observable
final class ConnectorsPageState {
    var rowModes: [String: ConnectorRowMode] = [:]
    var rowNotes: [String: ConnectorNote] = [:]
    /// Set to open the Add connector sheet; the page takes it and clears it.
    var sheet: ConnectorSheetRequest?

    func reset() {
        rowModes = [:]
        rowNotes = [:]
        sheet = nil
    }
}
