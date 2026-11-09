module HyperTabular
  module Runtime
    # A plan as the core takes it, and the arrays the core casts it into: one packed
    # ColumnSpec per plan column, one value array and one verdict array per column sized
    # for one batch, and the ColumnBuffer table that names them — allocated once and reused
    # for every batch. A delimited read and a sheet each own one.
    class Columns
      # Bytes in one verdict (CellVerdict: offset, len, reason — three u32).
      VERDICT_BYTES = 12
      # Bytes in one CellNote: a detail and an origin, two u16.
      NOTE_BYTES = 4
      # Bytes in one row width.
      WIDTH_BYTES = 4
      # Bytes in a pointer.
      WORD = [0].pack("J").bytesize
      # A ColumnBufferWith: its size, then the values, the verdicts and the notes.
      COLUMN_BUFFER_BYTES = 4 * WORD
      # A BuffersWith: its size, then eight pointer and size pairs, then the row widths.
      BUFFERS_WITH_BYTES = 18 * WORD

      # The packed specs (nil for no columns), the ColumnBufferWith table (likewise), how many
      # columns there are, and the most rows a batch holds.
      attr_reader :specs, :table, :count, :batch_rows

      # The size word of a size-tagged struct, in the pointer-wide slot it starts.
      def self.word(size)
        [size].pack("L<").ljust(WORD, "\0")
      end

      # One ColumnBufferWith, for its arrays. A null values array is for a call that reads none.
      def self.column_buffer(values, verdicts, notes)
        word(COLUMN_BUFFER_BYTES) + [values.to_i, verdicts.to_i, notes.to_i].pack("J3")
      end

      # The whole ColumnSpecWith for an entry of Column.specs. Each list goes in +lists+, which the
      # caller keeps for as long as the bytes are used. Null tokens inherit, so an empty list still
      # needs an address.
      def self.spec_with(spec, lists)
        fixed, months, tokens = spec
        fixed + [months, tokens].map do |list|
          next [0, 0].pack("J2") if list.nil?

          held = Runtime.buffer([list.bytesize, 1].max)
          held[0, list.bytesize] = list unless list.empty?
          lists << held
          [held.to_i, list.bytesize].pack("J2")
        end.join
      end

      # +specs+ is one Column.specs entry per plan column and +sizes+ the bytes one value of each
      # takes. +notes+ says whether each column keeps a CellNote per row, and +widths+ whether
      # each row's width is kept.
      def initialize(specs, sizes, batch_rows, notes, widths)
        @sizes = sizes
        @count = specs.size
        @batch_rows = batch_rows
        @row_widths = Runtime.buffer(WIDTH_BYTES * batch_rows) if widths
        return if specs.empty?

        @lists = []
        packed = specs.map { |spec| Columns.spec_with(spec, @lists) }.join
        @specs = Runtime.buffer(packed.bytesize)
        @specs[0, packed.bytesize] = packed
        @values = sizes.map { |size| Runtime.buffer(size * batch_rows) }
        @verdicts = sizes.map { Runtime.buffer(VERDICT_BYTES * batch_rows) }
        @notes = sizes.map { Runtime.buffer(NOTE_BYTES * batch_rows) } if notes
        entries = @count.times.map { |at| Columns.column_buffer(@values[at], @verdicts[at], @notes && @notes[at]) }.join
        @table = Runtime.buffer(entries.bytesize)
        @table[0, entries.bytesize] = entries
      end

      # The row widths' address, or 0 for none.
      def widths_address
        @row_widths.to_i
      end

      # A column's value array for the first +rows+ rows, copied out as the core wrote it.
      def values(column, rows)
        @values[column][0, @sizes[column] * rows]
      end

      # A column's verdict array for the first +rows+ rows — offset, len, reason per row —
      # copied out as the core wrote it.
      def verdicts(column, rows)
        @verdicts[column][0, VERDICT_BYTES * rows]
      end

      # A column's CellNote array for the first +rows+ rows, copied out as the core wrote it.
      def notes(column, rows)
        @notes[column][0, NOTE_BYTES * rows]
      end

      # The widths of the first +rows+ rows, copied out as the core wrote them.
      def widths(rows)
        @row_widths[0, WIDTH_BYTES * rows]
      end
    end
  end
end
