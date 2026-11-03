// ocr-text.swift <image...> — macOS Vision で画像の文字を読む(第三者の物は使わない・compile 不要: swift scripts/qa/ocr-text.swift a.png b.png)。
// stdout: 1 画像 1 行「path<TAB>テキスト(改行は空白)」。読めない時は「path<TAB><ERR 理由>」。
import Foundation
import Vision
import ImageIO

for path in CommandLine.arguments.dropFirst() {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
        print("\(path)\t<ERR 画像として開けない>"); continue
    }
    let req = VNRecognizeTextRequest()
    req.recognitionLevel = .accurate
    req.recognitionLanguages = ["ja", "en"]
    req.usesLanguageCorrection = false
    do {
        try VNImageRequestHandler(cgImage: img, options: [:]).perform([req])
        let text = (req.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        print("\(path)\t\(text.replacingOccurrences(of: "\t", with: " "))")
    } catch {
        print("\(path)\t<ERR \(error.localizedDescription)>")
    }
}
