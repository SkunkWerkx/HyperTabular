module HyperTabular
  # The doors a column can be cast through, by the code the core knows each as: HyperCast's
  # twenty-one, named as its gem names them, plus :text for the bytes themselves.
  DOORS = {
    bool: 1, i8: 2, i16: 3, i32: 4, i64: 5, u8: 6, u16: 7, u32: 8, u64: 9, f32: 10, f64: 11,
    uuid: 12, timestamp: 13, unix: 14, date: 15, time: 16, duration: 17, text: 18, decimal: 19,
    date_ordered: 20, datetime: 21, excel_serial: 22
  }.freeze

  # One output column of a plan: which source column it reads, the door it casts through,
  # what that door declares, and — for the numeric doors — the notation. A plan is a
  # projection: a forty-column file can be read into five typed columns, in any order, and
  # a source column can be read through more than one door.
  #
  # Built by the factory named after its door, never guessed from the data:
  #
  #   plan = [
  #     HyperTabular::Column.i32(0),
  #     HyperTabular::Column.text(1),
  #     HyperTabular::Column.decimal(2, eurozone),
  #     HyperTabular::Column.date(3, :day_month_year),
  #     HyperTabular::Column.unix(4, :milliseconds)
  #   ]
  #
  # +ordinal+ is the zero-based source column; past a record's last cell it reads as empty.
  # +door+ is a key of DOORS. +declared+ is the Symbol the door was declared with — a Unix
  # precision, a date order, an Excel epoch, as HyperCast names them — or nil. +format+ is
  # the HyperCast::NumFormat of a numeric door, or nil: no format is Western grouping, as
  # HyperCast::NumFormat::INVARIANT is, and Indian grouping is declared per column. +tokens+ is
  # the list this column answers to in place of its dialect's or sheet's (#with_tokens), nil to
  # inherit theirs. +date_options+ and +months+ are the HyperCast::DateOptions and
  # HyperCast::MonthNames the date doors read (#with_date_options), or nil. +constraints+ is the
  # HyperCast::Constraints every cell of a numeric or text column must also meet, declared for this
  # column's door, or nil: a constrained column is read with Batch#get_checked.
  Column = Data.define(:ordinal, :door, :declared, :format, :tokens, :date_options, :months, :constraints) do
    # Checks the column as the caller bug it would otherwise become at the first read: an
    # ordinal that is not a non-negative Integer, a format that is not a HyperCast::NumFormat or
    # constraints that are not a HyperCast::Constraints for this door (ArgumentError), an unknown
    # door or an undeclared option (KeyError, as HyperCast raises for the same).
    def initialize(ordinal:, door:, declared: nil, format: nil, tokens: nil, date_options: nil, months: nil,
                   constraints: nil)
      raise ArgumentError, "ordinal must be an Integer at least 0, and an ordinal must be below 2^20 " \
                           "(1,048,576); got #{ordinal.inspect}" unless
        ordinal.is_a?(Integer) && ordinal.between?(0, Column::MAX_ORDINAL)

      DOORS.fetch(door)
      if (options = Column::DECLARES[door])
        options.fetch(declared)
      elsif !declared.nil?
        raise ArgumentError, "the #{door} door declares nothing; got #{declared.inspect}"
      end
      if Column::NUMERIC.include?(door)
        format = HyperCast::NumFormat::INVARIANT if format.nil?
        raise ArgumentError, "format must be a HyperCast::NumFormat; got #{format.inspect}" unless
          format.is_a?(HyperCast::NumFormat)
      elsif !format.nil?
        raise ArgumentError, "the #{door} door reads no numeric format"
      end

      raise ArgumentError, "date_options must be a HyperCast::DateOptions; got #{date_options.inspect}" unless
        date_options.nil? || date_options.is_a?(HyperCast::DateOptions)
      raise ArgumentError, "months must be a HyperCast::MonthNames; got #{months.inspect}" unless
        months.nil? || months.is_a?(HyperCast::MonthNames)

      unless constraints.nil?
        raise ArgumentError, "constraints must be a HyperCast::Constraints; got #{constraints.inspect}" unless
          constraints.is_a?(HyperCast::Constraints)

        # One that never went through Constraints.new raises what HyperCast raises for it.
        constraints.send(:declaration) if constraints.door.nil?
        raise ArgumentError, Column.mismatched(constraints.door, door) unless constraints.door == door
      end

      super(ordinal: ordinal, door: door, declared: declared, format: format,
            tokens: tokens.nil? ? nil : Declared.tokens(tokens), date_options: date_options, months: months,
            constraints: constraints)
    end

    # The same column answering to these tokens rather than its dialect's or its sheet's: a cell
    # whose exact, unquoted bytes are one of them is an :empty Fault, whatever its door, at Origin
    # :token. An empty list means none, so a column can opt out of what it would inherit. On a
    # sheet, only a text cell can be one.
    def with_tokens(list)
      with(tokens: list)
    end

    # The same column reading dates under these HyperCast::DateOptions and, optionally, this
    # HyperCast::MonthNames: the timestamp, date, date_ordered and datetime doors read them, as
    # HyperCast's own do. On a sheet they reach only a text cell, since a typed date cell has no
    # text for them to read. A declaration the core refuses is an ArgumentError when the plan is
    # bound.
    def with_date_options(options, months = nil)
      with(date_options: options, months: months)
    end

    # A boolean column: HyperCast's boolean lexicon, as true or false.
    def self.bool(ordinal) = new(ordinal: ordinal, door: :bool)

    # A signed 8-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i8(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :i8, format: format, constraints: constraints)
    end

    # A signed 16-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i16(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :i16, format: format, constraints: constraints)
    end

    # A signed 32-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i32(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :i32, format: format, constraints: constraints)
    end

    # A signed 64-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i64(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :i64, format: format, constraints: constraints)
    end

    # An unsigned 8-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u8(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :u8, format: format, constraints: constraints)
    end

    # An unsigned 16-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u16(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :u16, format: format, constraints: constraints)
    end

    # An unsigned 32-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u32(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :u32, format: format, constraints: constraints)
    end

    # An unsigned 64-bit column, as the true unsigned Integer. Invariant notation unless
    # one is declared.
    def self.u64(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :u64, format: format, constraints: constraints)
    end

    # An IEEE single column, widened losslessly to a Float. Invariant notation unless one
    # is declared.
    def self.f32(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :f32, format: format, constraints: constraints)
    end

    # An IEEE double column, as a Float. Invariant notation unless one is declared.
    def self.f64(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :f64, format: format, constraints: constraints)
    end

    # An exact decimal column, as a HyperCast::Decimal; no float is ever formed. Invariant
    # notation unless one is declared.
    def self.decimal(ordinal, format = HyperCast::NumFormat::INVARIANT, constraints: nil)
      new(ordinal: ordinal, door: :decimal, format: format, constraints: constraints)
    end

    # A UUID column, as the lowercase hyphenated String.
    def self.uuid(ordinal) = new(ordinal: ordinal, door: :uuid)

    # An RFC 3339 instant column, as a UTC Time at full nanosecond fidelity.
    def self.timestamp(ordinal) = new(ordinal: ordinal, door: :timestamp)

    # A Unix-epoch column at the declared precision (:seconds, :milliseconds,
    # :microseconds, :nanoseconds) — never guessed from magnitude — as a UTC Time.
    def self.unix(ordinal, precision) = new(ordinal: ordinal, door: :unix, declared: precision)

    # An Excel date-serial column under the declared date system (:y1900, :y1904), as a
    # UTC Time.
    def self.excel_serial(ordinal, epoch) = new(ordinal: ordinal, door: :excel_serial, declared: epoch)

    # A calendar date column, as a Date. With no order declared: the strict ISO 8601
    # yyyy-MM-dd door. With one (:year_month_day, :month_day_year, :day_month_year): the
    # date_ordered door, which also reads the separated forms — the same split
    # HyperCast.date makes.
    def self.date(ordinal, order = nil)
      order.nil? ? new(ordinal: ordinal, door: :date) : date_ordered(ordinal, order)
    end

    # A separated calendar date column under the declared field order, as a Date.
    def self.date_ordered(ordinal, order) = new(ordinal: ordinal, door: :date_ordered, declared: order)

    # A zone-less civil date-time column under the declared field order, as a DateTime
    # whose +00:00 offset is a carrier artifact, not data — the text named no zone.
    def self.datetime(ordinal, order) = new(ordinal: ordinal, door: :datetime, declared: order)

    # A 24-hour time-of-day column, as an exact Integer of nanoseconds since midnight.
    def self.time(ordinal) = new(ordinal: ordinal, door: :time)

    # A duration column, as exact Rational seconds.
    def self.duration(ordinal) = new(ordinal: ordinal, door: :duration)

    # A text column: the cell's bytes themselves, untrimmed, quotes resolved, as a UTF-8
    # String. A cell with no bytes at all is the one way text fails (an :empty Fault).
    def self.text(ordinal, constraints: nil) = new(ordinal: ordinal, door: :text, constraints: constraints)

    # +plan+ as a reader holds it: a frozen Array of its own, every entry a Column — what
    # DelimitedReader and Sheet check a plan with. ArgumentError naming the first entry that
    # is not a Column.
    def self.plan(plan)
      plan = Array(plan).dup.freeze
      plan.each_with_index do |column, index|
        raise ArgumentError, "plan column #{index} must be a HyperTabular::Column; got #{column.inspect}" unless
          column.is_a?(Column)
      end
      plan
    end

    # +plan+ as the runtime takes it: for each column its fixed bytes, its month vocabulary, its
    # tokens (see #packed_with), and its constraints' packed bytes and packed enum, or nils. What a
    # column declares is judged here, ahead of any input, so a plan the core would refuse is an
    # ArgumentError naming the column at bind or open, a retired grouping word among them, and never
    # a read that fails later.
    def self.specs(plan)
      specs = plan.each_with_index.map do |column, index|
        [column.packed_with, column.months&.packed, column.packed_tokens, *column.packed_constraints]
      rescue ArgumentError => e
        raise ArgumentError, "plan column #{index} cannot be honoured: #{e.message}"
      end
      refused = Runtime.refused(specs)
      return specs if refused.nil?

      raise ArgumentError, "plan column #{refused} cannot be honoured: the core refuses what it declares"
    end

    # The column a door's code and what it declares name, as the core numbers them, or nil for
    # a code this version does not know. A numeric door reads +format+.
    def self.of_code(ordinal, code, param, format = nil)
      door = DOORS.key(code)
      return nil if door.nil?

      declares = Column::DECLARES[door]
      declared = declares&.key(param)
      return nil if declares && declared.nil?

      new(ordinal: ordinal, door: door, declared: declared, format: Column::NUMERIC.include?(door) ? format : nil)
    end

    # Bytes one value of this column's door takes in a column buffer.
    def value_bytes
      Column::VALUE_BYTES.fetch(door)
    end

    # The fixed part of the core's ColumnSpecWith, the one every read goes through: its size,
    # the ordinal, the door code, what the door declares, the notation (see #packed), and the date
    # options in HyperCast's packed form (twelve zeros, a size of zero, for none). The runtime
    # appends the two pointers and lengths its months and tokens live at. A format holding the
    # retired grouping bit raises ArgumentError here, from NumFormat#raw_flags.
    def packed_with
      param = declared.nil? ? 0 : Column::DECLARES.fetch(door).fetch(declared)
      [Column::SPEC_WITH_BYTES, ordinal, DOORS.fetch(door), param].pack("L<4") +
        (format.nil? ? Column::NO_FORMAT : format.packed) +
        (date_options.nil? ? Column::NO_DATE_OPTIONS : date_options.packed)
    end

    # The column's own tokens as the core takes them: nil to inherit, an empty String for none.
    def packed_tokens
      tokens.nil? ? nil : Declared.packed_tokens(tokens)
    end

    # The constraints as the core takes them: HyperCast's 104 packed bytes and its packed enum, or
    # two nils for none. HyperCast judges the declaration again here, so one that never went
    # through Constraints.new, or a column whose door is not the declaration's however it was
    # built, is the ArgumentError Constraints.new or Column.new raises.
    def packed_constraints
      return [nil, nil] if constraints.nil?
      raise ArgumentError, "constraints must be a HyperCast::Constraints; got #{constraints.inspect}" unless
        constraints.is_a?(HyperCast::Constraints)

      code, packed, packed_enum = constraints.send(:declaration)
      raise ArgumentError, Column.mismatched(constraints.door, door) unless code == DOORS[door]

      [packed, packed_enum]
    end

    # Whether the column declares constraints, and so is read with Batch#get_checked.
    def checked?
      !constraints.nil?
    end

    # The 104 little-endian bytes of the core's 1.0 ColumnSpec, which no read takes and the 1.0
    # exports stay proved with: ordinal, door code, what the
    # door declares (numbered as HyperCast numbers it), then the notation in HyperCast's
    # own packed form (its 92-byte size-tagged layout) — all zeros, which the core reads
    # as invariant, for a door that reads none.
    def packed
      param = declared.nil? ? 0 : Column::DECLARES.fetch(door).fetch(declared)
      [ordinal, DOORS.fetch(door), param].pack("L<3") + (format.nil? ? Column::NO_FORMAT : format.packed)
    end
  end

  # The words a declaration for one door given to a column of another is refused in.
  def Column.mismatched(declared, door)
    "Constraints for :#{declared} were given to a :#{door} column"
  end

  # The widest source ordinal a column can name: an ordinal must be below 2^20. No workbook
  # format has that many columns (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the
  # cap one row of cell table is at most 8 MiB.
  Column::MAX_ORDINAL = (1 << 20) - 1

  # The doors that read a HyperCast::NumFormat: the integers, the reals and the decimal.
  Column::NUMERIC = %i[i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 decimal].freeze

  # What the doors that declare something declare, as HyperCast's own tables number it.
  Column::DECLARES = {
    unix: HyperCast::UNIX_PRECISIONS,
    excel_serial: HyperCast::EXCEL_EPOCHS,
    date_ordered: HyperCast::DATE_ORDERS,
    datetime: HyperCast::DATE_ORDERS
  }.freeze

  # Bytes per value in a column buffer, by door (rust/src/kernel/abi.rs, ColumnBuffer):
  # HyperCast's for its doors, and a text cell's span — two u32s — for this one's.
  Column::VALUE_BYTES = HyperCast::Interop::VALUE_BYTES.merge(text: 8).freeze

  # The notation of a door that reads none: ninety-two zero bytes, a size of zero.
  Column::NO_FORMAT = ("\0" * HyperCast::NumFormat::PACKED_BYTES).b.freeze

  # The date options of a column that declares none: a size of zero.
  Column::NO_DATE_OPTIONS = ("\0" * HyperCast::DateOptions::PACKED_BYTES).b.freeze

  # Bytes in the fixed part of a ColumnSpecWith, Column#packed_with: four words, the notation and
  # the date options.
  Column::SPEC_WITH_FIXED = 16 + HyperCast::NumFormat::PACKED_BYTES + HyperCast::DateOptions::PACKED_BYTES

  # Bytes in a whole ColumnSpecWith, what its first word carries: the fixed part, then the month
  # names and tokens, each a pointer and its length, then the constraints' pointer and the packed
  # enum's pointer and length.
  Column::SPEC_WITH_BYTES = Column::SPEC_WITH_FIXED + 7 * [0].pack("J").bytesize
end
