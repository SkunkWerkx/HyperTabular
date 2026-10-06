module HyperTabular
  # A structural failure: the input is not rows of cells — a record of the wrong width,
  # input that ends inside a quoted cell, a workbook whose container or parts cannot be
  # read. Never a cell's verdict: a value that does not cast is a HyperCast::Fault in its
  # column, and the read goes on. A structural failure ends the input, after every intact
  # row before it has been delivered.
  #
  # For a workbook, #record is the part the failure is in, #line the sheet row and #byte the
  # offset within the part's inflated bytes.
  #
  #   begin
  #     reader.read
  #   rescue HyperTabular::TabularError => e
  #     case e
  #     in { kind: :column_count, line:, expected:, found: }
  #       warn "line #{line}: #{found} cells, not #{expected}"
  #     in { kind: :unclosed_quote, line: }
  #       warn "line #{line}: a quote is never closed"
  #     end
  #   end
  class TabularError < StandardError
    # What the core's failure codes mean.
    KINDS = {
      1 => :unclosed_quote, 2 => :column_count, 3 => :row_too_long, 16 => :not_a_zip, 17 => :container,
      18 => :encrypted, 19 => :method, 20 => :missing_part, 21 => :xml, 22 => :deflate,
      23 => :not_a_workbook, 24 => :shared_string, 25 => :too_large
    }.freeze

    # What is wrong: :unclosed_quote (the input ended inside a quoted cell), :column_count
    # (a record's cell count disagrees with the first record's), :row_too_long (a single
    # record is larger than DelimitedReader::MAX_ROW_BYTES); for a workbook :not_a_zip,
    # :container (the zip's own structure is broken), :encrypted, :method (a part is
    # compressed by neither stored nor deflate), :missing_part, :xml (a part ends inside an
    # XML construct), :deflate (a part is not a whole deflate stream), :not_a_workbook,
    # :shared_string (a cell names a shared string the table does not have) or :too_large.
    attr_reader :kind

    # Zero-based index of the offending record — the header and skipped blank lines
    # included.
    attr_reader :record

    # One-based line the offending record starts on.
    attr_reader :line

    # Absolute byte offset of the offending record's start, a byte-order mark included.
    attr_reader :byte

    # Cells in the first record, for :column_count; 0 otherwise.
    attr_reader :expected

    # Cells in this record, for :column_count; 0 otherwise.
    attr_reader :found

    # The failure the core reported, as [code, line, record, byte, expected, found]. A code
    # this binding does not know is the container's: the most general refusal.
    def self.from(failure)
      code, line, record, byte, expected, found = failure
      new(KINDS.fetch(code, :container), record, line, byte, expected, found)
    end

    # Raised by the readers; nothing else builds one.
    def initialize(kind, record, line, byte, expected = 0, found = 0)
      @kind = kind
      @record = record
      @line = line
      @byte = byte
      @expected = expected
      @found = found
      super(describe)
    end

    # The failure as a Hash, which is what lets `case error in { kind:, line: }` match.
    def deconstruct_keys(_keys)
      { kind: kind, record: record, line: line, byte: byte, expected: expected, found: found }
    end

    private

    # The message, in the words the other bindings use.
    def describe
      at = "#{record} (line #{line}, byte #{byte})"
      case kind
      when :column_count then "Record #{at} has #{found} cells; the first record had #{expected}."
      when :unclosed_quote then "The input ended inside a quoted cell in record #{at}."
      when :row_too_long then "Record #{at} exceeds the #{DelimitedReader::MAX_ROW_BYTES}-byte row ceiling."
      when :not_a_zip then "The workbook is not a zip file."
      when :encrypted then "The workbook is encrypted."
      when :method then "Part #{record} of the workbook is compressed by method #{found}, neither stored nor deflate."
      when :missing_part then "Part #{record}, which the workbook cannot be read without, is missing."
      when :xml then "Part #{record} of the workbook ends inside an XML construct (byte #{byte})."
      when :deflate then "Part #{record} of the workbook is not a whole deflate stream (byte #{byte})."
      when :not_a_workbook then "The zip is neither an XLSX nor an ODS workbook."
      when :shared_string then "Row #{line} names shared string #{found}; the table has #{expected}."
      when :too_large then "The workbook holds more text than a batch can address."
      else "The workbook's zip structure is broken."
      end
    end
  end
end
