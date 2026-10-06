module HyperTabular
  # Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time
  # into typed columns, every cell a HyperCast verdict.
  #
  #   plan = [HyperTabular::Column.i32(0), HyperTabular::Column.text(1), HyperTabular::Column.f64(2)]
  #   HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  #     while (batch = reader.read)
  #       ids = batch.values(0)                        # a column at a time: [1, 2, nil, 4, ...]
  #       batch.rows.times do |row|
  #         case batch.get(2, row)                     # or a cell at a time, as HyperCast's union
  #         in HyperCast::Success(value:) then total += value
  #         in HyperCast::Fault(reason:) then warn "#{reason}: #{batch.raw(2, row).inspect}"
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
  # Each #read returns a Batch, which owns what it shows: it stays good after the next
  # #read. Not thread-safe.
  class DelimitedReader
    # The row ceiling: a single record larger than this is a :row_too_long TabularError.
    MAX_ROW_BYTES = 1 << 30

    # How much is asked of an IO at a time, until a record does not fit.
    DEFAULT_BUFFER_BYTES = 256 * 1024

    # Rows per batch unless told otherwise.
    DEFAULT_BATCH_ROWS = 4096

    # Encodings whose bytes already are the UTF-8 (or byte-identical) form the core reads.
    BYTE_COMPATIBLE = [Encoding::UTF_8, Encoding::US_ASCII, Encoding::ASCII_8BIT].freeze

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
      @dialect = dialect
      @batch_rows = batch_rows
      @buffer_bytes = buffer_bytes
      @close_source = close_source
      # Cell-table entries one row takes: the widest ordinal the plan reads, plus two.
      per_row = (@plan.map(&:ordinal).max || -1) + 2
      @kernel = Runtime::Delimited.start(dialect.packed, @plan.map(&:packed), @plan.map(&:value_bytes),
                                         batch_rows, per_row)
      raise ArgumentError, "separator #{dialect.separator.inspect} is not tab or printable ASCII other than '\"'" if
        @kernel.nil?

      @start = 0
      @closed = false
      @failure = nil
      take(source)
      @header = dialect.has_header ? read_header : nil
    end

    # The header's names, when the dialect declares a header: frozen UTF-8 Strings, quotes
    # resolved. Empty for an input with no record at all; nil when the dialect declares no
    # header.
    attr_reader :header

    # The plan: the output Columns, in output order. Column +i+ of every batch is +plan[i]+.
    attr_reader :plan

    # The declared Dialect.
    attr_reader :dialect

    # The most rows one #read delivers.
    attr_reader :batch_rows

    # Records finished so far — the header and skipped blank lines included.
    def records
      @kernel.records
    end

    # Reads the next batch: a Batch of up to #batch_rows rows, or nil once the input is
    # exhausted. A TabularError when the input is structurally broken — raised after every
    # intact row before the break has been delivered, and the same error again on every
    # later call. IOError on a closed reader.
    def read
      raise IOError, "closed reader" if @closed
      raise @failure if @failure

      loop do
        length, last = window
        raise structural unless @kernel.fill(@start, length, last) == Runtime::Delimited::OK

        consumed = @kernel.consumed
        if @kernel.rows.positive?
          batch = made(@kernel.rows)
          @start += consumed
          return batch
        end
        @start += consumed
        return nil if last && (consumed == length || consumed.zero?)

        refill if consumed.zero?
      end
    end

    # Every remaining row, batch after batch: yields one frozen Array per row, a verdict
    # per plan column. Without a block, an Enumerator. Raises what #read raises.
    def each_row
      return to_enum(:each_row) unless block_given?

      while (batch = read)
        columns = Array.new(@plan.size) { |column| batch.verdicts(column) }
        batch.rows.times { |row| yield columns.map { |column| column[row] }.freeze }
      end
      self
    end

    # Closes the IO if this reader was told to (+close_source+, or DelimitedReader.open).
    # Batches already read stay good. Safe to call twice.
    def close
      return if @closed

      @closed = true
      @io.close if @close_source && @io
      nil
    end

    # True once #close has been called.
    def closed?
      @closed
    end

    # The reader in a line — not its buffers.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size} records=#{records}#{' closed' if @closed}>"
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

    # The batch the core just wrote, copied out of the kernel's arrays: its spans index the
    # input from @start, where the core was handed it.
    def made(rows)
      columns = @kernel.columns
      Batch.new(@plan, rows, Array.new(@plan.size) { |column| columns.values(column, rows) },
                Array.new(@plan.size) { |column| columns.verdicts(column, rows) },
                @kernel.cells, @kernel.per_row, @buffer, @start, @kernel.arena, workbook: false)
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
      @failure = TabularError.from(@kernel.failure)
    end

    # The failure of a record that outgrew the ceiling, kept: it is final too.
    def too_long
      @failure = TabularError.new(:row_too_long, @kernel.records, @kernel.line, @kernel.offset)
    end
  end
end
