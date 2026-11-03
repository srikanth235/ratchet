import Foundation
import Observation

/// Connectors (GitHub accounts, AWS profiles, agent logins and keys) and the GitHub device flow in progress. The Connectors
/// page reads it and calls its actions; `AppStore` fills it on a refresh and from events.
@MainActor @Observable
public final class ConnectorsStore {
    public internal(set) var connectors: [Connector] = []
    /// The GitHub sign-in in progress: its code is shown until the account appears or the flow ends.
    public internal(set) var githubFlow: GithubSignInFlow?
    /// Whether the keychain can hold tokens and keys, and whether "Sign in with GitHub" has a real client id.
    public internal(set) var connectorSetup = ConnectorSetup()

    private let client: any ConnectorsClient
    private let errors: StoreErrors

    public init(client: any ConnectorsClient, errors: StoreErrors) {
        self.client = client
        self.errors = errors
    }

    public func connectors(of kind: ConnectorKind) -> [Connector] {
        connectors.filter { $0.kind == kind }
    }

    func reset() {
        connectors = []
        githubFlow = nil
    }

    // MARK: Actions

    public func addConnector(_ new: NewConnector) async {
        errors.beginAction()
        do {
            _ = try await client.addConnector(new)
            connectors = try await client.connectors()
        } catch {
            errors.fail(error)
        }
    }

    /// Starts the GitHub device flow (a new account, or `connectorID` again). The code to type appears in `githubFlow`.
    public func startGithubSignIn(connectorID: String? = nil) async {
        errors.beginAction()
        do {
            githubFlow = try await client.startGithubSignIn(connectorID: connectorID)
        } catch {
            errors.fail(error)
        }
    }

    public func cancelGithubSignIn() async {
        errors.beginAction()
        do {
            try await client.cancelGithubSignIn()
            githubFlow = nil
        } catch {
            errors.fail(error)
        }
    }

    public func removeConnector(_ id: String) async {
        errors.beginAction()
        do {
            try await client.removeConnector(id: id)
            connectors = try await client.connectors()
        } catch {
            errors.fail(error)
        }
    }

    public func setDefaultConnector(_ id: String) async {
        errors.beginAction()
        do {
            connectors = try await client.setDefaultConnector(id: id)
        } catch {
            errors.fail(error)
        }
    }

    public func signInConnector(_ id: String) async {
        errors.beginAction()
        // A GitHub sign-in is the device flow again; AWS and agents are checked (their login lives on the Mac).
        if let connector = connectors.first(where: { $0.id == id }), connector.kind == .github {
            await startGithubSignIn(connectorID: id)
            return
        }
        do {
            _ = try await client.signInConnector(id: id)
            connectors = try await client.connectors()
        } catch {
            errors.fail(error)
        }
    }

    /// "Refresh all": every connector is checked with its provider.
    public func refreshConnectors() async {
        errors.beginAction()
        do {
            connectors = try await client.refreshConnectors()
        } catch {
            errors.fail(error)
        }
    }

    // MARK: Connector forms
    // These throw, so a form can show the reason next to its field instead of in the window's error line.

    /// Adds a connector and reports why it could not be added.
    public func addConnectorChecked(_ new: NewConnector) async throws {
        _ = try await client.addConnector(new)
        connectors = try await client.connectors()
    }

    /// A pasted GitHub token: checked against GitHub by the daemon, filed in the keychain, never returned.
    public func addGithubToken(_ token: String, name: String? = nil) async throws {
        connectors = try await client.addGithubToken(token, name: name)
    }

    /// Rename; for AWS the region and the monthly cap too.
    public func updateConnector(_ id: String, _ update: ConnectorUpdate) async throws {
        connectors = try await client.updateConnector(id: id, update)
    }

    /// "Check connection": asks the provider and returns its one-line answer; the list shows the fresh status.
    public func checkConnector(_ id: String) async throws -> ConnectorCheck {
        let check = try await client.checkConnector(id: id)
        connectors = try await client.connectors()
        return check
    }
}
