module HyperTabular
  # The rows one DelimitedReader#read or Sheet#read delivered, as typed columns: the same
  # class for delimited text and for a sheet of a workbook.
  #
  #   while (batch = reader.read)
  #     ids = batch.values(0)                        # a column at a time: [1, 2, nil, 4, ...]
  #     batch.rows.times do |row|
  #       case batch.get(2, row)                     # or a cell at a time, as HyperCast's union
  #       in HyperCast::Success(value:) then total += value
  #       in HyperCast::Fault(reason:) then warn "line #{batch.line(row)}: #{reason} in #{batch.raw(2, row).inspect}"
  #       end
  #     end
  #   end
  #
  # A batch owns what it shows — the arrays the core wrote are copied out of the reader's
  # buffers as it is made — so it stays good after the reader has moved on. Values are
  # HyperCast's gem's own for the same doors: true/false, Integer, Float, HyperCast::Decimal,
  # a hyphenated UUID String, a UTC Time, Date, DateTime, Integer nanoseconds since midnight,
  # Rational seconds, and a UTF-8 String for text.
  #
  # A batch is also Enumerable over its rows, for a caller that thinks in rows: #each yields
  # a Row per row, in order.
  #
  #   batch.each { |row| orders << Order.new(row.value(0), row.value(1), row.line) }
  class Batch
    include Enumerable

    # The verdict of every cell that had no bytes at all: one shared, frozen Fault.
    EMPTY = HyperCast::Fault.new(reason: :empty, offset: 0, length: 0)

    # The doors whose column is one String#unpack directive away from its values: HyperCast's
    # directive for one value, repeated down the column.
    SCALARS = HyperCast::Interop::SCALARS.transform_values { |directive| "#{directive}*" }.freeze

    # The doors whose value is a record: HyperCast's directive for one, how many fields it
    # yields, and what builds the Ruby value HyperCast's own door returns from them.
    RECORDS = HyperCast::Interop::RECORDS

    # How many rows the batch holds — never zero.
    attr_reader :rows

    # The plan the batch was read through: column +i+ of the batch is +columns[i]+.
    attr_reader :columns

    # Made by a reader from what the core just wrote, copied: +values+ and +verdicts+ per
    # plan column, the cell table (+per_row+ spans a row), what an unflagged span indexes
    # (+base+, from +origin+) and what a flagged one does (+arena+).
    def initialize(plan, rows, values, verdicts, cells, per_row, base, origin, arena, workbook:)
      @columns = plan
      @rows = rows
      @bytes = values
      @verdict_bytes = verdicts
      @cells = cells
      @per_row = per_row
      @base = base
      @origin = origin
      @arena = arena.force_encoding(Encoding::UTF_8)
      @workbook = workbook
      @values = Array.new(plan.size)
      @verdicts = Array.new(plan.size)
      @faults = Array.new(plan.size)
    end

    # Where row +row+ came from: for delimited text the 1-based line its record starts on,
    # for a sheet its 1-based row number. IndexError for a row outside the batch.
    def line(row)
      offset, length = @cells.unpack("L<L<", offset: (at(row) * @per_row + @per_row - 1) * 8)
      @workbook ? offset : length
    end

    # A column's values, one per row, as a frozen Array: the Ruby value HyperCast's gem
    # returns from the same door, and nil for a cell that did not cast. Decoded in one pass
    # the first time it is asked for.
    def values(column)
      @values[column] ||= decode(@columns.fetch(column), column)
    end

    # A column's verdicts, one per row, as a frozen Array of HyperCast::Success and
    # HyperCast::Fault. A Fault's span is in the units String#[] slices by on the cell's own
    # text, as HyperCast's are: `raw(column, row)[offset, length]` is the offending text.
    def verdicts(column)
      @verdicts[column] ||= judge(column)
    end

    # The cell at (+column+, +row+) as HyperCast judged it — made for `case`/`in`:
    #
    #   case batch.get(0, row)
    #   in HyperCast::Success(value:) then value
    #   in HyperCast::Fault(reason: :empty) then nil
    #   in HyperCast::Fault(reason:, offset:, length:) then raise "#{reason} at #{offset}"
    #   end
    #
    # Only this cell's verdict is built. IndexError for a column outside the plan or a row
    # outside the batch.
    def get(column, row)
      index = at(row)
      judged = @verdicts[column]
      return judged[index] if judged

      cell(column, index, values(column), faults(column))
    end

    # The text the cell at (+column+, +row+) was cast from, whatever its door and whatever
    # its verdict — what a Fault's span indexes, and what to show for a value that did not
    # cast. A UTF-8 String, quotes resolved. For a sheet this is a text cell's own text, and
    # what a typed cell was said as when it failed its door (or went through the text door);
    # a typed cell that cast has none. IndexError for a column outside the plan or a row
    # outside the batch.
    def raw(column, row)
      plan = @columns.fetch(column)
      entry = at(row) * @per_row + (@workbook ? column : plan.ordinal)
      offset, length = @cells.unpack("L<L<", offset: entry * 8)
      return slice(offset, length) if @workbook || length < Runtime::Delimited::SPAN_FLAG

      # A quoted cell with an escaped quote in it: unescaped, as the core cast it.
      quoted = @base.byteslice(@origin + offset, length & Runtime::Delimited::SPAN_LENGTH)
      Runtime.unescape(quoted).force_encoding(Encoding::UTF_8)
    end

    # Yields a Row for each row of the batch, in order, and returns the batch; without a
    # block, an Enumerator.
    def each
      return to_enum(:each) { @rows } unless block_given?

      @rows.times { |index| yield Row.new(self, index) }
      self
    end

    # The Row at +row+. IndexError for a row outside the batch.
    def row(row)
      Row.new(self, at(row))
    end

    # The batch in a line — not its cells.
    def inspect
      "#<#{self.class.name} rows=#{@rows} columns=#{@columns.size}>"
    end

    private

    # The bytes a span names: in the arena when it is flagged, in the base when not.
    def slice(offset, length)
      return @arena.byteslice(offset, length & Runtime::Delimited::SPAN_LENGTH) if length >= Runtime::Delimited::SPAN_FLAG

      @base.byteslice(@origin + offset, length)
    end

    # +row+ as an index into the batch, or an IndexError.
    def at(row)
      return row if row.is_a?(Integer) && row >= 0 && row < @rows

      raise IndexError, "row #{row.inspect} is outside the batch (#{@rows} rows)"
    end

    # A column's verdict array as the core wrote it — offset, length, reason per row, flat
    # — or nil when every cell cast, which one pass over the bytes answers without
    # unpacking anything.
    def faults(index)
      flat = @faults[index]
      if flat.nil?
        bytes = @verdict_bytes.fetch(index)
        flat = @faults[index] = bytes.count("\0") == bytes.bytesize ? false : bytes.unpack("L<*")
      end
      flat || nil
    end

    # A column's values out of its array: one String#unpack over the whole column, then the
    # door's Ruby type built for each row that cast.
    def decode(column, index)
      door = column.door
      bytes = @bytes.fetch(index)
      faults = faults(index)
      if (directive = SCALARS[door])
        values = bytes.unpack(directive)
        values.map! { |byte| byte != 0 } if door == :bool
        @rows.times { |row| values[row] = nil unless faults[row * 3 + 2].zero? } if faults
      elsif door == :text
        values = texts(bytes.unpack("L<*"), faults)
      else
        directive, width, build = RECORDS.fetch(door)
        fields = bytes.unpack(directive * @rows)
        values = Array.new(@rows) do |row|
          build.call(fields, row * width) if faults.nil? || faults[row * 3 + 2].zero?
        end
      end
      values.freeze
    end

    # A text column's values: each span sliced out of the input or the shared strings, or —
    # flagged — out of the arena.
    def texts(spans, faults)
      Array.new(@rows) do |row|
        slice(spans[row * 2], spans[row * 2 + 1]) if faults.nil? || faults[row * 3 + 2].zero?
      end
    end

    # A column's verdicts: a Success around each value, a Fault where the core says so.
    def judge(column)
      values = values(column)
      faults = faults(column)
      return values.map { |value| HyperCast::Success.new(value: value) }.freeze if faults.nil?

      Array.new(@rows) { |row| cell(column, row, values, faults) }.freeze
    end

    # One cell's verdict, from its column's values and the core's verdict array.
    def cell(column, row, values, faults)
      reason = faults.nil? ? 0 : faults[row * 3 + 2]
      return HyperCast::Success.new(value: values[row]) if reason.zero?

      fault(column, row, reason, faults[row * 3], faults[row * 3 + 1])
    end

    # One Fault, its span moved from the bytes the core counts in to the characters
    # String#[] slices by — an identity unless the cell's text has a multi-byte character.
    def fault(column, row, reason, offset, length)
      return EMPTY if reason == 1 && offset.zero? && length.zero?

      unless offset.zero? && length.zero?
        offset, length = HyperCast::Interop.characters(raw(column, row), offset, length)
      end
      HyperCast::Interop.fault(reason, offset, length)
    end
  end
end
