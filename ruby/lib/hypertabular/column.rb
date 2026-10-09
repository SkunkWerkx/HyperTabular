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
  # the HyperCast::NumFormat of a numeric door, or nil.
  Column = Data.define(:ordinal, :door, :declared, :format) do
    # Checks the column as the caller bug it would otherwise become at the first read: an
    # ordinal that is not a non-negative Integer or a format that is not a
    # HyperCast::NumFormat (ArgumentError), an unknown door or an undeclared option
    # (KeyError, as HyperCast raises for the same).
    def initialize(ordinal:, door:, declared: nil, format: nil)
      raise ArgumentError, "ordinal must be an Integer in 0..#{Column::MAX_ORDINAL}; got #{ordinal.inspect}" unless
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

      super(ordinal: ordinal, door: door, declared: declared, format: format)
    end

    # A boolean column: HyperCast's boolean lexicon, as true or false.
    def self.bool(ordinal) = new(ordinal: ordinal, door: :bool)

    # A signed 8-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i8(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :i8, format: format)

    # A signed 16-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i16(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :i16, format: format)

    # A signed 32-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i32(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :i32, format: format)

    # A signed 64-bit column, as an Integer. Invariant notation unless one is declared.
    def self.i64(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :i64, format: format)

    # An unsigned 8-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u8(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :u8, format: format)

    # An unsigned 16-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u16(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :u16, format: format)

    # An unsigned 32-bit column, as an Integer. Invariant notation unless one is declared.
    def self.u32(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :u32, format: format)

    # An unsigned 64-bit column, as the true unsigned Integer. Invariant notation unless
    # one is declared.
    def self.u64(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :u64, format: format)

    # An IEEE single column, widened losslessly to a Float. Invariant notation unless one
    # is declared.
    def self.f32(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :f32, format: format)

    # An IEEE double column, as a Float. Invariant notation unless one is declared.
    def self.f64(ordinal, format = HyperCast::NumFormat::INVARIANT) = new(ordinal: ordinal, door: :f64, format: format)

    # An exact decimal column, as a HyperCast::Decimal; no float is ever formed. Invariant
    # notation unless one is declared.
    def self.decimal(ordinal, format = HyperCast::NumFormat::INVARIANT)
      new(ordinal: ordinal, door: :decimal, format: format)
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
    def self.text(ordinal) = new(ordinal: ordinal, door: :text)

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

    # Bytes one value of this column's door takes in a column buffer.
    def value_bytes
      Column::VALUE_BYTES.fetch(door)
    end

    # The 44 little-endian bytes of the core's ColumnSpec: ordinal, door code, what the
    # door declares (numbered as HyperCast numbers it), then the notation in HyperCast's
    # own packed form — all zeros, which the core reads as invariant, for a door that
    # reads none.
    def packed
      param = declared.nil? ? 0 : Column::DECLARES.fetch(door).fetch(declared)
      [ordinal, DOORS.fetch(door), param].pack("L<3") + (format.nil? ? Column::NO_FORMAT : format.packed)
    end
  end

  # The widest source ordinal a column can name.
  Column::MAX_ORDINAL = (1 << 31) - 1

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

  # The notation of a door that reads none: thirty-two zero bytes.
  Column::NO_FORMAT = ("\0" * 32).b.freeze
end
