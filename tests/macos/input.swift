// Runs the shipped probe against only this disposable AppKit process.
// swift tests/macos/input.swift /absolute/path/to/Voai.app/Contents/MacOS/voai
import AppKit
import WebKit

guard CommandLine.arguments.count == 2 else { fatalError("Expected path to voai") }
let executable = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
let app = NSApplication.shared
app.setActivationPolicy(.regular)

let window = NSWindow(
  contentRect: NSRect(x: 0, y: 0, width: 470, height: 185),
  styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Voai — проверка прямого ввода"
let first = NSTextView(frame: NSRect(x: 10, y: 100, width: 450, height: 65))
let second = NSTextView(frame: NSRect(x: 10, y: 48, width: 450, height: 44))
let password = NSSecureTextField(frame: NSRect(x: 10, y: 10, width: 450, height: 28))
[first, second, password].forEach { window.contentView?.addSubview($0) }
window.center()
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)

func pump(_ seconds: Double) async {
  try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
}

@MainActor func runProbe(_ scenario: String, afterCapture: (() -> Void)? = nil) async throws
  -> Int32
{
  app.activate(ignoringOtherApps: true)
  window.makeKeyAndOrderFront(nil)
  await pump(0.15)
  let process = Process()
  let log = Pipe()
  process.executableURL = executable
  process.arguments = ["probe-input", "--pid", String(ProcessInfo.processInfo.processIdentifier)]
  var environment = ProcessInfo.processInfo.environment
  environment["VOAI_UI_LANGUAGE"] = "en"
  process.environment = environment
  process.standardError = log
  process.standardOutput = log
  let before = NSPasteboard.general.changeCount
  try process.run()
  var output = ""
  var applied = false
  let deadline = Date().addingTimeInterval(16)
  while process.isRunning {
    await pump(0.035)
    // read(2) on a nonblocking pipe keeps the target application's event loop alive.
    let fd = log.fileHandleForReading.fileDescriptor
    _ = fcntl(fd, F_SETFL, O_NONBLOCK)
    var bytes = [UInt8](repeating: 0, count: 4096)
    let n = read(fd, &bytes, bytes.count)
    if n > 0 { output += String(decoding: bytes.prefix(n), as: UTF8.self) }
    if !applied && output.contains("Test field captured.") {
      applied = true
      afterCapture?()
    }
    if Date() > deadline {
      process.terminate()
      fputs("Timeout: \(scenario)\n", stderr)
      exit(1)
    }
  }
  let rest = try log.fileHandleForReading.readToEnd() ?? Data()
  output += String(decoding: rest, as: UTF8.self)
  print(
    scenario, "exit", process.terminationStatus,
    output.trimmingCharacters(in: .whitespacesAndNewlines))
  if afterCapture != nil { expect(applied, "captured the fixture target for \(scenario)") }
  await pump(0.1)
  expect(NSPasteboard.general.changeCount == before, "pasteboard preserved for \(scenario)")
  return process.terminationStatus
}

let original = "до СЛОВО после"
@MainActor func prepare() async {
  window.makeFirstResponder(first)
  first.string = original
  first.setSelectedRange(NSRange(location: 3, length: 5))
  second.string = "другое поле"
  await pump(0.12)
}
func expect(_ condition: Bool, _ name: String) {
  guard condition else {
    fputs("FAIL: \(name)\n", stderr)
    exit(1)
  }
  print("PASS: \(name)")
}

Task { @MainActor in
  do {
    await prepare()
    app.activate(ignoringOtherApps: true)
    window.makeKeyAndOrderFront(nil)
    await pump(0.25)
    let replacement = try await runProbe("replace")
    expect(
      replacement == 0 && first.string == "до Тест Voai 🎙 после", "Unicode selection replacement")
    await prepare()
    let changed = try await runProbe("focus") { window.makeFirstResponder(second) }
    expect(
      changed != 0 && first.string == original && second.string == "другое поле",
      "same-window focus change")
    await prepare()
    let moved = try await runProbe("caret") {
      first.setSelectedRange(NSRange(location: 0, length: 0))
    }
    expect(moved != 0 && first.string == original, "caret moved")
    window.makeFirstResponder(password)
    password.stringValue = "placeholder"
    await pump(0.1)
    let secure = try await runProbe("secure")
    expect(secure != 0 && password.stringValue == "placeholder", "secure input")
    let web = WKWebView(frame: window.contentView!.bounds)
    window.contentView?.addSubview(web)
    web.loadHTMLString("<textarea autofocus id='edit'>текст в браузере</textarea>", baseURL: nil)
    while web.isLoading { await pump(0.05) }
    window.makeFirstResponder(web)
    _ = try await web.evaluateJavaScript("edit.focus(); edit.setSelectionRange(0, 5); true")
    await pump(0.2)
    let webpage = try await runProbe("webkit")
    let value = try await web.evaluateJavaScript("edit.value") as? String
    expect(webpage == 0 && value == "Тест Voai 🎙 в браузере", "WebKit textarea")
    // Monaco and other web editors can expose dozens of groups above the input.
    let groups = (0..<32).map { "<div role='group' aria-label='Group \($0)'>" }.joined()
    web.loadHTMLString(groups + "<textarea id='edit'>до СЛОВО после</textarea>" + String(repeating: "</div>", count: 32), baseURL: nil)
    while web.isLoading { await pump(0.05) }
    window.makeFirstResponder(web)
    _ = try await web.evaluateJavaScript("edit.focus(); edit.setSelectionRange(3, 8); true")
    await pump(0.2)
    let nested = try await runProbe("nested-web-editor")
    let updated = try await web.evaluateJavaScript("edit.value") as? String
    expect(nested == 0 && updated == "до Тест Voai 🎙 после", "deeply nested editor updated the DOM")
    window.orderOut(nil)
    exit(0)
  } catch {
    fputs("Fixture failed: \(error)\n", stderr)
    exit(1)
  }
}
app.run()
