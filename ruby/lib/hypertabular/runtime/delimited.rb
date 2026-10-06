module HyperTabular
  module Runtime
    # The core's delimited reader as one object: every byte of native memory a read needs
    # and every native call it makes. This class is the whole Fiddle crossing — the reader
    # above it (DelimitedReader) is plain Ruby that never sees a pointer — and so it is the
    # one thing a compiled extension would replace: same methods, bytes in and bytes out.
    #
    # The memory is allocated once and reused for every batch: the state block, the plan,
    # one value array and one verdict array per column, the cell table, the arena. The core
    # keeps none of it between calls beyond what it writes into the state block.
    class Delimited
      # The call did what it could; the result says how far it got.
      OK = 0
      # A caller bug, never a data verdict.
      ERR_CONTRACT = -1
      # The data is structurally broken; the failure says where.
      ERR_STRUCTURE = -2
      # The arena cannot hold what one row needs.
      ERR_ARENA = -3
      # The cell table (or the header's name table) cannot hold one row.
      ERR_CELLS = -4

      # The flag in the top bit of a span's length. On a cell-table entry: the cell has ""
      # inside and has to be unescaped to be read. On a text value or a header name: the
      # bytes are in the arena rather than in the input.
      SPAN_FLAG = 1 << 31
      # A span's length without its flag.
      SPAN_LENGTH = SPAN_FLAG - 1

      # Bytes in one span.
      SPAN_BYTES = 8
      # Filled: rows, consumed, arena_used, needed, then Failure (code, line, record, byte,
      # expected, found).
      FILLED_BYTES = 64
      FILLED = "Q<4L<2Q<2L<2".freeze

      ARENA_BYTES = 4096
      NAMES = 64

      CONTRACT = "hypertabular: libhypertabular reported a contract violation — a binding bug, " \
                 "please report it".freeze

      # What the last call wrote: rows (or header names), input bytes finished with, arena
      # bytes written, and — after ERR_STRUCTURE — the failure as
      # [code, line, record, byte, expected, found].
      attr_reader :rows, :consumed, :arena_used, :failure

      # The plan's arrays (Columns), and the cell-table entries one row takes: the widest
      # ordinal the plan reads, plus two — or more, if the core asked for more.
      attr_reader :columns, :per_row

      # The loaded core's version word, major << 16 | minor << 8 | patch.
      def self.version
        Runtime.function(:hypertabular_version).call
      end

      # +dialect+ is the four bytes of a RawDialect; +specs+ one packed ColumnSpec per plan
      # column and +sizes+ the bytes one value of each takes; +per_row+ the cell-table
      # entries one row takes. Nil when the core refuses the dialect.
      def self.start(dialect, specs, sizes, batch_rows, per_row)
        reader = new(specs, sizes, batch_rows, per_row)
        reader.send(:init, dialect) ? reader : nil
      end

      def initialize(specs, sizes, batch_rows, per_row)
        @header = Runtime.function(:hypertabular_delimited_header)
        @fill = Runtime.function(:hypertabular_delimited_fill)
        @state = Runtime.buffer(Runtime.function(:hypertabular_delimited_state_size).call)
        @batch_rows = batch_rows
        @columns = Columns.new(specs, sizes, batch_rows)
        @per_row = per_row
        @cramped = false
        @cells_cap = per_row * batch_rows
        @cells = Runtime.buffer(SPAN_BYTES * @cells_cap)
        @arena_cap = ARENA_BYTES
        @arena = Runtime.buffer(@arena_cap)
        @out = Runtime.buffer(FILLED_BYTES)
        @rows = @consumed = @arena_used = 0
      end

      # Names the String the calls that follow read: +start+ and +offset+ below index its
      # bytes. It is held where it is, not copied, and must not be modified until another
      # one is attached.
      def attach(input)
        @pin = Runtime.pin(input)
        @base = @pin.to_i
      end

      # Reads the next record of the attached input, +length+ bytes from +start+, as a
      # header. OK or ERR_STRUCTURE; +rows+ is then the number of names.
      def header(start, length, last)
        @names_cap ||= NAMES
        @names ||= Runtime.buffer(SPAN_BYTES * @names_cap)
        loop do
          code = @header.call(@state, @base + start, length, last ? 1 : 0,
                              @names, @names_cap, @arena, @arena_cap, @out)
          needed = finished
          case code
          when OK, ERR_STRUCTURE then return code
          when ERR_CELLS
            @names_cap = needed
            @names = Runtime.buffer(SPAN_BYTES * @names_cap)
          when ERR_ARENA then grow_arena(needed)
          else raise CONTRACT
          end
        end
      end

      # The header's names as the core located them: offset and flagged length, a pair per
      # name, offsets relative to the +start+ the header was read at (or into the arena,
      # for a flagged one).
      def names
        @names[0, SPAN_BYTES * @rows].unpack("L<*")
      end

      # Fills every column from the attached input, +length+ bytes from +start+ — the one
      # native call a batch makes. OK or ERR_STRUCTURE.
      #
      # The core ends a batch early when the arena fills, so a batch that came back short
      # with the arena half used or more has the next one start with it doubled: escaped
      # text costs a few batches, not one per row.
      def fill(start, length, last)
        grow_arena(@arena_cap * 2) if @cramped
        @cramped = false
        loop do
          code = @fill.call(@state, @base + start, length, last ? 1 : 0,
                            @columns.specs, @columns.table, @columns.count, @batch_rows,
                            @cells, @cells_cap, @arena, @arena_cap, @out)
          needed = finished
          case code
          when OK
            @cramped = @rows.positive? && @rows < @batch_rows && @arena_used * 2 >= @arena_cap
            return code
          when ERR_STRUCTURE then return code
          when ERR_CELLS
            @per_row = [@per_row, needed].max
            @cells_cap = @per_row * @batch_rows
            @cells = Runtime.buffer(SPAN_BYTES * @cells_cap)
          when ERR_ARENA then grow_arena(needed)
          else raise CONTRACT
          end
        end
      end

      # The cell table for the batch in hand — #per_row spans a row — copied out.
      def cells
        @cells[0, SPAN_BYTES * @per_row * @rows]
      end

      # The arena as the last call left it: the unescaped text of every flagged span.
      def arena
        @arena[0, @arena_used]
      end

      # Records finished so far — the header and skipped blank lines included.
      def records
        @state[8, 8].unpack1("Q<")
      end

      # One-based line number of the next unread byte.
      def line
        @state[4, 4].unpack1("L<")
      end

      # Absolute byte offset of the next unread byte.
      def offset
        @state[16, 8].unpack1("Q<")
      end

      private

      def init(dialect)
        raw = Runtime.buffer(4)
        raw[0, 4] = dialect
        Runtime.function(:hypertabular_delimited_init).call(@state, raw) == OK
      end

      # Reads what the call wrote to its Filled block; returns `needed`.
      def finished
        @rows, @consumed, @arena_used, needed, *@failure = @out[0, FILLED_BYTES].unpack(FILLED)
        needed
      end

      def grow_arena(needed)
        @arena_cap = [needed, @arena_cap * 2].max
        @arena = Runtime.buffer(@arena_cap)
      end
    end
  end
end
