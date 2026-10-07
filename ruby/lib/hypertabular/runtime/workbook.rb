module HyperTabular
  module Runtime
    # The core's workbook reader as objects: every byte of native memory a workbook read
    # needs and every native call it makes — the Fiddle crossing behind Workbook and Sheet,
    # which never see a pointer.
    #
    # A workbook call works in buffers the caller hands it (a Buffers block of seven pointer
    # and size pairs), and when one is too small it says which and how large, having undone
    # nothing: the buffer is grown with what it held kept, and the same call made again.
    module Book
      class << self
        # For the specs: every buffer a workbook call works in starts with room for one
        # element and no shared-strings bound is asked for, so every call that can stop and
        # resume does — the grow-and-keep path exercised mid-part.
        attr_accessor :stingy

        # For the specs: how many times the window, the arena and the cell table were grown,
        # as [window, arena, cells].
        attr_accessor :grown
      end
      self.stingy = false
      self.grown = [0, 0, 0]

      OK = Delimited::OK
      ERR_STRUCTURE = Delimited::ERR_STRUCTURE
      ERR_ARENA = Delimited::ERR_ARENA
      ERR_CELLS = Delimited::ERR_CELLS
      # The window is too small to inflate and tokenize in.
      ERR_WINDOW = -5

      # The smallest window the core works in.
      WINDOW_MIN = 64 * 1024
      # Bytes in one span, and in one slot of the row a read assembles.
      SPAN_BYTES = 8
      SLOT_BYTES = 16
      # Buffers: seven pointer and size pairs — the same block a delimited fill takes.
      BUFFERS = Delimited::BUFFERS
      BUFFERS_BYTES = Delimited::BUFFERS_BYTES
      # Opened: format, epoch, strings_bytes, strings_count, needed, then Failure.
      OPENED = "L<2Q<3L<2Q<2L<2".freeze
      OPENED_BYTES = 64
      # Filled: rows, consumed, arena_used, needed, then Failure.
      FILLED = Delimited::FILLED
      FILLED_BYTES = Delimited::FILLED_BYTES

      # The workbook's tables as a sheet's calls are handed them: strings, the span of each,
      # the kind of each cell format. Pointers and sizes.
      Tables = Struct.new(:strings, :strings_len, :table, :table_len, :kinds, :kinds_len)

      # The buffers a call to the core may ask to have grown: one set for a workbook while
      # it opens, one for each sheet.
      class Scratch
        # The buffers, each a Fiddle::Pointer, and their sizes (in bytes, spans, slots).
        attr_reader :window, :arena, :cells, :arena_cap, :cells_cap

        def initialize(window, arena, cells, row)
          @window_cap = window
          @window = Runtime.buffer([window, 1].max)
          @arena_cap = arena
          @arena = Runtime.buffer([arena, 1].max)
          @cells_cap = cells
          @cells = Runtime.buffer([SPAN_BYTES * cells, 1].max)
          @row_cap = row
          @row = Runtime.buffer([SLOT_BYTES * row, 1].max)
          @buffers = Runtime.buffer(BUFFERS_BYTES)
        end

        # This scratch as the core takes it, with the workbook's tables when a sheet is
        # being read.
        def buffers(tables = nil)
          words = [@window.to_i, @window_cap, @arena.to_i, @arena_cap, @cells.to_i, @cells_cap, @row.to_i, @row_cap]
          words.concat(tables ? tables.to_a : [0] * 6)
          @buffers[0, BUFFERS_BYTES] = words.pack(BUFFERS)
          @buffers
        end

        # Makes the window at least +needed+ bytes, what it held kept.
        def grow_window(needed)
          @window, @window_cap = grown(@window, @window_cap, needed, 1)
        end

        # Makes the arena at least +needed+ bytes, what it held kept.
        def grow_arena(needed)
          @arena, @arena_cap = grown(@arena, @arena_cap, needed, 1)
        end

        # Makes the cell table at least +needed+ spans, what it held kept.
        def grow_cells(needed)
          @cells, @cells_cap = grown(@cells, @cells_cap, needed, SPAN_BYTES)
        end

        # Makes +call+ (a block taking the Buffers block and the out block) until it stops
        # asking for room, growing the buffer it names each time. Returns the code it ended
        # on and the Filled fields: rows, consumed, arena_used, needed, then the failure.
        def drive(out, tables = nil)
          loop do
            code = yield buffers(tables)
            filled = out[0, FILLED_BYTES].unpack(FILLED)
            case code
            when ERR_WINDOW then grow_window(filled[3]).then { Book.grown[0] += 1 }
            when ERR_ARENA then grow_arena(filled[3]).then { Book.grown[1] += 1 }
            when ERR_CELLS then grow_cells(filled[3]).then { Book.grown[2] += 1 }
            else return [code, filled]
            end
          end
        end

        private

        def grown(old, capacity, needed, unit)
          length = [needed, capacity + 1].max
          larger = Runtime.buffer(unit * length)
          larger[0, unit * capacity] = old[0, unit * capacity] if capacity.positive?
          [larger, length]
        end
      end

      # A workbook opened in memory: its state, its sheets, its tables.
      class Opened
        CALLS = {
          sheets: :hypertabular_workbook_sheets, strings: :hypertabular_workbook_strings,
          styles: :hypertabular_workbook_styles
        }.freeze

        # :xlsx or :ods; the date system's code (1 for 1900, 2 for 1904); the sheets as
        # [name, hidden, part, index]; the shared strings' bytes; the state template every
        # sheet copies; and the tables a sheet's calls are handed.
        attr_reader :format, :epoch, :sheets, :strings, :state, :tables, :container, :length

        # Opens +container+, a String the workbook holds unmodified for its whole life. Raises
        # the failure as [code, line, record, byte, expected, found] in a StructureError.
        def initialize(container)
          @container_string = container
          @container = Runtime.pin(container)
          @length = container.bytesize
          @state_size = Runtime.function(:hypertabular_workbook_state_size).call
          @state = Runtime.buffer(@state_size)
          @out = Runtime.buffer([OPENED_BYTES, FILLED_BYTES].max)
          scratch = Book.stingy ? Scratch.new(1, 1, 1, 0) : Scratch.new(WINDOW_MIN, 1024, 64, 0)
          strings_bytes = open(scratch)
          @sheets = listed(scratch)
          load_tables(scratch, strings_bytes)
        end

        # A copy of the opened state, which the core allows: a sheet's read of its own.
        def copy_of_state
          copy = Runtime.buffer(@state_size)
          copy[0, @state_size] = @state[0, @state_size]
          copy
        end

        private

        def open(scratch)
          call = Runtime.function(:hypertabular_workbook_open)
          loop do
            code = call.call(@state, @container, @length, scratch.buffers, @out)
            format, epoch, strings_bytes, _count, needed, *failure = @out[0, OPENED_BYTES].unpack(OPENED)
            case code
            when OK
              @format = format == 2 ? :ods : :xlsx
              @epoch = epoch
              return strings_bytes
            when ERR_WINDOW then scratch.grow_window(needed)
            when ERR_ARENA then scratch.grow_arena(needed)
            when ERR_STRUCTURE then raise StructureError, failure
            else raise Delimited::CONTRACT
            end
          end
        end

        def run(scratch, name)
          call = Runtime.function(CALLS.fetch(name))
          code, filled = scratch.drive(@out) { |buffers| call.call(@state, @container, @length, buffers, @out) }
          Book.settle(code, filled)
        end

        # The sheets the core listed: three spans each — name, part, then one whose offset's
        # low bit says hidden and whose length is the sheet's index.
        def listed(scratch)
          rows = run(scratch, :sheets).first
          spans = scratch.cells[0, SPAN_BYTES * 3 * rows].unpack("L<*")
          arena = scratch.arena[0, scratch.arena_cap]
          Array.new(rows) do |index|
            name_at, name_len, part_at, part_len, flags, sheet = spans[index * 6, 6]
            [arena.byteslice(name_at, name_len).force_encoding(Encoding::UTF_8).freeze, flags.odd?,
             arena.byteslice(part_at, part_len), sheet]
          end.freeze
        end

        def load_tables(scratch, strings_bytes)
          # The shared strings take no more room than their part inflates to; asking for it
          # once saves growing into it.
          bound = [strings_bytes, 1 << 28].min
          scratch.grow_arena(bound) if !Book.stingy && scratch.arena_cap < bound
          rows, _consumed, arena_used = run(scratch, :strings)
          @strings = scratch.arena[0, arena_used].force_encoding(Encoding::UTF_8).freeze
          @strings_buffer = copy(scratch.arena, arena_used)
          @table = copy(scratch.cells, SPAN_BYTES * rows)
          table_len = rows
          kinds_len = run(scratch, :styles).first
          @kinds = copy(scratch.arena, kinds_len)
          @tables = Tables.new(@strings_buffer.to_i, arena_used, @table.to_i, table_len, @kinds.to_i, kinds_len)
        end

        def copy(from, bytes)
          buffer = Runtime.buffer([bytes, 1].max)
          buffer[0, bytes] = from[0, bytes] if bytes.positive?
          buffer
        end
      end

      # One sheet being read: its own copy of the state, its plan's arrays, its scratch.
      class Reading
        # The plan's arrays, and the cell-table entries one row takes: one per plan column,
        # and one for the row's number.
        attr_reader :columns, :per_row, :header

        def initialize(book, sheet, has_header, skip_empty_rows, specs, sizes, batch_rows, width)
          @book = book
          @state = book.copy_of_state
          @columns = Columns.new(specs, sizes, batch_rows)
          @per_row = specs.size + 1
          @scratch = Book.stingy ? Scratch.new(1, 1, 1, width) : Scratch.new(0, 4096, batch_rows * @per_row, width)
          @out = Runtime.buffer(FILLED_BYTES)
          _name, _hidden, part, index = sheet
          code = Runtime.function(:hypertabular_workbook_sheet).call(
            @state, book.container, book.length, Runtime.pin(part), part.bytesize, index,
            has_header ? 1 : 0, skip_empty_rows ? 1 : 0, @out
          )
          Book.settle(code, @out[0, FILLED_BYTES].unpack(FILLED))
          @header = read_header if has_header
        end

        # The next batch's rows, its cell table, the arena as far as the core wrote into it
        # (which is as far as any of the batch's spans reach), and the failure — [code, line,
        # record, byte, expected, found] — that came with them, if one did: the caller raises
        # it after them. The table and the arena are copies.
        def fill
          call = Runtime.function(:hypertabular_workbook_fill)
          code, filled = @scratch.drive(@out, @book.tables) do |buffers|
            call.call(@state, @book.container, @book.length, @columns.specs, @columns.table,
                      @columns.count, @columns.batch_rows, buffers, @out)
          end
          raise Delimited::CONTRACT unless [OK, ERR_STRUCTURE].include?(code)

          rows, _consumed, arena_used = filled
          [rows, @scratch.cells[0, SPAN_BYTES * @per_row * rows], @scratch.arena[0, arena_used],
           code == OK ? nil : filled[4..]]
        end

        private

        def read_header
          call = Runtime.function(:hypertabular_workbook_header)
          code, filled = @scratch.drive(@out, @book.tables) do |buffers|
            call.call(@state, @book.container, @book.length, buffers, @out)
          end
          rows = Book.settle(code, filled).first
          spans = @scratch.cells[0, SPAN_BYTES * rows].unpack("L<*")
          arena = nil
          Array.new(rows) do |index|
            offset = spans[index * 2]
            length = spans[index * 2 + 1]
            name =
              if length < Delimited::SPAN_FLAG
                @book.strings.byteslice(offset, length)
              else
                arena ||= @scratch.arena[0, @scratch.arena_cap].force_encoding(Encoding::UTF_8)
                arena.byteslice(offset, length & Delimited::SPAN_LENGTH)
              end
            name.freeze
          end.freeze
        end
      end

      # A structural failure the core reported, as [code, line, record, byte, expected,
      # found], on its way to becoming a TabularError.
      class StructureError < StandardError
        attr_reader :failure

        def initialize(failure)
          @failure = failure
          super("structural failure #{failure.first}")
        end
      end

      # What a call's code means: the Filled fields, or a structural failure. Anything else
      # is this binding's bug.
      def self.settle(code, filled)
        case code
        when OK then filled
        when ERR_STRUCTURE then raise StructureError, filled[4..]
        else raise Delimited::CONTRACT
        end
      end
    end
  end
end
