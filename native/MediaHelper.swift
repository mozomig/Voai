import AVFoundation
import Foundation

private struct MediaError: Error {
    let message: String
    init(_ message: String) { self.message = message }
}

private func openAsset(_ path: String) throws -> AVURLAsset {
    let url = URL(fileURLWithPath: path)
    guard (try? url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true else {
        throw MediaError("Исходный файл недоступен.")
    }
    // A renamed HLS manifest can otherwise trigger remote segment loading.
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    let prefix = try handle.read(upToCount: 512) ?? Data()
    let header = String(decoding: prefix, as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: "\u{feff}")))
        .lowercased()
    if ["m3u", "m3u8", "pls"].contains(url.pathExtension.lowercased())
        || header.hasPrefix("#extm3u") || header.hasPrefix("[playlist]") {
        throw MediaError("Выберите аудио или видео вместо плейлиста.")
    }
    return AVURLAsset(url: url, options: [
        AVURLAssetPreferPreciseDurationAndTimingKey: true,
        AVURLAssetReferenceRestrictionsKey: AVAssetReferenceRestrictions.forbidAll.rawValue
    ])
}

private let sampleRate: Int32 = 16_000
private func frames(_ time: CMTime) throws -> Int64 {
    let converted = CMTimeConvertScale(time, timescale: sampleRate, method: .roundHalfAwayFromZero)
    guard converted.isNumeric else { throw MediaError("Не удалось прочитать время дорожки.") }
    return converted.value
}

private func report(_ progress: Int) {
    print("{\"progress\":\(progress)}")
    fflush(stdout)
}

// Seek over gaps in presentation timestamps. Sparse regions read back as silence,
// keeping the transcript on the source movie timeline even after an empty edit.
private func append(_ buffer: CMSampleBuffer, to file: FileHandle, cursor: inout Int64,
                    duration: Int64) throws {
    let start = try frames(CMSampleBufferGetPresentationTimeStamp(buffer))
    let count = Int64(CMSampleBufferGetNumSamples(buffer))
    guard count > 0, start < duration else { return }
    let lower = max(start, cursor)
    let upper = min(start + count, duration)
    guard lower < upper else { return }
    guard let data = CMSampleBufferGetDataBuffer(buffer) else {
        throw MediaError("Аудиодорожка содержит нечитаемый фрагмент.")
    }
    if lower > cursor {
        try file.seek(toOffset: UInt64(lower) * 2)
    }
    let length = Int(upper - lower) * 2
    var bytes = Data(count: length)
    let status = bytes.withUnsafeMutableBytes { storage in
        CMBlockBufferCopyDataBytes(data, atOffset: Int(lower - start) * 2,
                                   dataLength: length, destination: storage.baseAddress!)
    }
    guard status == kCMBlockBufferNoErr else { throw MediaError("Не удалось прочитать звук.") }
    try file.write(contentsOf: bytes)
    cursor = upper
}

private func decode(asset: AVURLAsset, destination: String) async throws {
    let tracks = try await asset.loadTracks(withMediaType: .audio)
    guard let track = tracks.first else { throw MediaError("В файле нет аудиодорожки.") }
    let duration = try frames(await asset.load(.duration))
    guard duration > 0, duration <= Int64.max / 2 else {
        throw MediaError("У файла некорректная длительность.")
    }
    let reader = try AVAssetReader(asset: asset)
    let output = AVAssetReaderTrackOutput(track: track, outputSettings: [
        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sampleRate,
        AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false,
        AVLinearPCMIsNonInterleaved: false
    ])
    output.alwaysCopiesSampleData = false
    guard reader.canAdd(output) else { throw MediaError("Формат аудиодорожки не поддерживается.") }
    reader.add(output)
    guard reader.startReading() else { throw MediaError("Не удалось открыть аудиодорожку.") }
    defer { reader.cancelReading() }

    let descriptor = open(destination, O_WRONLY | O_CREAT | O_EXCL, 0o600)
    guard descriptor >= 0 else { throw MediaError("Не удалось создать временную аудиодорожку.") }
    let file = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    var cursor: Int64 = 0
    var progress = -1
    while try autoreleasepool(invoking: {
        guard let buffer = output.copyNextSampleBuffer() else { return false }
        try append(buffer, to: file, cursor: &cursor, duration: duration)
        let current = min(99, Int(Double(cursor) / Double(duration) * 100))
        if current > progress {
            progress = current
            report(progress)
        }
        return true
    }) {}
    guard reader.status == .completed, cursor > 0 else {
        throw MediaError("Не удалось дочитать аудиодорожку.")
    }
    // A movie can continue after its audio track ends. Preserve its full timeline.
    if cursor < duration {
        try file.truncate(atOffset: UInt64(duration) * 2)
    }
    try file.close()
    report(100)
}

@MainActor
private func play(asset: AVURLAsset, from start: Int64, to end: Int64) async throws {
    guard 0 <= start, start < end else { throw MediaError("Некорректный интервал реплики.") }
    guard let track = try await asset.loadTracks(withMediaType: .audio).first else {
        throw MediaError("В файле нет аудиодорожки.")
    }
    // Match the decoder's selected track even if the movie has alternate languages.
    let composition = AVMutableComposition()
    guard let audio = composition.addMutableTrack(withMediaType: .audio,
                                                  preferredTrackID: kCMPersistentTrackID_Invalid) else {
        throw MediaError("Не удалось открыть аудиодорожку.")
    }
    let range = try await track.load(.timeRange)
    let visible = CMTimeRangeGetIntersection(range, otherRange: CMTimeRange(start: .zero,
                                                                             end: try await asset.load(.duration)))
    guard visible.isValid, !visible.isEmpty else { throw MediaError("Аудиодорожка пуста.") }
    try audio.insertTimeRange(visible, of: track, at: visible.start)
    let item = AVPlayerItem(asset: composition)
    let player = AVPlayer(playerItem: item)
    let from = CMTime(value: start, timescale: 1000)
    let until = CMTimeMinimum(CMTime(value: end, timescale: 1000), visible.end)
    guard until.isNumeric, until > from else { throw MediaError("Реплика за пределами записи.") }
    item.forwardPlaybackEndTime = until
    guard await player.seek(to: from, toleranceBefore: .zero, toleranceAfter: .zero) else {
        throw MediaError("Не удалось перейти к реплике.")
    }
    player.play()
    defer { player.pause() }
    var last = from
    var advanced = ContinuousClock.now
    while player.currentTime() < until {
        try await Task.sleep(for: .milliseconds(80))
        let now = player.currentTime()
        if now > last { last = now; advanced = .now }
        if item.status == .failed || advanced.duration(to: .now) > .seconds(8) {
            throw MediaError("Не удалось воспроизвести реплику.")
        }
    }
}

@main
struct MediaHelper {
    static func main() async {
        let args = CommandLine.arguments
        do {
            if args.count == 4, args[1] == "decode" {
                try await decode(asset: openAsset(args[2]), destination: args[3])
            } else if args.count == 5, args[1] == "play",
                      let start = Int64(args[3]), let end = Int64(args[4]) {
                try await play(asset: openAsset(args[2]), from: start, to: end)
            } else {
                throw MediaError("Некорректные параметры медиафайла.")
            }
        } catch {
            // AVFoundation errors can include source paths. Keep diagnostics content-free.
            let message = (error as? MediaError)?.message ?? "Не удалось обработать медиафайл."
            FileHandle.standardError.write(Data((message + "\n").utf8))
            exit(1)
        }
    }
}
