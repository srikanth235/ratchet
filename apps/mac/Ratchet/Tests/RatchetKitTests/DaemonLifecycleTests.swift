import Foundation
import Testing
@testable import RatchetKit

@Suite("Daemon lifecycle")
struct DaemonLifecycleTests {
    @Test func aFirstLaunchRegistersTheAgentThenWaitsForTheSocket() {
        var machine = DaemonLifecycle()
        #expect(machine.phase == .checking)
        #expect(machine.handle(.launched(.notRegistered)) == [.register])
        #expect(machine.phase == .starting)
        #expect(machine.handle(.registered(.enabled)) == [.probe(after: 0)])
        #expect(machine.phase == .starting)
        #expect(machine.handle(.probed(reachable: false)) == [.probe(after: 0.5)])
        #expect(machine.phase == .starting)
        #expect(machine.handle(.probed(reachable: true)) == [.probe(after: 5)])
        #expect(machine.phase == .running)
    }

    @Test func aRegisteredAgentIsJustWaitedFor() {
        var machine = DaemonLifecycle()
        #expect(machine.handle(.launched(.enabled)) == [.probe(after: 0)])
        #expect(machine.phase == .starting)
        _ = machine.handle(.probed(reachable: true))
        #expect(machine.phase.isRunning)
    }

    @Test func approvalInLoginItemsIsPolledUntilGranted() {
        var machine = DaemonLifecycle()
        _ = machine.handle(.launched(.notRegistered))
        #expect(machine.handle(.registered(.requiresApproval)) == [.probe(after: 2)])
        #expect(machine.phase == .needsApproval)
        // The socket is still silent: ask the system again.
        #expect(machine.handle(.probed(reachable: false)) == [.checkRegistration])
        #expect(machine.handle(.registrationChecked(.requiresApproval)) == [.probe(after: 2)])
        #expect(machine.phase == .needsApproval)
        // The user switched it on.
        #expect(machine.handle(.registrationChecked(.enabled)) == [.probe(after: 0)])
        #expect(machine.phase == .starting)
        _ = machine.handle(.probed(reachable: true))
        #expect(machine.phase == .running)
    }

    @Test func aMissingPlistFailsQuietlyButAHandStartedDaemonStillWorks() {
        var machine = DaemonLifecycle()
        _ = machine.handle(.launched(.notFound))
        if case .failed = machine.phase {} else { Issue.record("expected failed, got \(machine.phase)") }
        _ = machine.handle(.probed(reachable: true))
        #expect(machine.phase == .running)
    }

    @Test func aRegistrationErrorIsShownWithItsWords() {
        var machine = DaemonLifecycle()
        _ = machine.handle(.launched(.notRegistered))
        _ = machine.handle(.registrationFailed("Operation not permitted"))
        #expect(machine.phase == .failed("Operation not permitted"))
    }

    @Test func aDaemonThatNeverComesUpFailsAfterTheWait() {
        var machine = DaemonLifecycle()
        _ = machine.handle(.launched(.enabled))
        var commands: [DaemonCommand] = [.probe(after: 0)]
        var probes = 0
        while !commands.isEmpty, probes < 200 {
            probes += 1
            commands = machine.handle(.probed(reachable: false))
        }
        #expect(commands.isEmpty)
        #expect(probes == DaemonLifecycle.maxAttempts + 1)
        if case .failed = machine.phase {} else { Issue.record("expected failed, got \(machine.phase)") }
        // Try again.
        #expect(machine.handle(.startRequested) == [.register])
        #expect(machine.phase == .starting)
        #expect(machine.attempts == 0)
    }

    @Test func aCrashedDaemonIsWaitedForAgain() {
        var machine = DaemonLifecycle(mode: .real, phase: .running)
        #expect(machine.handle(.probed(reachable: true)) == [.probe(after: 5)])
        #expect(machine.handle(.probed(reachable: false)) == [.probe(after: 1)])
        #expect(machine.phase == .starting)
        _ = machine.handle(.probed(reachable: true))
        #expect(machine.phase == .running)
    }

    @Test func stoppingAndRestarting() {
        var machine = DaemonLifecycle(mode: .real, phase: .running)
        #expect(machine.handle(.stopRequested) == [.unregister])
        #expect(machine.phase == .stopped)
        // A probe that was already in flight must not bring it back to life.
        #expect(machine.handle(.probed(reachable: true)).isEmpty)
        #expect(machine.phase == .stopped)
        #expect(machine.handle(.restartRequested) == [.restart])
        #expect(machine.phase == .starting)
        #expect(machine.handle(.registered(.enabled)) == [.probe(after: 0)])
    }

    @Test func demoModeSwitchesTheAgentAndWaitsForTheNewDaemon() {
        var machine = DaemonLifecycle(mode: .real, phase: .running)
        #expect(machine.handle(.modeChanged(.real)).isEmpty)
        #expect(machine.handle(.modeChanged(.demo)) == [.switchMode(.demo)])
        #expect(machine.mode == .demo)
        #expect(machine.phase == .starting)
        #expect(DaemonMode.demo.plistName == "com.ratchet.ratchetd.demo.plist")
        #expect(DaemonMode.real.plistName == "com.ratchet.ratchetd.plist")
        #expect(DaemonMode.demo.other == .real)
    }

    @Test func aRegistrationThatFlipsBackToNotRegisteredFailsUnlessStopped() {
        var machine = DaemonLifecycle(mode: .real, phase: .starting)
        _ = machine.handle(.registrationChecked(.notRegistered))
        if case .failed = machine.phase {} else { Issue.record("expected failed, got \(machine.phase)") }
        var stopped = DaemonLifecycle(mode: .real, phase: .stopped)
        #expect(stopped.handle(.registrationChecked(.notRegistered)).isEmpty)
        #expect(stopped.phase == .stopped)
    }
}
