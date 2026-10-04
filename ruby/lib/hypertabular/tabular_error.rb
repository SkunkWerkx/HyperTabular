module HyperTabular
  # A structural failure: the input is not rows of cells — a record of the wrong width,
  # input that ends inside a quoted cell. Never a cell's verdict: a value that does not
  # cast is a HyperCast::Fault in its column, and the read goes on. A structural failure
  # ends the input, after every intact row before it has been delivered.
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
    KINDS = { 1 => :unclosed_quote, 2 => :column_count }.freeze

    # What is wrong: :unclosed_quote (the input ended inside a quoted cell), :column_count
    # (a record's cell count disagrees with the first record's) or :row_too_long (a single
    # record is larger than DelimitedReader::MAX_ROW_BYTES).
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

    # Raised by DelimitedReader; nothing else builds one.
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
      else "Record #{at} exceeds the #{DelimitedReader::MAX_ROW_BYTES}-byte row ceiling."
      end
    end
  end
end
