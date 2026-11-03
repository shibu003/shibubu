// IME 試験の入力ソース切替（Carbon の TIS）。引数なし = 今の入力ソース ID を出す / ID = それを選んでから今の ID を出す / --list = 一覧。
// 例: swift scripts/input-source.swift com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese
import Carbon

func sourceID(_ source: TISInputSource) -> String {
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "?" }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

let args = Array(CommandLine.arguments.dropFirst())
if args.first == "--list" {
    let all = TISCreateInputSourceList(nil, false).takeRetainedValue() as! [TISInputSource]
    for source in all { print(sourceID(source)) }
    exit(0)
}
if let id = args.first {
    let filter = [kTISPropertyInputSourceID as String: id] as CFDictionary
    let found = TISCreateInputSourceList(filter, false).takeRetainedValue() as! [TISInputSource]
    guard let source = found.first else { print("見つからない: \(id)"); exit(1) }
    print("TISSelectInputSource \(id) -> \(TISSelectInputSource(source))")
}
print(sourceID(TISCopyCurrentKeyboardInputSource().takeRetainedValue()))
