module HyperTabular
  # How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
  # stated, quoting is stated, the header is stated — the same stance HyperCast's NumFormat
  # takes for numeric notation.
  #
  # +separator+ is one byte: tab, or any printable ASCII character except the double quote.
  # +quoting+ says whether " quotes cells (RFC 4180, "" for a literal quote); off, a quote
  # is an ordinary byte. +has_header+ says whether the first record is a header, given by
  # DelimitedReader#header and never delivered as a row. +skip_blank_lines+ says whether a
  # completely empty line is skipped rather than read as a one-cell row.
  #
  #   HyperTabular::Dialect::CSV
  #   HyperTabular::Dialect.new(separator: ";", has_header: false)
  Dialect = Data.define(:separator, :quoting, :has_header, :skip_blank_lines) do
    # Everything but the separator defaults to what CSV means: quoted, with a header, blank
    # lines skipped. A separator that is not a one-byte String is a caller bug
    # (ArgumentError); which bytes the scanner can honour is the core's to say, and it says
    # so when a reader is built.
    def initialize(separator:, quoting: true, has_header: true, skip_blank_lines: true)
      raise ArgumentError, "separator must be a one-byte String; got #{separator.inspect}" unless
        separator.is_a?(String) && separator.bytesize == 1

      super(separator: -separator, quoting: quoting ? true : false,
            has_header: has_header ? true : false, skip_blank_lines: skip_blank_lines ? true : false)
    end

    # The four bytes of the core's RawDialect: separator, quoting, skip_blank_lines, and
    # the scan engine left to the core's own choice.
    def packed
      [separator.getbyte(0), quoting ? 1 : 0, skip_blank_lines ? 1 : 0, 0].pack("C4")
    end
  end

  # Comma-separated, quoted, with a header, blank lines skipped.
  Dialect::CSV = Dialect.new(separator: ",")

  # Tab-separated, otherwise as CSV.
  Dialect::TSV = Dialect.new(separator: "\t")

  # Pipe-separated, otherwise as CSV.
  Dialect::PSV = Dialect.new(separator: "|")
end
