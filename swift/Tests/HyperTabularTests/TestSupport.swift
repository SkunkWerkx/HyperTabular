import Foundation
import XCTest

#if canImport(Android)
    import Android
#elseif canImport(Glibc)
    import Glibc
#elseif canImport(Musl)
    import Musl
#endif

@testable import HyperTabular

/// What several suites here want.
enum Support {
    static let indian = NumFormat.invariant.withGrouping(.indian)
    static let commaDecimal = NumFormat(decimalSeparator: ",", groupSeparator: ".", styles: .all)

    /// A package of the corpus, or a skip where the corpus is out of reach.
    static func package(_ file: String) throws -> String {
        guard let directory = CorpusTests.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        return directory.appendingPathComponent("workbook").appendingPathComponent(file).path
    }

    /// A format holding a word no public initializer of HyperCast's will make: what a decoder that
    /// rebuilt a `NumFormat` from stored bytes could hand a plan. HyperCast's own door traps at
    /// construction, which ``traps(_:)`` holds; this is the only way a bad word reaches this binding's
    /// door at all. The bytes are read back through HyperCast's own accessor, so that a change in
    /// `NumFormat`'s layout fails here and not as a test that proves nothing.
    static func worded(_ around: NumFormat, flags: UInt32) -> NumFormat {
        var format = around
        withUnsafeMutableBytes(of: &format) { $0.storeBytes(of: flags, toByteOffset: 4, as: UInt32.self) }
        precondition(Interop.rawFormat(format).1 == flags, "NumFormat no longer holds its flags where this says")
        return format
    }

    /// The words 351 and 863 hold the retired bit; 95 and 607 are those without it, the control that a
    /// door refuses for the word and nothing else.
    static let policies: [(flags: UInt32, format: NumFormat, refused: Bool)] = [
        (351, worded(.invariant, flags: 351), true),
        (863, worded(indian, flags: 863), true),
        (95, .invariant, false),
        (607, indian, false),
    ]

    /// The smallest package that holds these strings in column A, each as text in its own row: a
    /// stored zip, written here since Swift's standard library has no zip writer.
    static func strings(_ cells: [String]) throws -> Workbook {
        let namespace = "http://schemas.openxmlformats.org"
        let rows = cells.enumerated().map { at, cell in
            "<row r=\"\(at + 1)\"><c r=\"A\(at + 1)\" t=\"inlineStr\"><is><t>\(cell)</t></is></c></row>"
        }.joined()
        let parts: [(String, String)] = [
            (
                "[Content_Types].xml",
                "<Types xmlns=\"\(namespace)/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>"
            ),
            (
                "_rels/.rels",
                "<Relationships xmlns=\"\(namespace)/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"\(namespace)/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
            ),
            (
                "xl/workbook.xml",
                "<workbook xmlns=\"\(namespace)/spreadsheetml/2006/main\" xmlns:r=\"\(namespace)/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
            ),
            (
                "xl/_rels/workbook.xml.rels",
                "<Relationships xmlns=\"\(namespace)/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"\(namespace)/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>"
            ),
            (
                "xl/worksheets/sheet1.xml",
                "<worksheet xmlns=\"\(namespace)/spreadsheetml/2006/main\"><sheetData>\(rows)</sheetData></worksheet>"
            ),
        ]
        return try Workbook(bytes: zip(parts))
    }

    private static func zip(_ parts: [(String, String)]) -> [UInt8] {
        var out = [UInt8]()
        var directory = [UInt8]()
        func u16(_ value: Int, _ into: inout [UInt8]) {
            withUnsafeBytes(of: UInt16(value).littleEndian) { into.append(contentsOf: $0) }
        }
        func u32(_ value: UInt32, _ into: inout [UInt8]) {
            withUnsafeBytes(of: value.littleEndian) { into.append(contentsOf: $0) }
        }
        for (name, text) in parts {
            let body = Array(text.utf8)
            let path = Array(name.utf8)
            let crc = crc32(body)
            let offset = UInt32(out.count)
            u32(0x0403_4B50, &out)
            for field in [20, 0, 0, 0, 0] { u16(field, &out) }
            u32(crc, &out)
            u32(UInt32(body.count), &out)
            u32(UInt32(body.count), &out)
            u16(path.count, &out)
            u16(0, &out)
            out += path + body
            u32(0x0201_4B50, &directory)
            for field in [20, 20, 0, 0, 0, 0] { u16(field, &directory) }
            u32(crc, &directory)
            u32(UInt32(body.count), &directory)
            u32(UInt32(body.count), &directory)
            u16(path.count, &directory)
            for field in [0, 0, 0, 0] { u16(field, &directory) }
            u32(0, &directory)
            u32(offset, &directory)
            directory += path
        }
        let start = UInt32(out.count)
        out += directory
        u32(0x0605_4B50, &out)
        u16(0, &out)
        u16(0, &out)
        u16(parts.count, &out)
        u16(parts.count, &out)
        u32(UInt32(directory.count), &out)
        u32(start, &out)
        u16(0, &out)
        return out
    }

    private static func crc32(_ bytes: [UInt8]) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in bytes {
            crc ^= UInt32(byte)
            for _ in 0..<8 {
                crc = crc & 1 != 0 ? (crc >> 1) ^ 0xEDB8_8320 : crc >> 1
            }
        }
        return ~crc
    }

    #if os(macOS) || os(Linux)
        /// What became of `body`, run in a child process: a precondition failure is a trap XCTest can't
        /// catch, so it is met where it ends only the child. The child's handlers turn the trap into an
        /// exit, so no crash report is written and nothing appears on screen. `fork` comes through `dlsym`
        /// because Swift marks it unavailable on Darwin; the child does nothing but `body` and `_exit`.
        enum Ending: Equatable {
            case returned
            case trapped
            case other(Int32)
        }

        static func ending(of body: () -> Void) -> Ending {
            typealias Fork = @convention(c) () -> Int32
            guard let handle = dlopen(nil, RTLD_NOW), let symbol = dlsym(handle, "fork") else {
                return .other(-1)
            }
            let fork = unsafeBitCast(symbol, to: Fork.self)
            fflush(nil)
            let child = fork()
            if child == 0 {
                let silent = open("/dev/null", O_WRONLY)
                dup2(silent, 2)
                let trapped: @convention(c) (Int32) -> Void = { _ in _exit(42) }
                signal(SIGTRAP, trapped)
                signal(SIGILL, trapped)
                body()
                _exit(0)
            }
            let deadline = Date().addingTimeInterval(60)
            var status: Int32 = 0
            while waitpid(child, &status, WNOHANG) == 0 {
                if Date() > deadline {
                    kill(child, SIGKILL)
                    waitpid(child, &status, 0)
                    return .other(-2)
                }
                usleep(1000)
            }
            let exited = status & 0x7F == 0
            let code = (status >> 8) & 0xFF
            guard exited else { return .other(status) }
            return code == 42 ? .trapped : code == 0 ? .returned : .other(code)
        }
    #endif
}
