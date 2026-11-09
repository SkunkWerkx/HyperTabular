import HyperTabularCore

/// The 1.0 calls, which stay exported beside the `_with` ones: a read of this binding goes through
/// the `_with` calls alone, and a test proves through these that the older ones still answer the
/// same. They are thin on purpose, and nothing else calls them.
enum FirstSurface {
    /// `hypertabular_delimited_init`.
    static func delimitedInit(_ state: UnsafeMutablePointer<HyperTabularDelimitedState>, separator: UInt8) -> Int32 {
        var raw = HyperTabularRawDialect(separator: separator, quoting: 1, skip_blank_lines: 1, engine: 0)
        return hypertabular_delimited_init(state, &raw)
    }

    /// `hypertabular_delimited_fill`.
    static func delimitedFill(
        _ state: UnsafeMutablePointer<HyperTabularDelimitedState>, _ input: UnsafePointer<UInt8>?, _ length: Int,
        last: Bool, specs: UnsafePointer<HyperTabularColumnSpec>, buffers: UnsafePointer<HyperTabularColumnBuffer>,
        count: Int, maxRows: Int, memory: inout HyperTabularBuffers, filled: inout HyperTabularFilled
    ) -> Int32 {
        hypertabular_delimited_fill(
            state, input, UInt(length), last ? 1 : 0, specs, buffers, UInt(count), UInt(maxRows), &memory, &filled)
    }

    /// `hypertabular_workbook_sheet`.
    static func workbookSheet(
        _ state: UnsafeMutableRawBufferPointer, _ container: UnsafeMutableRawBufferPointer, part: [UInt8],
        index: UInt32,
        hasHeader: Bool, skipEmptyRows: Bool, filled: inout HyperTabularFilled
    ) -> Int32 {
        part.withUnsafeBufferPointer {
            hypertabular_workbook_sheet(
                OpaquePointer(state.baseAddress), Workbook.address(container), UInt(container.count), $0.baseAddress,
                UInt($0.count), index, hasHeader ? 1 : 0, skipEmptyRows ? 1 : 0, &filled)
        }
    }

    /// `hypertabular_workbook_fill`.
    static func workbookFill(
        _ state: UnsafeMutableRawBufferPointer, _ container: UnsafeMutableRawBufferPointer,
        specs: UnsafePointer<HyperTabularColumnSpec>, buffers: UnsafePointer<HyperTabularColumnBuffer>, count: Int,
        maxRows: Int, memory: inout HyperTabularBuffers, filled: inout HyperTabularFilled
    ) -> Int32 {
        hypertabular_workbook_fill(
            OpaquePointer(state.baseAddress), Workbook.address(container), UInt(container.count), specs, buffers,
            UInt(count), UInt(maxRows), &memory, &filled)
    }
}
