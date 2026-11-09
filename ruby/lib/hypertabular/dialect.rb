module HyperTabular
  # What a declaration's fields are held to, shared by Dialect, SheetOptions, Range and Column.
  module Declared
    module_function

    # +value+ if it is a count an unsigned 32-bit field holds; an ArgumentError naming +field+
    # otherwise.
    def count(field, value)
      return value if value.is_a?(Integer) && value.between?(0, 0xFFFF_FFFF)

      raise ArgumentError, "#{field} must be an Integer from 0 to 2^32 - 1; got #{value.inspect}"
    end

    # +list+ as a frozen Array of frozen Strings, each the UTF-8 the core compares a cell's bytes
    # with, or an ArgumentError: a lone String is too easy a mistake to take a letter at a time.
    def tokens(list)
      raise ArgumentError, "tokens must be an Array of Strings; got #{list.inspect}" unless list.is_a?(Array)

      list.map do |token|
        raise ArgumentError, "a token must be a String; got #{token.inspect}" unless token.is_a?(String)

        token.encoding == Encoding::BINARY ? -token.dup.force_encoding(Encoding::UTF_8) : -token.encode(Encoding::UTF_8)
      end.freeze
    end

    # A token list as the core takes it: each a little-endian u32 length, then its bytes.
    def packed_tokens(list)
      list.map { |token| [token.bytesize].pack("L<") + token.b }.join.b
    end
  end

  # How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
  # stated, quoting is stated, the header is stated, the encoding is stated — the same stance
  # HyperCast's NumFormat takes for numeric notation.
  #
  # +separator+ is one byte: tab, or any printable ASCII character except the double quote.
  # +quoting+ says whether " quotes cells (RFC 4180, "" for a literal quote); off, a quote
  # is an ordinary byte. +has_header+ says whether the first record is a header, given by
  # DelimitedReader#header and never delivered as a row. +skip_blank_lines+ says whether a
  # completely empty line is skipped rather than read as a one-cell row.
  #
  # +skip+ is how many physical lines to pass over after a byte-order mark, before anything is
  # read of them, so a quote in one means nothing. +header_row+ is how many records to discard
  # after those, ahead of the header if there is one; a blank line this dialect skips is not a
  # record. +short_rows+ says what a record with fewer cells than the width is: :reject, a
  # TabularError, or :pad, a row whose missing cells are :empty Faults at Origin :padded.
  # +long_rows+ says what a record with more cells is: :reject, :report (a row cut to the width,
  # and listed by Batch#faults with the width it had) or :truncate (cut, and nothing said).
  # +encoding+ is what the input's bytes are, never sniffed: :utf8, read in place; :windows_1252;
  # :latin1; or :utf16le, with or without a byte-order mark. Anything but UTF-8 is decoded ahead
  # of the read, so a batch's text and every span are the decoded text's, and it is the source's
  # bytes that are decoded, whatever encoding a String source says it is in. +tokens+ is what
  # stands for no value in every column without tokens of its own: a cell whose exact, unquoted
  # bytes are one of these is an :empty Fault, whatever its door, at Origin :token.
  #
  #   HyperTabular::Dialect::CSV
  #   HyperTabular::Dialect.new(separator: ";", has_header: false)
  #   HyperTabular::Dialect.new(separator: ",", skip: 2, short_rows: :pad, tokens: ["N/A"])
  Dialect = Data.define(:separator, :quoting, :has_header, :skip_blank_lines, :skip, :header_row, :short_rows,
                        :long_rows, :encoding, :tokens) do
    # Everything but the separator defaults to what CSV means: quoted, with a header, blank
    # lines skipped, every record the same width, UTF-8, no tokens. A separator that is not a
    # one-byte String, or any other field that is not what it says above, is a caller bug
    # (ArgumentError); which bytes the scanner can honour is the core's to say, and it says so
    # when a reader is built.
    def initialize(separator:, quoting: true, has_header: true, skip_blank_lines: true, skip: 0, header_row: 0,
                   short_rows: :reject, long_rows: :reject, encoding: :utf8, tokens: [])
      raise ArgumentError, "separator must be a one-byte String; got #{separator.inspect}" unless
        separator.is_a?(String) && separator.bytesize == 1

      { short_rows: [short_rows, Dialect::SHORT_ROWS], long_rows: [long_rows, Dialect::LONG_ROWS],
        encoding: [encoding, Dialect::ENCODINGS] }.each do |field, (value, known)|
        raise ArgumentError, "#{field} must be one of #{known.keys.inspect}; got #{value.inspect}" unless
          known.key?(value)
      end

      super(separator: -separator, quoting: quoting ? true : false,
            has_header: has_header ? true : false, skip_blank_lines: skip_blank_lines ? true : false,
            skip: Declared.count("skip", skip), header_row: Declared.count("header_row", header_row),
            short_rows: short_rows, long_rows: long_rows, encoding: encoding, tokens: Declared.tokens(tokens))
    end

    # The four bytes of the core's 1.0 RawDialect: separator, quoting, skip_blank_lines, and
    # the scan engine left to the core's own choice. No read takes it; it is what the 1.0 exports
    # stay proved with.
    def packed
      [separator.getbyte(0), quoting ? 1 : 0, skip_blank_lines ? 1 : 0, 0].pack("C4")
    end

    # The 24 bytes of the core's RawDelimitedOptions, what every read starts from. The encoding
    # is not in it: decoding happens ahead of the fill.
    def packed_with
      [Dialect::OPTIONS_BYTES, separator.getbyte(0), quoting ? 1 : 0, skip_blank_lines ? 1 : 0, 0, skip, header_row,
       Dialect::SHORT_ROWS.fetch(short_rows), Dialect::LONG_ROWS.fetch(long_rows)].pack("L<C4L<4")
    end

    # The tokens as the core takes them, or nil for none.
    def packed_tokens
      tokens.empty? ? nil : Declared.packed_tokens(tokens)
    end
  end

  # The size of Dialect#packed_with: what RawDelimitedOptions' first word carries.
  Dialect::OPTIONS_BYTES = 24

  # What a record with fewer cells than the width can be, by the core's code.
  Dialect::SHORT_ROWS = { reject: 0, pad: 1 }.freeze

  # What a record with more cells than the width can be, by the core's code.
  Dialect::LONG_ROWS = { reject: 0, report: 1, truncate: 2 }.freeze

  # What a delimited file's bytes can be declared as, by the core's transcoding code: UTF-8 makes
  # no call.
  Dialect::ENCODINGS = { utf8: 0, windows_1252: 1, latin1: 2, utf16le: 3 }.freeze

  # Comma-separated, quoted, with a header, blank lines skipped.
  Dialect::CSV = Dialect.new(separator: ",")

  # Tab-separated, otherwise as CSV.
  Dialect::TSV = Dialect.new(separator: "\t")

  # Pipe-separated, otherwise as CSV.
  Dialect::PSV = Dialect.new(separator: "|")
end
