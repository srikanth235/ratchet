import Foundation

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

// A blocking client socket on a unix domain path, with just what the daemon client needs. URLSession cannot speak to
// a unix socket; this is plain POSIX, so it behaves the same on macOS and on Linux (where the tests talk to a real
// ratchetd). Blocking reads run on their own thread (see UnixSocketTransport), never on the cooperative pool.

enum UnixSocketError: Error, Equatable, LocalizedError {
    case pathTooLong(String)
    case cannotConnect(path: String, code: Int32)
    case io(operation: String, code: Int32)

    var errorDescription: String? {
        switch self {
        case .pathTooLong(let path):
            "The socket path is too long: \(path)"
        case .cannotConnect(let path, let code):
            "Cannot reach ratchetd at \(path): " + String(cString: strerror(code))
        case .io(let operation, let code):
            "The connection to ratchetd failed (\(operation): " + String(cString: strerror(code)) + ")"
        }
    }
}

/// One connected socket. `shutdown()` may be called from any thread to wake a blocked `read`; only `close()` frees the
/// descriptor, and only once.
final class UnixSocket: @unchecked Sendable {
    private let lock = NSLock()
    private var fd: Int32
    private var isShutdown = false

    private init(fd: Int32) {
        self.fd = fd
    }

    /// Connects to the socket file at `path`. Unix sockets connect (or refuse) at once, so this does not wait.
    static func connect(path: String) throws -> UnixSocket {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        #if os(macOS)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        #endif
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        let bytes = Array(path.utf8CString)  // includes the terminating NUL
        guard bytes.count <= capacity else { throw UnixSocketError.pathTooLong(path) }
        withUnsafeMutablePointer(to: &address.sun_path) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: capacity) { destination in
                for (index, byte) in bytes.enumerated() { destination[index] = byte }
            }
        }

        let descriptor = posixSocket()
        guard descriptor >= 0 else { throw UnixSocketError.io(operation: "socket", code: errno) }
        #if canImport(Darwin)
        // A write to a closed socket must fail with EPIPE, not kill the app with SIGPIPE.
        var one: Int32 = 1
        _ = setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        #endif
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
                posixConnect(descriptor, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard result == 0 else {
            let code = errno
            _ = posixClose(descriptor)
            throw UnixSocketError.cannotConnect(path: path, code: code)
        }
        return UnixSocket(fd: descriptor)
    }

    /// True when something is listening at `path` right now.
    static func canConnect(path: String) -> Bool {
        guard let socket = try? connect(path: path) else { return false }
        socket.close()
        return true
    }

    /// Writes all of `bytes`.
    func writeAll(_ bytes: [UInt8]) throws {
        let descriptor = try openDescriptor()
        var offset = 0
        while offset < bytes.count {
            let written = bytes.withUnsafeBufferPointer { buffer -> Int in
                guard let base = buffer.baseAddress else { return 0 }
                return posixSend(descriptor, base + offset, buffer.count - offset)
            }
            if written < 0 {
                if errno == EINTR { continue }
                throw UnixSocketError.io(operation: "write", code: errno)
            }
            offset += written
        }
    }

    /// Reads up to `maxBytes`. An empty result means the other side closed the connection.
    func read(maxBytes: Int = 16 * 1024) throws -> [UInt8] {
        let descriptor = try openDescriptor()
        var buffer = [UInt8](repeating: 0, count: maxBytes)
        while true {
            let count = buffer.withUnsafeMutableBytes { raw in
                posixRead(descriptor, raw.baseAddress, maxBytes)
            }
            if count < 0 {
                if errno == EINTR { continue }
                if isShutDownByUs { return [] }
                throw UnixSocketError.io(operation: "read", code: errno)
            }
            return Array(buffer[0..<count])
        }
    }

    /// Wakes a blocked `read` (it returns no bytes) and stops further traffic. Safe to call from any thread, any number
    /// of times.
    func shutdown() {
        lock.lock()
        defer { lock.unlock() }
        guard fd >= 0, !isShutdown else { return }
        isShutdown = true
        _ = posixShutdown(fd)
    }

    /// Frees the descriptor. Call once, from the thread that reads.
    func close() {
        lock.lock()
        defer { lock.unlock() }
        guard fd >= 0 else { return }
        _ = posixClose(fd)
        fd = -1
    }

    private var isShutDownByUs: Bool {
        lock.lock()
        defer { lock.unlock() }
        return isShutdown
    }

    private func openDescriptor() throws -> Int32 {
        lock.lock()
        defer { lock.unlock() }
        guard fd >= 0 else { throw UnixSocketError.io(operation: "use", code: EBADF) }
        return fd
    }
}

// MARK: - Calls into the C library
// These are free functions so that the methods above (`connect`, `read`, `close`, `shutdown`) do not shadow them.

private func posixSocket() -> Int32 {
    #if os(Linux)
    return socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
    #else
    return socket(AF_UNIX, SOCK_STREAM, 0)
    #endif
}

private func posixConnect(_ descriptor: Int32, _ address: UnsafePointer<sockaddr>, _ length: socklen_t) -> Int32 {
    connect(descriptor, address, length)
}

private func posixRead(_ descriptor: Int32, _ buffer: UnsafeMutableRawPointer?, _ count: Int) -> Int {
    read(descriptor, buffer, count)
}

private func posixSend(_ descriptor: Int32, _ buffer: UnsafePointer<UInt8>, _ count: Int) -> Int {
    #if os(Linux)
    return send(descriptor, buffer, count, Int32(MSG_NOSIGNAL))
    #else
    return send(descriptor, buffer, count, 0)
    #endif
}

private func posixShutdown(_ descriptor: Int32) -> Int32 {
    shutdown(descriptor, 2)  // SHUT_RDWR
}

private func posixClose(_ descriptor: Int32) -> Int32 {
    close(descriptor)
}
