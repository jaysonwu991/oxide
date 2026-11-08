import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("Oxide Computer Use: \(message)\n".utf8))
    exit(1)
}

func address(_ path: String) -> sockaddr_un {
    var value = sockaddr_un()
    value.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: value.sun_path) else { fail("IPC path is too long") }
    value.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    withUnsafeMutableBytes(of: &value.sun_path) { target in target.copyBytes(from: bytes) }
    return value
}

func withAddress<T>(_ path: String, _ body: (UnsafePointer<sockaddr>, socklen_t) -> T) -> T {
    var value = address(path)
    return withUnsafePointer(to: &value) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            body($0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
}

func peerIsCurrentUser(_ socket: Int32) -> Bool {
    var uid: uid_t = 0
    var gid: gid_t = 0
    return getpeereid(socket, &uid, &gid) == 0 && uid == getuid()
}

// LaunchServices starts the permission-owning app. Directly spawning a binary
// inside an .app from Oxide would retain the caller's responsibility chain.
func proxy() -> Never {
    let bundle = Bundle.main.bundleURL
    guard bundle.pathExtension == "app" else { fail("--mcp must run from the built app bundle") }
    var template = Array("/private/tmp/oxide-cu-XXXXXX".utf8CString)
    let directory = template.withUnsafeMutableBufferPointer { buffer -> String in
        guard let path = mkdtemp(buffer.baseAddress!) else { fail("could not create private IPC directory") }
        return String(cString: path)
    }
    let environment = ProcessInfo.processInfo.environment.filter { ["HOME", "COMPUTER_USE_CONFIG"].contains($0.key) }
    let context: [String: Any] = ["environment": environment, "cwd": FileManager.default.currentDirectoryPath]
    do {
        let data = try JSONSerialization.data(withJSONObject: context)
        try data.write(to: URL(fileURLWithPath: directory + "/context.json"))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: directory + "/context.json")
    } catch { fail("could not preserve runtime policy context: \(error)") }
    let path = directory + "/session.sock"
    let listener = socket(AF_UNIX, SOCK_STREAM, 0)
    guard listener >= 0 else { fail("could not create IPC socket") }
    guard withAddress(path, { bind(listener, $0, $1) }) == 0, listen(listener, 1) == 0 else {
        fail("could not listen for helper")
    }
    let launcher = Process()
    launcher.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    launcher.arguments = ["-n", "-g", "-a", bundle.path, "--args", "--serve", path, String(getpid())]
    launcher.standardOutput = FileHandle.standardError
    do { try launcher.run() } catch { fail("could not launch helper: \(error)") }
    launcher.waitUntilExit()
    guard launcher.terminationStatus == 0 else { fail("LaunchServices rejected the helper") }
    var ready = pollfd(fd: listener, events: Int16(POLLIN), revents: 0)
    guard poll(&ready, 1, 15_000) > 0 else { fail("helper did not connect within 15 seconds") }
    let channel = accept(listener, nil, nil)
    close(listener)
    unlink(path)
    unlink(directory + "/context.json")
    rmdir(directory)
    guard channel >= 0, peerIsCurrentUser(channel) else { fail("invalid helper connection") }
    signal(SIGPIPE, SIG_IGN)
    // Closing the transport on cancellation makes the helper's watchdog stop
    // its runtime, even if a GUI operation is still in progress.
    for value in [SIGTERM, SIGINT, SIGHUP] {
        signal(value, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: value, queue: .global())
        source.setEventHandler { shutdown(channel, SHUT_RDWR); exit(0) }
        source.resume()
        retainedSignals.append(source)
    }
    DispatchQueue.global().async {
        while true {
            let data = FileHandle.standardInput.availableData
            if data.isEmpty { shutdown(channel, SHUT_WR); return }
            var offset = 0
            let count = data.count
            let success = data.withUnsafeBytes { bytes -> Bool in
                while offset < count {
                    let written = Darwin.write(channel, bytes.baseAddress!.advanced(by: offset), count - offset)
                    if written < 0 && errno == EINTR { continue }
                    if written <= 0 { return false }
                    offset += written
                }
                return true
            }
            if !success { return }
        }
    }
    let input = FileHandle(fileDescriptor: channel, closeOnDealloc: true)
    while true {
        let data = input.availableData
        if data.isEmpty { exit(0) }
        FileHandle.standardOutput.write(data)
    }
}

var retainedSignals: [DispatchSourceSignal] = []

final class Helper: NSObject, NSApplicationDelegate {
    var status: NSStatusItem!
    var server: Process?
    var channel: Int32 = -1
    var watchdog: Timer?
    var stopping = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        status.button?.image = NSImage(systemSymbolName: "cursorarrow", accessibilityDescription: "Oxide Computer Use")
        status.button?.toolTip = "Oxide Computer Use · ready"
        let menu = NSMenu()
        menu.addItem(withTitle: "Oxide Computer Use", action: nil, keyEquivalent: "")
        menu.addItem(.separator())
        for (title, action) in [
            ("Check permissions", #selector(permissions)),
            ("Grant Accessibility…", #selector(accessibility)),
            ("Grant Screen Recording…", #selector(screenRecording)),
            ("Stop computer use and quit", #selector(stop))
        ] {
            let item = menu.addItem(withTitle: title, action: action, keyEquivalent: "")
            item.target = self
        }
        status.menu = menu
        let args = CommandLine.arguments
        guard args.count == 4, args[1] == "--serve", let parent = Int32(args[3]), parent > 1 else { return }
        let path = args[2]
        let directory = URL(fileURLWithPath: path).deletingLastPathComponent().path
        var info = stat()
        guard path.hasPrefix("/private/tmp/oxide-cu-"), lstat(directory, &info) == 0,
              info.st_uid == getuid(), info.st_mode & 0o777 == 0o700,
              info.st_mode & S_IFMT == S_IFDIR else { fail("invalid IPC directory") }
        // LaunchServices does not inherit the MCP caller's environment or cwd.
        // Preserve policy selection explicitly so a custom deny list is not lost.
        let context: [String: Any]
        do {
            let data = try Data(contentsOf: URL(fileURLWithPath: directory + "/context.json"))
            guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  value["environment"] is [String: String], value["cwd"] is String else {
                fail("invalid runtime policy context")
            }
            context = value
        } catch { fail("could not read runtime policy context: \(error)") }
        channel = socket(AF_UNIX, SOCK_STREAM, 0)
        guard channel >= 0, withAddress(path, { connect(channel, $0, $1) }) == 0,
              peerIsCurrentUser(channel) else { fail("could not connect to Oxide") }
        let runtime = Process()
        runtime.executableURL = Bundle.main.resourceURL!.appendingPathComponent("computer-use")
        runtime.arguments = ["mcp"]
        var environment = ProcessInfo.processInfo.environment
        for (key, value) in context["environment"] as! [String: String] where ["HOME", "COMPUTER_USE_CONFIG"].contains(key) {
            environment[key] = value
        }
        runtime.environment = environment
        runtime.currentDirectoryURL = URL(fileURLWithPath: context["cwd"] as! String, isDirectory: true)
        let transport = FileHandle(fileDescriptor: channel, closeOnDealloc: false)
        runtime.standardInput = transport
        runtime.standardOutput = transport
        runtime.standardError = FileHandle.standardError
        runtime.terminationHandler = { _ in DispatchQueue.main.async { NSApp.terminate(nil) } }
        do { try runtime.run() } catch { fail("could not start bundled runtime: \(error)") }
        server = runtime
        status.button?.toolTip = "Oxide Computer Use · session connected"
        watchdog = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            if Darwin.kill(parent, 0) != 0 && errno == ESRCH { self?.stop() }
        }
    }

    @objc func permissions() {
        let alert = NSAlert()
        alert.messageText = "Oxide Computer Use permissions"
        alert.informativeText = "Accessibility: \(AXIsProcessTrusted() ? "granted" : "missing")\nScreen Recording: \(CGPreflightScreenCaptureAccess() ? "granted" : "missing")\n\nThese grants belong to this helper. Existing Oxide.app grants are separate. Reconnect the MCP server after changing grants."
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
    }

    @objc func accessibility() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
    }

    @objc func screenRecording() { _ = CGRequestScreenCaptureAccess() }

    @objc func stop() {
        if stopping { return }
        stopping = true
        watchdog?.invalidate()
        if channel >= 0 { shutdown(channel, SHUT_RDWR) }
        if let runtime = server, runtime.isRunning {
            runtime.terminate()
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                if runtime.isRunning { Darwin.kill(runtime.processIdentifier, SIGKILL) }
                NSApp.terminate(nil)
            }
        } else { NSApp.terminate(nil) }
    }

    func applicationWillTerminate(_ notification: Notification) {
        watchdog?.invalidate()
        if channel >= 0 { shutdown(channel, SHUT_RDWR); close(channel) }
        if let runtime = server, runtime.isRunning { Darwin.kill(runtime.processIdentifier, SIGKILL) }
    }
}

if CommandLine.arguments.contains("--mcp") { proxy() }
let app = NSApplication.shared
let helper = Helper()
app.setActivationPolicy(.accessory)
app.delegate = helper
app.run()
