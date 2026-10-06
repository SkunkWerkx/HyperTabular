module HyperTabular
  module Runtime
    # A plan as the core takes it, and the arrays the core casts it into: one packed
    # ColumnSpec per plan column, one value array and one verdict array per column sized
    # for one batch, and the ColumnBuffer table that names them — allocated once and reused
    # for every batch. A delimited read and a sheet each own one.
    class Columns
      # Bytes in one verdict (CellVerdict: offset, len, reason — three u32).
      VERDICT_BYTES = 12
      # ColumnSpec: ordinal, door, param, then HyperCast's 32-byte RawNumFormat.
      SPEC_BYTES = 44

      # The packed specs (nil for no columns), the ColumnBuffer table (likewise), how many
      # columns there are, and the most rows a batch holds.
      attr_reader :specs, :table, :count, :batch_rows

      # +specs+ is one packed ColumnSpec per plan column and +sizes+ the bytes one value of
      # each takes.
      def initialize(specs, sizes, batch_rows)
        @sizes = sizes
        @count = specs.size
        @batch_rows = batch_rows
        return if specs.empty?

        @specs = Runtime.buffer(SPEC_BYTES * @count)
        @specs[0, SPEC_BYTES * @count] = specs.join
        @values = sizes.map { |size| Runtime.buffer(size * batch_rows) }
        @verdicts = sizes.map { Runtime.buffer(VERDICT_BYTES * batch_rows) }
        addresses = @values.zip(@verdicts).flatten.map(&:to_i).pack("J*")
        @table = Runtime.buffer(addresses.bytesize)
        @table[0, addresses.bytesize] = addresses
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
    end
  end
end
