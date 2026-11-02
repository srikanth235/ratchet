import Foundation

/// The Add connector sheet, without the sheet: what the person typed, what is wrong with it, whether it can be sent, and what
/// sending it asks of the store. A value, so the rules are tested without a view.
public struct AddConnectorForm: Equatable, Sendable {
    public var kind: ConnectorKind
    /// GitHub: "Use an access token instead" is open.
    public var showsToken: Bool
    public var token = ""
    /// AWS: the profile picked from `~/.aws`.
    public var awsProfile: String?
    public var region = "us-east-1"
    /// The monthly cap as typed; empty is no cap.
    public var cap = ""
    public var harness: Harness = .claudeCode
    public var apiKey = ""
    public var keyName = ""

    public init(kind: ConnectorKind, showsToken: Bool = false, awsProfile: String? = nil) {
        self.kind = kind
        self.showsToken = showsToken
        self.awsProfile = awsProfile
    }

    /// What pressing the main button does.
    public enum Submission: Equatable, Sendable {
        /// Start the GitHub device flow.
        case githubDeviceFlow
        /// A pasted GitHub token.
        case githubToken(String)
        /// An AWS profile with its region and optional cap, or an agent key.
        case connector(NewConnector)
    }

    // MARK: What is wrong

    public var tokenProblem: String? { token.isEmpty ? nil : SecretKind.githubToken.problem(token) }
    public var keyProblem: String? { apiKey.isEmpty ? nil : harness.secretKind.problem(apiKey) }
    public var regionProblem: String? { SettingsCheck.regionProblem(region) }
    public var capProblem: String? { SettingsCheck.cap(cap).problem }

    // MARK: What the main button says and whether it works

    public var primaryTitle: String {
        switch kind {
        case .github: showsToken ? "Add token" : "Get a code"
        case .aws, .agent: "Add"
        }
    }

    /// `canStore`: the keychain can hold a token or a key. The device flow and an AWS profile need none.
    public func canSubmit(canStore: Bool) -> Bool {
        switch kind {
        case .github:
            return showsToken ? canStore && SecretKind.githubToken.isAcceptable(token) : true
        case .aws:
            return awsProfile != nil && regionProblem == nil && capProblem == nil
                && !region.trimmingCharacters(in: .whitespaces).isEmpty
        case .agent:
            return canStore && harness.secretKind.isAcceptable(apiKey)
        }
    }

    /// What to send, or nil while the form cannot be submitted.
    public func submission(canStore: Bool) -> Submission? {
        guard canSubmit(canStore: canStore) else { return nil }
        switch kind {
        case .github:
            return showsToken ? .githubToken(token) : .githubDeviceFlow
        case .aws:
            guard let awsProfile else { return nil }
            return .connector(NewConnector(
                kind: .aws, name: awsProfile, region: region.trimmingCharacters(in: .whitespaces),
                monthlyCapUSD: SettingsCheck.cap(cap).value))
        case .agent:
            let name = keyName.trimmingCharacters(in: .whitespaces)
            return .connector(NewConnector(kind: .agent, name: name.isEmpty ? harness.title : name, secret: apiKey))
        }
    }

    /// Forgets what must not outlive a submission: the token and the key.
    public mutating func clearSecrets() {
        token = ""
        apiKey = ""
    }
}
