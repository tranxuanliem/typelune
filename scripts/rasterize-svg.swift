// Rasterize an SVG to a PNG with a transparent background using AppKit (no extra tools).
// Usage: swift scripts/rasterize-svg.swift <in.svg> <out.png> <pixels> [<in2.svg> <out2.png> <pixels2> ...]
// Prints the alpha of the corner and centre pixels so callers can verify transparency.
//        swift scripts/rasterize-svg.swift --check-alpha <a.png> [<b.png> ...]
// Fails unless every PNG has a fully transparent corner and an opaque centre.
import AppKit

if CommandLine.arguments.dropFirst().first == "--check-alpha" {
    var ok = true
    for path in CommandLine.arguments.dropFirst(2) {
        guard let rep = NSBitmapImageRep(data: (try? Data(contentsOf: URL(fileURLWithPath: path))) ?? Data()),
            rep.hasAlpha
        else {
            print("\(path): no alpha channel")
            ok = false
            continue
        }
        let corner = rep.colorAt(x: 0, y: 0)?.alphaComponent ?? -1
        let centre = rep.colorAt(x: rep.pixelsWide / 2, y: rep.pixelsHigh / 2)?.alphaComponent ?? -1
        let pass = corner == 0 && centre == 1
        ok = ok && pass
        print("\(path): \(rep.pixelsWide)px corner-alpha=\(corner) centre-alpha=\(centre) \(pass ? "ok" : "FAIL")")
    }
    exit(ok ? 0 : 1)
}

let args = Array(CommandLine.arguments.dropFirst())
guard !args.isEmpty, args.count % 3 == 0 else {
    FileHandle.standardError.write(Data("usage: rasterize-svg.swift in.svg out.png size ...\n".utf8))
    exit(2)
}
for i in stride(from: 0, to: args.count, by: 3) {
    let (input, output, size) = (args[i], args[i + 1], Int(args[i + 2]) ?? 0)
    guard size > 0, let image = NSImage(contentsOf: URL(fileURLWithPath: input)),
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: 0, bitsPerPixel: 0)
    else {
        FileHandle.standardError.write(Data("cannot rasterize \(input)\n".utf8))
        exit(1)
    }
    rep.size = NSSize(width: size, height: size)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    NSColor.clear.set()
    NSRect(x: 0, y: 0, width: size, height: size).fill()
    image.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
    try png.write(to: URL(fileURLWithPath: output))
    let corner = rep.colorAt(x: 0, y: 0)?.alphaComponent ?? -1
    let centre = rep.colorAt(x: size / 2, y: size / 2)?.alphaComponent ?? -1
    print("\(output): \(size)px corner-alpha=\(corner) centre-alpha=\(centre)")
}
