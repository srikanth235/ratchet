import Foundation

/// The overrides for working on Ratchet itself. Empty means "use the default".
public struct AdvancedSettings: Hashable, Sendable {
    public var runnerAMI: String
    public var benchAMI: String
    public var tofuBinary: String
    public var moduleDir: String
    /// What an empty `tofuBinary` and `moduleDir` resolve to.
    public var defaultTofuBinary: String
    public var defaultModuleDir: String

    public init(
        runnerAMI: String = "", benchAMI: String = "", tofuBinary: String = "", moduleDir: String = "",
        defaultTofuBinary: String = "", defaultModuleDir: String = ""
    ) {
        self.runnerAMI = runnerAMI
        self.benchAMI = benchAMI
        self.tofuBinary = tofuBinary
        self.moduleDir = moduleDir
        self.defaultTofuBinary = defaultTofuBinary
        self.defaultModuleDir = defaultModuleDir
    }

    public func value(of field: AdvancedField) -> String {
        switch field {
        case .runnerAMI: runnerAMI
        case .benchAMI: benchAMI
        case .tofuBinary: tofuBinary
        case .moduleDir: moduleDir
        }
    }

    public mutating func set(_ value: String, for field: AdvancedField) {
        switch field {
        case .runnerAMI: runnerAMI = value
        case .benchAMI: benchAMI = value
        case .tofuBinary: tofuBinary = value
        case .moduleDir: moduleDir = value
        }
    }

    public func placeholder(for field: AdvancedField) -> String {
        switch field {
        case .runnerAMI, .benchAMI: "ami-0123456789abcdef0"
        case .tofuBinary: defaultTofuBinary
        case .moduleDir: defaultModuleDir
        }
    }
}

public enum AdvancedField: String, CaseIterable, Identifiable, Sendable {
    case runnerAMI, benchAMI, tofuBinary, moduleDir

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .runnerAMI: "Runner AMI"
        case .benchAMI: "Bench AMI"
        case .tofuBinary: "OpenTofu binary"
        case .moduleDir: "OpenTofu module directory"
        }
    }

    public var hint: String {
        switch self {
        case .runnerAMI: "Overrides the tag lookup for cloud agent runners."
        case .benchAMI: "Overrides the tag lookup for the evaluator instances."
        case .tofuBinary: "Empty = the bundled one."
        case .moduleDir: "Empty = the bundled infra/aws."
        }
    }
}
