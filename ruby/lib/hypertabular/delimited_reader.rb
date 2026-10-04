module HyperTabular
  # Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time
  # into typed columns, every cell a HyperCast verdict.
  #
  #   plan = [HyperTabular::Column.i32(0), HyperTabular::Column.text(1), HyperTabular::Column.f64(2)]
  #   HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  #     while reader.read
  #       ids = reader.values(0)                       # a column at a time: [1, 2, nil, 4, ...]
  #       reader.rows.times do |row|
  #         case reader.verdict(2, row)                # or a cell at a time, as HyperCast's union
  #         in HyperCast::Success(value:) then total += value
  #         in HyperCast::Fault(reason:) then warn "#{reason}: #{reader.raw(2, row).inspect}"
  #         end
  #       end
  #     end
  #   end
  #
  # The native core (libhypertabular) owns no memory and reads no files. It is handed a
  # chunk of input and the buffers to fill, casts each plan column in one native loop, and
  # says how many rows it wrote and how many bytes it is finished with. Everything else is
  # here: this class holds the input, has one value array and one verdict array per column
  # allocated once and reused for every batch, and puts what the core did not consume back
  # in front of it. The native boundary is crossed once per batch, not once per cell, and a
  # column comes out of its buffer in one String#unpack.
  #
  # A value that does not cast is that cell's verdict, and the read goes on. Input that is
  # not rows of cells at all — a record of the wrong width, a quote never closed — is a
  # TabularError, raised after every intact row before it has been delivered.
  #
  # Values are HyperCast's gem's own for the same doors: true/false, Integer, Float,
  # HyperCast::Decimal, a hyphenated UUID String, a UTC Time, Date, DateTime, Integer
  # nanoseconds since midnight, Rational seconds, and a UTF-8 String for text. What a batch
  # hands out is copied out of the reader's buffers, so it stays good after the next #read;
  # the reader itself answers for one batch at a time. Not thread-safe.
  class DelimitedReader
    # The row ceiling: a single record larger than this is a :row_too_long TabularError.
    MAX_ROW_BYTES = 1 << 30

    # How much is asked of an IO at a time, until a record does not fit.
    DEFAULT_BUFFER_BYTES = 256 * 1024

    # Rows per batch unless told otherwise.
    DEFAULT_BATCH_ROWS = 4096

    # The verdict of every cell that had no bytes at all: one shared, frozen Fault.
    EMPTY = HyperCast::Fault.new(reason: :empty, offset: 0, length: 0)

    # Encodings whose bytes already are the UTF-8 (or byte-identical) form the core reads.
    BYTE_COMPATIBLE = [Encoding::UTF_8, Encoding::US_ASCII, Encoding::ASCII_8BIT].freeze

    # The doors whose column is one String#unpack directive away from its values.
    SCALARS = {
      bool: "C*", i8: "c*", i16: "s<*", i32: "l<*", i64: "q<*", u8: "C*", u16: "S<*", u32: "L<*",
      u64: "Q<*", f32: "e*", f64: "E*", time: "Q<*"
    }.freeze

    # HyperCast's Timestamp — seconds and nanoseconds, protobuf-shaped — as a UTC Time: the
    # one `Time.at(seconds, nanos, :nanosecond, in: "UTC")` makes, at half the cost a row of
    # having the zone's name read each time.
    INSTANT = ["q<l<x4", 2, ->(fields, at) { Time.at(fields[at], fields[at + 1], :nanosecond).utc }].freeze

    # HyperCast's Date — year, month, day — as a Date.
    DAY = ["S<CC", 3, ->(fields, at) { Date.new(fields[at], fields[at + 1], fields[at + 2]) }].freeze

    # The doors whose value is a record: the directive that unpacks one, how many fields
    # that yields, and what builds the Ruby value from them — each the type HyperCast's gem
    # returns from the same door.
    RECORDS = {
      decimal: ["Q<L<CCx2", 4, lambda { |fields, at|
        HyperCast::Decimal.new(magnitude: (fields[at + 1] << 64) | fields[at], scale: fields[at + 2],
                               negative: fields[at + 3] != 0)
      }],
      uuid: ["H8H4H4H4H12", 5, ->(fields, at) { fields[at, 5].join("-") }],
      timestamp: INSTANT, unix: INSTANT, excel_serial: INSTANT,
      date: DAY, date_ordered: DAY,
      datetime: ["S<CCx4Q<", 4, lambda { |fields, at|
        second_of_day, nanos = fields[at + 3].divmod(1_000_000_000)
        hour, rest = second_of_day.divmod(3600)
        minute, second = rest.divmod(60)
        DateTime.new(fields[at], fields[at + 1], fields[at + 2], hour, minute,
                     second + Rational(nanos, 1_000_000_000))
      }],
      duration: ["q<l<x4", 2, lambda { |fields, at|
        Rational(fields[at] * 1_000_000_000 + fields[at + 1], 1_000_000_000)
      }]
    }.freeze

    # Opens a file of UTF-8 delimited text. With a block, yields the reader, closes it —
    # and the file — when the block ends, and returns the block's value; without one,
    # returns the reader, whose #close closes the file.
    def self.open(path, dialect, plan, batch_rows: DEFAULT_BATCH_ROWS, buffer_bytes: DEFAULT_BUFFER_BYTES)
      file = File.open(path, "rb")
      begin
        reader = new(file, dialect, plan, batch_rows: batch_rows, buffer_bytes: buffer_bytes, close_source: true)
      ensure
        # The file is ours to close when no reader came of it, whatever went wrong.
        file.close if reader.nil?
      end
      return reader unless block_given?

      begin
        yield reader
      ensure
        reader.close
      end
    end

    # Reads UTF-8 delimited text from +source+: a String, read in place — nothing is copied
    # — or an IO (anything with #readpartial or #read), read forward only through a buffer
    # of +buffer_bytes+ that grows when a record does not fit it. An IO is read as bytes;
    # one opened in text mode on Windows should be in binmode first.
    #
    # +dialect+ is the declared Dialect and +plan+ the output Columns, in output order.
    # +batch_rows+ is the most rows one #read delivers. +close_source+ says whether #close
    # also closes the IO.
    #
    # A dialect the scanner cannot honour, or a plan that is not Columns, is an
    # ArgumentError. With a header declared the header record is read here, so a header
    # that is structurally broken is a TabularError here.
    def initialize(source, dialect, plan, batch_rows: DEFAULT_BATCH_ROWS, buffer_bytes: DEFAULT_BUFFER_BYTES,
                   close_source: false)
      raise ArgumentError, "dialect must be a HyperTabular::Dialect; got #{dialect.inspect}" unless
        dialect.is_a?(Dialect)
      raise ArgumentError, "batch_rows must be a positive Integer; got #{batch_rows.inspect}" unless
        batch_rows.is_a?(Integer) && batch_rows.positive?
      raise ArgumentError, "buffer_bytes must be a positive Integer; got #{buffer_bytes.inspect}" unless
        buffer_bytes.is_a?(Integer) && buffer_bytes.positive?

      @plan = Array(plan).dup.freeze
      @plan.each_with_index do |column, index|
        raise ArgumentError, "plan column #{index} must be a HyperTabular::Column; got #{column.inspect}" unless
          column.is_a?(Column)
      end
      @batch_rows = batch_rows
      @buffer_bytes = buffer_bytes
      @close_source = close_source
      # Cell-table entries one row takes: the widest ordinal the plan reads, plus two.
      @per_row = (@plan.map(&:ordinal).max || -1) + 2
      @kernel = Runtime::Delimited.start(dialect.packed, @plan.map(&:packed), @plan.map(&:value_bytes),
                                         batch_rows, @per_row)
      raise ArgumentError, "separator #{dialect.separator.inspect} is not tab or printable ASCII other than '\"'" if
        @kernel.nil?

      @values = Array.new(@plan.size)
      @verdicts = Array.new(@plan.size)
      @faults = Array.new(@plan.size)
      @directives = {}
      @rows = 0
      @start = 0
      @window = 0
      @closed = false
      @failure = nil
      take(source)
      @header = dialect.has_header ? read_header : nil
    end

    # The header's names, when the dialect declares a header: frozen UTF-8 Strings, quotes
    # resolved. Empty for an input with no record at all; nil when the dialect declares no
    # header.
    attr_reader :header

    # The plan: the output Columns, in output order.
    attr_reader :plan

    # Rows in the batch in hand; 0 before the first #read and after the last.
    attr_reader :rows

    # The most rows one #read delivers.
    attr_reader :batch_rows

    # The number of plan columns.
    def column_count
      @plan.size
    end

    # The plan Column at +column+.
    def column(column)
      @plan.fetch(column)
    end

    # Records finished so far — the header and skipped blank lines included.
    def records
      @kernel.records
    end

    # Reads the next batch: true with #rows rows in hand, false once the input is
    # exhausted. A TabularError when the input is structurally broken — raised after every
    # intact row before the break has been delivered, and the same error again on every
    # later call. IOError on a closed reader.
    def read
      raise IOError, "closed reader" if @closed
      raise @failure if @failure

      clear
      loop do
        length, last = window
        raise structural unless @kernel.fill(@start, length, last) == Runtime::Delimited::OK

        consumed = @kernel.consumed
        if @kernel.rows.positive?
          @window = @start
          @start += consumed
          @rows = @kernel.rows
          return true
        end
        @start += consumed
        return false if last && (consumed == length || consumed.zero?)

        refill if consumed.zero?
      end
    end

    # A column's values for the batch in hand, one per row, as a frozen Array: the Ruby
    # value HyperCast's gem returns from the same door, and nil for a cell that did not
    # cast. Decoded from the column's buffer in one pass the first time it is asked for.
    def values(column)
      @values[column] ||= decode(@plan.fetch(column), column)
    end

    # A column's verdicts for the batch in hand, one per row, as a frozen Array of
    # HyperCast::Success and HyperCast::Fault. A Fault's span is in the units String#[]
    # slices by on the cell's own text, as HyperCast's are: `raw(column, row)[offset,
    # length]` is the offending text.
    def verdicts(column)
      @verdicts[column] ||= judge(column)
    end

    # The verdict of one cell of the batch in hand — made for `case`/`in`:
    #
    #   case reader.verdict(0, row)
    #   in HyperCast::Success(value:) then value
    #   in HyperCast::Fault(reason: :empty) then nil
    #   in HyperCast::Fault(reason:, offset:, length:) then raise "#{reason} at #{offset}"
    #   end
    #
    # Only this cell's verdict is built — asking after the few cells whose value came back
    # nil does not pay for a Success around every other row of the column. IndexError for a
    # column outside the plan or a row outside the batch.
    def verdict(column, row)
      index = at(row)
      judged = @verdicts[column]
      return judged[index] if judged

      values = values(column)
      cell(column, index, values, faults(column))
    end

    # The text the cell at (+column+, +row+) was cast from, whatever its door and whatever
    # its verdict — what a Fault's span indexes, and what to show for a value that did not
    # cast. A UTF-8 String, quotes resolved. IndexError for a column outside the plan or a
    # row outside the batch.
    def raw(column, row)
      entry = at(row) * @per_row + @plan.fetch(column).ordinal
      @cells ||= @kernel.cells(@per_row)
      offset, length = @cells.unpack("L<L<", offset: entry * Runtime::Delimited::SPAN_BYTES)
      return @buffer.byteslice(@window + offset, length) if length < Runtime::Delimited::SPAN_FLAG

      # A cell with an escaped quote in it: unescaped, as the core cast it.
      @kernel.unescape(@window + offset, length & Runtime::Delimited::SPAN_LENGTH).force_encoding(Encoding::UTF_8)
    end

    # Every remaining row, batch after batch: yields one frozen Array per row, a verdict
    # per plan column. Without a block, an Enumerator. Raises what #read raises.
    def each_row
      return to_enum(:each_row) unless block_given?

      while read
        columns = Array.new(@plan.size) { |column| verdicts(column) }
        @rows.times { |row| yield columns.map { |column| column[row] }.freeze }
      end
      self
    end

    # Lets go of the batch in hand, and closes the IO if this reader was told to
    # (+close_source+, or DelimitedReader.open). Safe to call twice.
    def close
      return if @closed

      @closed = true
      clear
      @io.close if @close_source && @io
      nil
    end

    # True once #close has been called.
    def closed?
      @closed
    end

    # The reader in a line — not its buffers.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size} rows=#{@rows} records=#{records}#{' closed' if @closed}>"
    end

    private

    # Takes the source: a String becomes the whole input, an IO the thing to refill from.
    def take(source)
      if source.is_a?(String)
        @buffer = utf8(source)
        @eof = true
      elsif defined?(::Pathname) && source.is_a?(::Pathname)
        # It has a #read, and one that starts the file over on every call.
        raise ArgumentError, "source is a Pathname; DelimitedReader.open reads a path"
      elsif source.respond_to?(:readpartial) || source.respond_to?(:read)
        @io = source
        @partial = source.respond_to?(:readpartial)
        @buffer = String.new(encoding: Encoding::UTF_8)
        @eof = false
      else
        raise ArgumentError, "source must be a String or an IO; got #{source.class}"
      end
      @kernel.attach(@buffer)
    end

    # The text as a frozen String of UTF-8 bytes that is this reader's alone: the caller's
    # own when it is already that; otherwise a copy-on-write duplicate, which shares the
    # bytes and is immune to what the caller does to theirs next. Only a foreign encoding
    # pays a transcode.
    def utf8(text)
      return text if text.frozen? && text.encoding == Encoding::UTF_8
      return text.dup.force_encoding(Encoding::UTF_8).freeze if BYTE_COMPATIBLE.include?(text.encoding)

      text.encode(Encoding::UTF_8).freeze
    end

    # What the core is to read next — how many bytes from @start — and whether nothing
    # follows them.
    def window
      length = @buffer.bytesize - @start
      length > MAX_ROW_BYTES ? [MAX_ROW_BYTES, false] : [length, @eof]
    end

    # Puts the unfinished record at the front of the buffer and reads more behind it —
    # at least as much again as is pending, so a record that does not fit doubles the
    # buffer rather than creeping up on it.
    def refill
      pending = @buffer.bytesize - @start
      raise too_long if @io.nil? || pending >= MAX_ROW_BYTES

      chunk = more([@buffer_bytes, pending].max)
      if chunk.nil? || chunk.empty?
        @eof = true
        return
      end
      chunk = chunk.dup if chunk.frozen?
      @buffer = @buffer.byteslice(@start, pending) if @start.positive?
      @buffer << chunk.force_encoding(Encoding::UTF_8)
      @start = 0
      @kernel.attach(@buffer)
    end

    # Up to +bytes+ more of the IO, or nil at its end. #readpartial where there is one, so
    # a pipe or a socket hands over what it has rather than waiting to fill the request.
    def more(bytes)
      @partial ? @io.readpartial(bytes) : @io.read(bytes)
    rescue EOFError
      nil
    end

    # Forgets the batch in hand.
    def clear
      @rows = 0
      @values.fill(nil)
      @verdicts.fill(nil)
      @faults.fill(nil)
      @cells = nil
      @arena = nil
    end

    # Reads the header record: its names, or none for an input with no record at all.
    def read_header
      loop do
        length, last = window
        raise structural unless @kernel.header(@start, length, last) == Runtime::Delimited::OK

        consumed = @kernel.consumed
        if @kernel.rows.positive?
          names = header_names
          @start += consumed
          return names
        end
        @start += consumed
        # An empty input has no header and no rows; the width is unknown.
        return [].freeze if last && (consumed == length || consumed.zero?)

        refill if consumed.zero?
      end
    end

    # The names the core just located: in the input as written, or — flagged — unescaped
    # in the arena.
    def header_names
      spans = @kernel.names
      arena = nil
      Array.new(@kernel.rows) do |index|
        offset = spans[index * 2]
        length = spans[index * 2 + 1]
        name =
          if length < Runtime::Delimited::SPAN_FLAG
            @buffer.byteslice(@start + offset, length)
          else
            arena ||= @kernel.arena.force_encoding(Encoding::UTF_8)
            arena.byteslice(offset, length & Runtime::Delimited::SPAN_LENGTH)
          end
        name.freeze
      end.freeze
    end

    # The structural failure the core just reported, kept: it is final.
    def structural
      code, line, record, byte, expected, found = @kernel.failure
      @failure = TabularError.new(TabularError::KINDS.fetch(code), record, line, byte, expected, found)
    end

    # The failure of a record that outgrew the ceiling, kept: it is final too.
    def too_long
      @failure = TabularError.new(:row_too_long, @kernel.records, @kernel.line, @kernel.offset)
    end

    # +row+ as an index into the batch in hand, or an IndexError.
    def at(row)
      return row if row.is_a?(Integer) && row >= 0 && row < @rows

      raise IndexError, "row #{row.inspect} is outside the batch in hand (#{@rows} rows)"
    end

    # A column's verdict array as the core wrote it — offset, length, reason per row, flat
    # — or nil when every cell cast, which one pass over the bytes answers without
    # unpacking anything.
    def faults(index)
      flat = @faults[index]
      if flat.nil?
        bytes = @kernel.verdicts(index)
        flat = @faults[index] = bytes.count("\0") == bytes.bytesize ? false : bytes.unpack("L<*")
      end
      flat || nil
    end

    # A column's values out of its buffer: one String#unpack over the whole column, then
    # the door's Ruby type built for each row that cast.
    def decode(column, index)
      return [].freeze if @rows.zero?

      door = column.door
      bytes = @kernel.values(index)
      faults = faults(index)
      if (directive = SCALARS[door])
        values = bytes.unpack(directive)
        values.map! { |byte| byte != 0 } if door == :bool
        @rows.times { |row| values[row] = nil unless faults[row * 3 + 2].zero? } if faults
      elsif door == :text
        values = texts(bytes.unpack("L<*"), faults)
      else
        directive, width, build = RECORDS.fetch(door)
        fields = bytes.unpack(repeated(directive))
        values = Array.new(@rows) do |row|
          build.call(fields, row * width) if faults.nil? || faults[row * 3 + 2].zero?
        end
      end
      values.freeze
    end

    # +directive+ once per row of the batch in hand. The full batch's is kept — every batch
    # but the last is one — and a short batch's is built for it.
    def repeated(directive)
      return directive * @rows unless @rows == @batch_rows

      @directives[directive] ||= directive * @rows
    end

    # A text column's values: each span sliced out of the input as written, or — flagged —
    # out of the arena, where the core unescaped it.
    def texts(spans, faults)
      Array.new(@rows) do |row|
        next unless faults.nil? || faults[row * 3 + 2].zero?

        offset = spans[row * 2]
        length = spans[row * 2 + 1]
        if length < Runtime::Delimited::SPAN_FLAG
          @buffer.byteslice(@window + offset, length)
        else
          @arena ||= @kernel.arena.force_encoding(Encoding::UTF_8)
          @arena.byteslice(offset, length & Runtime::Delimited::SPAN_LENGTH)
        end
      end
    end

    # A column's verdicts: a Success around each value, a Fault where the core says so.
    def judge(column)
      values = values(column)
      faults = faults(column) unless @rows.zero?
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
        text = raw(column, row)
        unless text.ascii_only?
          length = text.byteslice(offset, length).length
          offset = text.byteslice(0, offset).length
        end
      end
      HyperCast::Fault.new(reason: HyperCast::REASONS.fetch(reason), offset: offset, length: length)
    end
  end
end
