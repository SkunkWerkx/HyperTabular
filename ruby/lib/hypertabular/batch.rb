module HyperTabular
  # One thing that went wrong in a Batch: an entry of Batch#faults.
  #
  # +row+ is the row in the batch, from zero. +line+ is where it came from, Batch#line.
  # +plan_column+ is the column of the plan, and +source_column+ the column in the source, from
  # zero (for delimited text the plan column's ordinal, for a sheet its own column, A being 0,
  # wherever the range starts), each nil for an entry about the row as a whole. +reason+ is the
  # verdict's: :malformed, :out_of_range, or :empty for a formula with no cached value, and nil
  # for a violation or an entry about the row as a whole. +detail+ is what failed in the cell, a
  # HyperCast::Detail or nil. +origin+ is where the verdict came from, Batch#origin. +offset+ and
  # +length+ are the fault's or the violation's span in the characters String#[] slices the cell's
  # text by, 0 where there is none, and +expected+ and +found+ a long row's widths, otherwise 0.
  # +keyword+ is the HyperCast::Keyword a violation broke, and nil for anything else.
  Problem = Data.define(:row, :line, :plan_column, :source_column, :reason, :detail, :origin, :offset, :length,
                        :expected, :found, :keyword)

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
  # A column that declares HyperCast::Constraints is read with #get_checked, which says what
  # passed (a HyperCast::Checked), what broke a rule (a HyperCast::Violation) and what didn't cast
  # (a HyperCast::Fault). #get and #verdicts would read a violation as passed, so they raise
  # TypeError for one; #values holds a violation's value as cast.
  #
  # A batch owns what it shows — the arrays the core wrote are copied out of the reader's
  # buffers as it is made — so it stays good after the reader has moved on, and so do its rows
  # and its #faults. A Fault carries the HyperCast::Detail of what failed in the cell, and #origin
  # says where a verdict came from. Values are
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

    # The verdict of every cell that had no bytes at all in a read with notes off: one shared,
    # frozen Fault.
    EMPTY = HyperCast::Fault.new(reason: :empty, offset: 0, length: 0)

    # The same in a read with notes on, which says it was blank: one shared, frozen Fault.
    BLANK = HyperCast::Fault.new(reason: :empty, offset: 0, length: 0, detail: HyperCast::Detail::BLANK)

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
    # (+base+, from +origin+) and what a flagged one does (+arena+). +notes+ is each column's
    # CellNote array, or nil with notes off, +checks+ each column's RawCheck array (nil for an
    # unconstrained column) or nil for a plan that declares no constraints, +widths+ the row widths
    # of a read that reports long rows, with the +expected+ width and the +long_rows+ code, and
    # +first_column+ the source column a plan's ordinal 0 is.
    def initialize(plan, rows, values, verdicts, cells, per_row, base, origin, arena, workbook:, notes: nil,
                   checks: nil, widths: nil, expected: 0, long_rows: 0, first_column: 0)
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
      @notes = notes
      @checks = checks
      @widths = widths
      @expected = expected
      @long_rows = long_rows
      @first_column = first_column
      @values = Array.new(plan.size)
      @verdicts = Array.new(plan.size)
      @checked_verdicts = Array.new(plan.size)
      @words = Array.new(plan.size)
    end

    # What a reader hands a Batch as +checks+: each column's RawCheck array for its first +rows+ rows,
    # from the runtime's +columns+, or nil for a plan that declares no constraints.
    def self.checks_of(plan, columns, rows)
      return nil unless plan.any?(&:checked?)

      Array.new(plan.size) { |column| columns.checks(column, rows) }
    end

    # Where row +row+ came from: for delimited text the 1-based line its record starts on,
    # for a sheet its 1-based row number. IndexError for a row outside the batch.
    def line(row)
      offset, length = @cells.unpack("L<L<", offset: (at(row) * @per_row + @per_row - 1) * 8)
      @workbook ? offset : length
    end

    # A column's values, one per row, as a frozen Array: the Ruby value HyperCast's gem
    # returns from the same door, and nil for a cell that did not cast. A cell of a constrained
    # column that broke a rule cast, so its value is here as cast, never rounded. Decoded in one
    # pass the first time it is asked for.
    def values(column)
      @values[column] ||= decode(@columns.fetch(column), column)
    end

    # A column's verdicts, one per row, as a frozen Array of HyperCast::Success and
    # HyperCast::Fault. A Fault's span is in the units String#[] slices by on the cell's own
    # text, as HyperCast's are: `raw(column, row)[offset, length]` is the offending text. TypeError
    # for a column that declares constraints: read it with #checked_verdicts.
    def verdicts(column)
      @verdicts[column] ||= judge(unchecked(column))
    end

    # A column's verdicts as HyperCast's checked door judges them, one per row, as a frozen Array:
    # each cell as #get_checked reads it. Any column can be read this way, a constrained one and
    # one that declares nothing alike. Built in one pass the first time it is asked for.
    # IndexError for a column outside the plan.
    def checked_verdicts(column)
      @columns.fetch(column)
      @checked_verdicts[column] ||= Array.new(@rows) { |row| get_checked(column, row) }.freeze
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
    # outside the batch, and TypeError for a column that declares constraints: read it with
    # #get_checked.
    def get(column, row)
      unchecked(column)
      index = at(row)
      judged = @verdicts[column]
      return judged[index] if judged

      cell(column, index, values(column), verdict_words(column))
    end

    # The cell at (+column+, +row+) as HyperCast's checked door judges it — made for `case`/`in`:
    #
    #   case batch.get_checked(0, row)
    #   in HyperCast::Checked(value:) then value
    #   in HyperCast::Violation(keyword:, value:) then warn "#{value} breaks #{keyword}"
    #   in HyperCast::Fault(reason:) then warn reason.to_s
    #   end
    #
    # A HyperCast::Checked when the cell cast and met every constraint its column declares:
    # +adjusted+ when a multiple_of rounding changed the value, and for a text enum match the entry
    # as declared, the very String the Constraints holds, with its +enum_index+. A
    # HyperCast::Violation when it cast and broke one: the Keyword, the value as cast, and the span
    # of the judged text. A HyperCast::Fault when it didn't cast, as #get says it. A column that
    # declares none is read too: every cell that cast passes, unadjusted. IndexError for a column
    # outside the plan or a row outside the batch.
    def get_checked(column, row)
      plan = @columns.fetch(column)
      index = at(row)
      faults = verdict_words(column)
      reason = faults.nil? ? 0 : faults[index * 3 + 2]
      unless reason.zero? || reason == VIOLATION
        return fault(column, index, reason, faults[index * 3], faults[index * 3 + 1])
      end

      value = values(column)[index]
      return HyperCast::Checked.new(value: value, adjusted: false, enum_index: nil) unless plan.checked?

      keyword, adjusted, enum_index = @checks.fetch(column).unpack("L<3", offset: index * CHECK_BYTES)
      if reason == VIOLATION
        offset, length = span(column, index, faults[index * 3], faults[index * 3 + 1])
        return HyperCast::Violation.new(keyword: HyperCast::Keyword.new(code: keyword), value: value,
                                        offset: offset, length: length)
      end

      enum_index = nil if enum_index == NO_ENTRY
      value = plan.constraints.enum.fetch(enum_index) if enum_index && plan.door == :text
      HyperCast::Checked.new(value: value, adjusted: adjusted != 0, enum_index: enum_index)
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

    # Where the verdict of the cell at (+column+, +row+) came from: :cell, a door judged the cell's
    # own text; :token, its exact unquoted bytes are a declared token, so it is :empty and no door
    # ran; :padded, the row has no such cell; or :uncached, a workbook's formula with no cached
    # value, :empty with no door run. With notes off, always :cell. IndexError for a column outside
    # the plan or a row outside the batch.
    def origin(column, row)
      @columns.fetch(column)
      index = at(row)
      return :cell if @notes.nil?

      ORIGINS.fetch(@notes.fetch(column).unpack1("S<", offset: index * NOTE_BYTES + 2))
    end

    # Everything wrong in this batch, by row then column, a row's entry first: a frozen Array of
    # Problem. Listed: :malformed and :out_of_range cells, a workbook's formulas with no cached
    # value, and long rows a :report dialect cut. Not listed: blanks, tokens, padding, or a row
    # :truncate cut. With notes off a refused cell has no detail and an uncached formula isn't
    # listed. A row the empty-row filter dropped can't be named. Made the first time it is asked
    # for, from what the batch holds, so a read nobody asks this of pays nothing for it, and a
    # batch kept past the next read answers for itself.
    def faults
      @problems ||= listed
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

    # Bytes in one CellNote: a detail code and an origin code, two u16s.
    NOTE_BYTES = 4
    # Bytes in one RawProblem.
    PROBLEM_BYTES = 36
    # Bytes in one RawCheck: the keyword, adjusted and the enum index, three u32s.
    CHECK_BYTES = 12
    # The reason a verdict carries for a cell that cast and broke a declared constraint.
    VIOLATION = 4
    # A RawCheck's enum index when no text enum entry matched.
    NO_ENTRY = 0xFFFF_FFFF
    # What each origin code in a CellNote means.
    ORIGINS = %i[cell token padded uncached].freeze
    private_constant :NOTE_BYTES, :PROBLEM_BYTES, :CHECK_BYTES, :VIOLATION, :NO_ENTRY, :ORIGINS

    # What the core lists for this batch, as Problems.
    def listed
      raw = Runtime.faults(@verdict_bytes, @notes, @checks, @rows, @widths, @expected, @long_rows)
      Array.new(raw.bytesize / PROBLEM_BYTES) do |entry|
        problem(*raw.unpack("L<3S<2L<5", offset: entry * PROBLEM_BYTES))
      end.freeze
    end

    # One RawProblem as a Problem: the source column worked out, the span moved to characters.
    def problem(row, plan_column, reason, detail, origin, offset, length, expected, found, keyword)
      whole_row = plan_column == ROW_LEVEL
      offset, length = span(plan_column, row, offset, length) unless whole_row
      Problem.new(row: row, line: line(row), plan_column: whole_row ? nil : plan_column,
                  source_column: whole_row ? nil : @columns.fetch(plan_column).ordinal + @first_column,
                  reason: HyperCast::REASONS[reason], detail: detail.zero? ? nil : HyperCast::Detail.of(detail),
                  origin: ORIGINS.fetch(origin), offset: offset, length: length, expected: expected, found: found,
                  keyword: keyword.zero? ? nil : HyperCast::Keyword.new(code: keyword))
    end

    # A span the core counts in bytes of the cell's text, in the characters String#[] slices it
    # by — an identity unless the text has a multi-byte character.
    def span(column, row, offset, length)
      return [offset, length] if offset.zero? && length.zero?

      HyperCast::Interop.characters(raw(column, row), offset, length)
    end

    # +column+, or TypeError for a column that declares constraints, which a reader of verdicts
    # without them would read a violation of as passed.
    def unchecked(column)
      return column unless @columns.fetch(column).checked?

      raise TypeError, "column #{column} declares constraints: read it with get_checked"
    end

    # The verdict of the cell at (+column+, +row+) as its column is read: #get_checked's for a
    # column that declares constraints, #get's for one that doesn't. Row#to_a and
    # DelimitedReader#each_row read with it.
    def verdict_of(column, row)
      @columns.fetch(column).checked? ? get_checked(column, row) : get(column, row)
    end

    # A column's verdicts as #verdict_of reads them.
    def verdicts_of(column)
      @columns.fetch(column).checked? ? checked_verdicts(column) : verdicts(column)
    end

    # The plan column of an entry about the row as a whole.
    ROW_LEVEL = 0xFFFF_FFFF
    private_constant :ROW_LEVEL

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
    def verdict_words(index)
      flat = @words[index]
      if flat.nil?
        bytes = @verdict_bytes.fetch(index)
        flat = @words[index] = bytes.count("\0") == bytes.bytesize ? false : bytes.unpack("L<*")
      end
      flat || nil
    end

    # A column's values out of its array: one String#unpack over the whole column, then the
    # door's Ruby type built for each row that cast.
    def decode(column, index)
      door = column.door
      bytes = @bytes.fetch(index)
      faults = verdict_words(index)
      if (directive = SCALARS[door])
        values = bytes.unpack(directive)
        values.map! { |byte| byte != 0 } if door == :bool
        @rows.times { |row| values[row] = nil unless cast?(faults, row) } if faults
      elsif door == :text
        values = texts(bytes.unpack("L<*"), faults)
      else
        directive, width, build = RECORDS.fetch(door)
        fields = bytes.unpack(directive * @rows)
        values = Array.new(@rows) do |row|
          build.call(fields, row * width) if faults.nil? || cast?(faults, row)
        end
      end
      values.freeze
    end

    # A text column's values: each span sliced out of the input or the shared strings, or —
    # flagged — out of the arena.
    def texts(spans, faults)
      Array.new(@rows) do |row|
        slice(spans[row * 2], spans[row * 2 + 1]) if faults.nil? || cast?(faults, row)
      end
    end

    # Whether the cell at +row+ cast: it passed, or broke a constraint with the value it cast to.
    def cast?(faults, row)
      reason = faults[row * 3 + 2]
      reason.zero? || reason == VIOLATION
    end

    # A column's verdicts: a Success around each value, a Fault where the core says so.
    def judge(column)
      values = values(column)
      faults = verdict_words(column)
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
      detail = @notes.nil? ? 0 : @notes.fetch(column).unpack1("S<", offset: row * NOTE_BYTES)
      if reason == 1 && offset.zero? && length.zero?
        return EMPTY if detail.zero?
        return BLANK if detail == BLANK.detail.code
      end

      offset, length = span(column, row, offset, length)
      HyperCast::Interop.fault(reason, offset, length, detail)
    end
  end
end
