// ocr-mask.swift <needles file> <in.png> <out.png> [<in> <out> ...] — Vision で読んだ文字のうち needle を含む所だけ黒(#202731)で塗った写しを out に書く(第三者の物は使わない)。
// 実行は swiftc -O -o <bin> scripts/qa/ocr-mask.swift で compile してから(swift script の JIT だと Vision の boundingBox(for:) が link できない)。
// 1 組 1 行: 「in<TAB>masked <塗った矩形の数>」/「in<TAB>no hit」(out は書かない)/「in<TAB><ERR 理由>」。元の in は書き換えない。
// needle の部分の矩形が取れなければ観測(1 行の文字)全体を塗る。余白 2px。
import Foundation
import Vision
import ImageIO
import CoreGraphics
import UniformTypeIdentifiers

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 3, (args.count - 1) % 2 == 0,
      let nraw = try? String(contentsOfFile: args[0], encoding: .utf8) else {
    FileHandle.standardError.write(Data("usage: ocr-mask.swift <needles> <in> <out> [<in> <out>...]\n".utf8)); exit(2)
}
let needles = nraw.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
let pad: CGFloat = 2

// 1 回読んで needle に当たった矩形を返す。pass 2 以降(塗り残し)は観測全体を塗る。
func hits(_ img: CGImage, whole: Bool) throws -> [CGRect] {
    let req = VNRecognizeTextRequest()
    req.recognitionLevel = .accurate
    req.recognitionLanguages = ["ja", "en"]
    req.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: img, options: [:]).perform([req])
    let w = CGFloat(img.width), h = CGFloat(img.height)
    var rects: [CGRect] = []
    for obs in req.results ?? [] {
        guard let cand = obs.topCandidates(1).first else { continue }
        let s = cand.string
        for n in needles {
            var from = s.startIndex
            while let r = s.range(of: n, options: .caseInsensitive, range: from..<s.endIndex) {
                let box = whole ? obs.boundingBox : ((try? cand.boundingBox(for: r))?.boundingBox ?? obs.boundingBox)
                rects.append(CGRect(x: box.minX * w - pad, y: box.minY * h - pad, width: box.width * w + 2 * pad, height: box.height * h + 2 * pad))
                from = r.upperBound
            }
        }
    }
    return rects
}

func mask(_ inp: String, _ outp: String) -> String {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: inp) as CFURL, nil),
          let orig = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return "<ERR 画像として開けない>" }
    var img = orig, total = 0
    for pass in 0..<3 {   // 塗った後にもう一度読み、残りが無くなるまで(最大 3 回)
        let rects: [CGRect]
        do { rects = try hits(img, whole: pass > 0) } catch { return "<ERR \(error.localizedDescription)>" }
        if rects.isEmpty { break }
        guard let ctx = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return "<ERR context>" }
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
        ctx.setFillColor(CGColor(srgbRed: 0x20/255.0, green: 0x27/255.0, blue: 0x31/255.0, alpha: 1))
        for r in rects { ctx.fill(r) }
        guard let out = ctx.makeImage() else { return "<ERR 書けない>" }
        img = out; total += rects.count
    }
    if total == 0 { return "no hit" }
    guard let dst = CGImageDestinationCreateWithURL(URL(fileURLWithPath: outp) as CFURL, UTType.png.identifier as CFString, 1, nil) else { return "<ERR 書けない>" }
    CGImageDestinationAddImage(dst, img, nil)
    return CGImageDestinationFinalize(dst) ? "masked \(total)" : "<ERR 書けない>"
}

var i = 1
while i < args.count { print("\(args[i])\t\(mask(args[i], args[i + 1]))"); i += 2 }
