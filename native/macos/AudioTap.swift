// PC の音声を Core Audio process tap(macOS 14.2 以降)で取得し、PCM を stdout へ流す補助プログラム。
//
// 音声はディスクに書かない。stdin が閉じる(親が止めた、または親が落ちた)か、stdout が閉じると終了する。
//
// stdout の形式は、次のパケットの繰り返し。数値はすべてリトルエンディアン。
//   "ZTAP" | サンプルレート u32 | チャンネル数 u32 | 本体のバイト数 u32 | 本体(インターリーブの f32)
// 取得を始められたら、本体が 0 バイトのパケットを 1 つ出す。親はこれで開始の成否を知る。
//
// 使い方:
//   zuchi-audio-tap                               取得を始める
//   zuchi-audio-tap --check-permission mic|system 権限の状態を 1 語で出力する(読むだけ)

import CoreAudio
import Foundation

// 終了コード。Rust 側と揃える。
let exitUsage: Int32 = 64
let exitUnsupportedFormat: Int32 = 65
let exitCoreAudioFailed: Int32 = 69

func fail(_ code: Int32, _ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

// MARK: - 権限の確認

/// TCC の非公開 API で権限の状態を読む。tap の権限には公開の確認 API がなく、
/// 拒否されていても無音が届くだけなので、見分けるために使う。状態は変更しない。
func permissionStatus(service: String) -> String {
    let path = "/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC"
    guard let handle = dlopen(path, RTLD_NOW), let symbol = dlsym(handle, "TCCAccessPreflight") else {
        return "unknown"
    }
    typealias Preflight = @convention(c) (CFString, CFDictionary?) -> Int32
    let preflight = unsafeBitCast(symbol, to: Preflight.self)
    switch preflight(service as CFString, nil) {
    case 0: return "granted"
    case 1: return "denied"
    case 2: return "undetermined"
    default: return "unknown"
    }
}

// MARK: - パケットの書き出し

/// 全バイトを書く。親が読むのをやめた(EPIPE)ら false を返す。
func writeAll(_ pointer: UnsafeRawPointer, _ count: Int) -> Bool {
    var offset = 0
    while offset < count {
        let written = write(STDOUT_FILENO, pointer + offset, count - offset)
        if written < 0 {
            if errno == EINTR { continue }
            return false
        }
        offset += written
    }
    return true
}

func writePacket(sampleRate: UInt32, channels: UInt32, body: UnsafeRawPointer?, byteCount: Int) -> Bool {
    var header = Data("ZTAP".utf8)
    for value in [sampleRate, channels, UInt32(byteCount)] {
        withUnsafeBytes(of: value.littleEndian) { header.append(contentsOf: $0) }
    }
    let wroteHeader = header.withUnsafeBytes { writeAll($0.baseAddress!, $0.count) }
    guard wroteHeader else { return false }
    guard let body, byteCount > 0 else { return true }
    return writeAll(body, byteCount)
}

// MARK: - Core Audio の補助

func check(_ status: OSStatus, _ what: String) {
    if status != noErr {
        fail(exitCoreAudioFailed, "\(what) に失敗しました(OSStatus \(status))")
    }
}

func address(_ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain)
}

func defaultOutputUID() -> String {
    var device = AudioObjectID(kAudioObjectUnknown)
    var size = UInt32(MemoryLayout<AudioObjectID>.size)
    var property = address(kAudioHardwarePropertyDefaultOutputDevice)
    check(
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &property, 0, nil, &size, &device),
        "既定の出力デバイスの取得")

    var uid: Unmanaged<CFString>?
    var uidSize = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    var uidProperty = address(kAudioDevicePropertyDeviceUID)
    check(
        AudioObjectGetPropertyData(device, &uidProperty, 0, nil, &uidSize, &uid),
        "出力デバイスの UID の取得")
    guard let uid else { fail(exitCoreAudioFailed, "出力デバイスの UID が空です") }
    return uid.takeRetainedValue() as String
}

// MARK: - 取得

/// tap と、それを読むための集約デバイスを持つ。操作は main キューで行う。
final class TapCapture {
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var deviceID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let ioQueue = DispatchQueue(label: "zuchi.audio-tap.io", qos: .userInteractive)

    func start() {
        let description = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        description.name = "ai-zuchi"
        description.isPrivate = true
        description.muteBehavior = .unmuted
        check(AudioHardwareCreateProcessTap(description, &tapID), "process tap の作成")

        var format = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var property = address(kAudioTapPropertyFormat)
        check(AudioObjectGetPropertyData(tapID, &property, 0, nil, &size, &format), "tap の形式の取得")

        let isFloat32 =
            format.mFormatID == kAudioFormatLinearPCM
            && format.mFormatFlags & kAudioFormatFlagIsFloat != 0
            && format.mBitsPerChannel == 32
            && format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0
        guard isFloat32 else {
            fail(exitUnsupportedFormat, "想定していない形式です(flags \(format.mFormatFlags))")
        }

        let outputUID = defaultOutputUID()
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "ai-zuchi tap",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapDriftCompensationKey: true,
                    kAudioSubTapUIDKey: description.uuid.uuidString,
                ]
            ],
        ]
        check(
            AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &deviceID),
            "集約デバイスの作成")

        let sampleRate = UInt32(format.mSampleRate)
        let channels = format.mChannelsPerFrame
        check(
            AudioDeviceCreateIOProcIDWithBlock(&procID, deviceID, ioQueue) { _, input, _, _, _ in
                let buffers = UnsafeMutableAudioBufferListPointer(
                    UnsafeMutablePointer(mutating: input))
                for buffer in buffers {
                    let ok = writePacket(
                        sampleRate: sampleRate, channels: channels,
                        body: UnsafeRawPointer(buffer.mData), byteCount: Int(buffer.mDataByteSize))
                    if !ok { DispatchQueue.main.async { shutdown() } }
                }
            },
            "IO の登録")
        // 権限の確認はここで起きる
        check(AudioDeviceStart(deviceID, procID), "取得の開始")

        if !writePacket(sampleRate: sampleRate, channels: channels, body: nil, byteCount: 0) {
            shutdown()
        }
    }

    func stop() {
        if let procID {
            AudioDeviceStop(deviceID, procID)
            AudioDeviceDestroyIOProcID(deviceID, procID)
        }
        procID = nil
        if deviceID != kAudioObjectUnknown {
            AudioHardwareDestroyAggregateDevice(deviceID)
            deviceID = AudioObjectID(kAudioObjectUnknown)
        }
        if tapID != kAudioObjectUnknown {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
    }
}

let capture = TapCapture()

func shutdown() -> Never {
    // 片付けが固まっても、プロセスは必ず終わらせる
    alarm(2)
    capture.stop()
    exit(0)
}

// MARK: - 入口

let arguments = CommandLine.arguments
if arguments.count == 3, arguments[1] == "--check-permission" {
    switch arguments[2] {
    case "mic": print(permissionStatus(service: "kTCCServiceMicrophone"))
    case "system": print(permissionStatus(service: "kTCCServiceAudioCapture"))
    default: fail(exitUsage, "mic か system を指定してください")
    }
    exit(0)
}
guard arguments.count == 1 else {
    fail(exitUsage, "使い方: zuchi-audio-tap [--check-permission mic|system]")
}

// EPIPE として受け取り、片付けてから終わる
signal(SIGPIPE, SIG_IGN)

let signalSources = [SIGINT, SIGTERM].map { number -> DispatchSourceSignal in
    signal(number, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
    source.setEventHandler { shutdown() }
    source.resume()
    return source
}

// 親が落ちても残らないように、stdin の EOF を見張る
Thread.detachNewThread {
    var byte: UInt8 = 0
    while true {
        let count = read(STDIN_FILENO, &byte, 1)
        if count == 0 || (count < 0 && errno != EINTR) { break }
    }
    DispatchQueue.main.async { shutdown() }
}

// 出力先が変わったら、新しい既定の出力に合わせて作り直す。サンプルレートの変化はパケットのヘッダで親に伝わる
var defaultOutputProperty = address(kAudioHardwarePropertyDefaultOutputDevice)
check(
    AudioObjectAddPropertyListenerBlock(
        AudioObjectID(kAudioObjectSystemObject), &defaultOutputProperty, .main
    ) { _, _ in
        capture.stop()
        capture.start()
    },
    "既定の出力の監視")

DispatchQueue.main.async { capture.start() }
dispatchMain()
